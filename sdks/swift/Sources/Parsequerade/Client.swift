import Foundation
import ParsequeradeCore

private func decodeObject(_ pointer: UnsafeMutablePointer<CChar>?) throws -> [String: Any] {
    guard let pointer else { throw ApiError(500, "native_core_failed", "The native execution core did not return a result.") }
    defer { pq_string_free(pointer) }
    return try decodeObject(String(cString: pointer))
}
private func decodeObject(_ text: String) throws -> [String: Any] {
    guard let data = text.data(using: .utf8),
          let value = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw ApiError(500, "native_core_failed", "The native execution core returned an invalid result.")
    }
    if let error = value["error"] {
        throw try JSONDecoder().decode(ApiError.self, from: JSONSerialization.data(withJSONObject: error))
    }
    return value
}
private func json(_ value: Any) throws -> String {
    do { return String(decoding: try JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed, .sortedKeys]), as: UTF8.self) }
    catch { throw ApiError(400, "invalid_request", "Provide a JSON-compatible request.") }
}
private func object<T: Encodable>(_ value: T) throws -> Any {
    do { return try JSONSerialization.jsonObject(with: JSONEncoder().encode(value), options: .fragmentsAllowed) }
    catch { throw ApiError(400, "invalid_request", "Provide a JSON-compatible request.") }
}
private func decode<T: Decodable>(_ value: Any, as type: T.Type) throws -> T {
    do { return try JSONDecoder().decode(type, from: JSONSerialization.data(withJSONObject: value, options: .fragmentsAllowed)) }
    catch { throw ApiError(500, "response_type_mismatch", "The requested Swift type does not match the facade response.") }
}

/// Async Swift facade over the same execution core as Rust. Reuse a client to
/// share authentication and cursor signing. No network request occurs at creation.
public actor Client {
    private var handle: UInt64?
    private let transport: AppleTransport?
    private var operations: [UInt64: OperationCancellation] = [:]

    public init(issuerID: String, keyID: String, privateKey: String, transport: AppleTransport? = nil) throws {
        guard pq_abi_version() == 1 else {
            throw ApiError(500, "native_abi_mismatch", "The native execution core is incompatible with this SDK.")
        }
        let config = try json(["issuerId": issuerID, "keyId": keyID, "privateKey": privateKey, "customTransport": transport != nil])
        let result = try config.withCString { try decodeObject(pq_client_new($0)) }
        guard let handle = result["client"] as? UInt64 else { throw ApiError.invalidUpstream() }
        self.handle = handle
        self.transport = transport
    }

    deinit { if let handle { pq_client_free(handle) } }

    /// Cancel outstanding operations and release the client. Cancellation of a
    /// mutation may report write_outcome_unknown; keep its checkpoint for recovery.
    public func close() {
        if let handle {
            for operation in operations.values { operation.cancel() }
            pq_client_free(handle)
            self.handle = nil
        }
    }

    /// Read a facade path, preserving exact JSON nulls when decoded as JSONValue.
    public func get<T: Decodable & Sendable>(_ path: String, query: Query = [:], as type: T.Type = T.self) async throws -> T {
        try await execute("get", args: [path, query.sorted { $0.key < $1.key }.map { [$0.key, $0.value] }], as: type)
    }

    /// Resolve ownership and send exactly one PATCH. Read back after an uncertain write.
    public func updateAgeRating(appId: String, appInfoId: String, attributes: AgeRatingUpdate) async throws -> AgeRating {
        try await execute("update_age_rating", args: [appId, appInfoId, try object(attributes)], as: AgeRating.self)
    }

    /// Update supported metadata on an app, version, or localization facade path.
    public func updateMetadata(_ path: String, attributes: [String: String]) async throws -> JSONValue {
        try await execute("update_metadata", args: [path, attributes], as: JSONValue.self)
    }

    /// Create one unsubmitted version. Read versions before retrying an uncertain write.
    public func createVersion(appId: String, platform: String, versionString: String) async throws -> Version {
        try await execute("create_version", args: [appId, platform, versionString], as: Version.self)
    }

    /// Attach an eligible build to its matching draft version and verify the result.
    public func attachBuild(appId: String, versionId: String, buildId: String) async throws -> Version {
        try await execute("attach_build", args: [appId, versionId, buildId], as: Version.self)
    }

    /// Read Apple's live image specifications and placement groups without writing.
    public func listingImageReferenceData() async throws -> JSONValue {
        try await execute("listing_image_reference_data", args: [], as: JSONValue.self)
    }

    /// Upload or resume a PNG. Persist each snapshot durably before returning from
    /// the closure; a thrown persistence error is propagated unchanged before the next
    /// write. The supplied checkpoint is updated even when the operation fails.
    public func uploadListingImage(_ request: ListingImageUpload, png: Data,
                                   checkpoint: ListingImageUploadCheckpoint,
                                   persist: @escaping @Sendable (ListingImageUploadCheckpoint) async throws -> Void) async throws -> ListingImageUploadReceipt {
        try checkpoint.begin()
        defer { checkpoint.end() }
        return try await execute("upload_listing_image", args: [try object(request), try object(checkpoint)],
                                 png: png, checkpoint: checkpoint, persist: persist, as: ListingImageUploadReceipt.self)
    }

    private func execute<T: Decodable & Sendable>(_ operation: String, args: [Any], png: Data? = nil,
                          checkpoint: ListingImageUploadCheckpoint? = nil,
                          persist: (@Sendable (ListingImageUploadCheckpoint) async throws -> Void)? = nil,
                          as type: T.Type) async throws -> T {
        guard let handle else { throw ApiError(400, "client_closed", "This client is closed.") }
        if Task.isCancelled { throw ApiError(499, "operation_cancelled", "The operation was cancelled.") }
        var request: [String: Any] = ["operation": operation, "args": args]
        if let png { request["png"] = png.base64EncodedString() }
        let encoded = try json(request)
        let started = try encoded.withCString { try decodeObject(pq_start(handle, $0)) }
        guard let id = started["operation"] as? UInt64 else { throw ApiError.invalidUpstream() }
        let cancellation = OperationCancellation(id)
        operations[id] = cancellation
        defer { operations.removeValue(forKey: id); pq_operation_free(id) }
        return try await withTaskCancellationHandler {
            var persistenceError: Error?
            while true {
                // Polling is blocking. Keep it on a Dispatch worker, never Swift's
                // cooperative executor, and suspend the caller while waiting.
                let next: String? = await withCheckedContinuation { continuation in
                    DispatchQueue.global(qos: .utility).async {
                        guard let pointer = pq_poll(id, 50) else { continuation.resume(returning: nil); return }
                        let text = String(cString: pointer)
                        pq_string_free(pointer)
                        continuation.resume(returning: text)
                    }
                }
                guard let next, next != "null" else { continue }
                // Final errors must be handled after copying the final checkpoint.
                guard let event = try JSONSerialization.jsonObject(with: Data(next.utf8)) as? [String: Any],
                      let kind = event["kind"] as? String else { throw ApiError.invalidUpstream() }
                if kind == "result" {
                    if let snapshot = event["checkpoint"] { checkpoint?.replace(try decode(snapshot, as: [String: JSONValue].self)) }
                    if let persistenceError { throw persistenceError }
                    _ = try decodeObject(next)
                    guard let value = event["value"] else { throw ApiError.invalidUpstream() }
                    return try decode(value, as: type)
                }
                guard let eventID = event["id"] as? UInt64 else { throw ApiError.invalidUpstream() }
                var response: [String: Any]
                if kind == "http" {
                    response = Task.isCancelled || self.handle == nil ? ["error": "failed"] : await send(event, cancellation: cancellation)
                } else if kind == "checkpoint", let snapshot = event["checkpoint"], let persist {
                    let values = try decode(snapshot, as: [String: JSONValue].self)
                    checkpoint?.replace(values)
                    let copy = ListingImageUploadCheckpoint()
                    copy.replace(values)
                    if Task.isCancelled || self.handle == nil { response = ["error": "checkpoint_persistence_failed"] }
                    else {
                        do { try await invokePersistence(persist, snapshot: copy, cancellation: cancellation, owner: checkpoint); response = ["ok": true] }
                        catch is CallbackAborted { response = ["error": "checkpoint_persistence_failed"] }
                        catch { persistenceError = error; response = ["error": "checkpoint_persistence_failed"] }
                    }
                } else { throw ApiError.invalidUpstream() }
                let text = try json(response)
                _ = text.withCString { pq_respond(id, eventID, $0) }
            }
        } onCancel: { cancellation.cancel() }
    }

    private func send(_ event: [String: Any], cancellation: OperationCancellation) async -> [String: Any] {
        guard let transport, let rawURL = event["url"] as? String, let url = URL(string: rawURL),
              let method = event["method"] as? String, let timeout = event["timeoutMs"] as? Double else {
            return ["error": "invalid_response"]
        }
        let now = Date().timeIntervalSince1970
        let deadline = min(now + timeout / 1000, (event["deadlineMs"] as? Double).map { $0 / 1000 } ?? .infinity)
        guard deadline > now else { return ["error": "timeout"] }
        var request = URLRequest(url: url, timeoutInterval: deadline - now)
        request.httpMethod = method
        request.allHTTPHeaderFields = event["headers"] as? [String: String]
        if let body = event["body"] as? String { request.httpBody = Data(base64Encoded: body) }
        do {
            let (data, response) = try await invokeTransport(transport, request, cancellation: cancellation, deadline: deadline)
            guard data.count <= (event["maxResponseBytes"] as? Int ?? 0),
                  response.url == request.url else { return ["error": "invalid_response"] }
            var headers = [String: String]()
            for (key, value) in response.allHeaderFields { headers[String(describing: key)] = String(describing: value) }
            return ["status": response.statusCode, "headers": headers, "body": data.base64EncodedString()]
        } catch {
            return ["error": (error as? URLError)?.code == .timedOut ? "timeout" : "failed"]
        }
    }
}
