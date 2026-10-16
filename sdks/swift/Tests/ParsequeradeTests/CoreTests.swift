import XCTest
import Foundation
import CryptoKit
@testable import Parsequerade

private func loadFixtureObject(_ name: String) throws -> [String: Any] {
    try JSONSerialization.jsonObject(with: Data(contentsOf: Bundle.module.url(forResource: name, withExtension: "json")!)) as! [String: Any]
}
private func parseJSONValue(_ object: Any) throws -> JSONValue {
    try JSONDecoder().decode(JSONValue.self, from: JSONSerialization.data(withJSONObject: object, options: .fragmentsAllowed))
}
private func encoded<T: Encodable>(_ object: T) throws -> JSONValue {
    try JSONDecoder().decode(JSONValue.self, from: JSONEncoder().encode(object))
}
private func facadeQuery(_ object: Any) throws -> Query {
    try (object as! [String: Any]).mapValues { value in
        if let string = value as? String { return string }
        return String(decoding: try JSONSerialization.data(withJSONObject: value, options: .fragmentsAllowed), as: UTF8.self)
    }
}
private func subset(_ actual: JSONValue, _ expected: JSONValue, _ name: String, file: StaticString = #filePath, line: UInt = #line) {
    switch expected {
    case .object(let fields):
        guard case .object(let found) = actual else { return XCTFail("\(name): expected object, got \(actual)", file: file, line: line) }
        for (key, expected) in fields {
            guard let found = found[key] else { XCTFail("\(name): missing \(key)", file: file, line: line); continue }
            subset(found, expected, name, file: file, line: line)
        }
    case .array(let items):
        guard case .array(let found) = actual else { return XCTFail("\(name): expected array", file: file, line: line) }
        XCTAssertEqual(found.count, items.count, name, file: file, line: line)
        for (actual, expected) in zip(found, items) { subset(actual, expected, name, file: file, line: line) }
    default: XCTAssertEqual(actual, expected, name, file: file, line: line)
    }
}

private actor CoreMock {
    var steps: [[String: Any]]
    var calls: [URLRequest] = []
    init(_ steps: [[String: Any]]) { self.steps = steps }
    func send(_ request: URLRequest) throws -> (Data, HTTPURLResponse) {
        calls.append(request)
        guard !steps.isEmpty else { XCTFail("Unexpected request \(request.httpMethod ?? "") \(request.url!.path)"); throw URLError(.badServerResponse) }
        let step = steps.removeFirst()
        XCTAssertEqual(request.httpMethod, step["method"] as? String)
        XCTAssertEqual(request.url?.path, step["path"] as? String)
        let pairs = URLComponents(url: request.url!, resolvingAgainstBaseURL: false)!.queryItems ?? []
        let query = Dictionary(uniqueKeysWithValues: pairs.map { ($0.name, $0.value ?? "") })
        XCTAssertEqual(query, step["query"] as? [String: String] ?? [:])
        if let body = step["body"], !(body is NSNull) {
            XCTAssertEqual(try parseJSONValue(JSONSerialization.jsonObject(with: request.httpBody!)), try parseJSONValue(body))
        } else if let body = step["bodyBase64"] as? String {
            XCTAssertEqual(request.httpBody, Data(base64Encoded: body))
            XCTAssertNil(request.value(forHTTPHeaderField: "Authorization"))
        } else { XCTAssertNil(request.httpBody) }
        if step["failure"] as? Bool == true || step["error"] as? String == "failed" { throw URLError(.networkConnectionLost) }
        let payload = step["response"] ?? step["payload"] ?? [:]
        return (try JSONSerialization.data(withJSONObject: payload),
                HTTPURLResponse(url: request.url!, statusCode: step["status"] as? Int ?? 200,
                                httpVersion: "HTTP/1.1", headerFields: ["Content-Type": "application/json"])!)
    }
    func remaining() -> Int { steps.count }
    func requests() -> [URLRequest] { calls }
}
private func client(_ mock: CoreMock) throws -> Client {
    try Client(issuerID: "issuer", keyID: "key", privateKey: P256.Signing.PrivateKey().pemRepresentation,
               transport: { try await mock.send($0) })
}
private let imageRequest = ListingImageUpload(appId: "app1", versionId: "version1", locale: "en-US",
                                            placementType: .productPageHeaderAsset, fileName: "listing.png")

final class CoreTests: XCTestCase {
    func testSharedEighteenScenariosThroughPublicAPI() async throws {
        let url = Bundle.module.url(forResource: "scenarios", withExtension: "json")!
        let scenarios = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [[String: Any]]
        XCTAssertEqual(scenarios.count, 18)
        for scenario in scenarios {
            let name = scenario["name"] as! String
            let mock = CoreMock(scenario["steps"] as! [[String: Any]])
            let sdk = try client(mock)
            let args = scenario["args"] as! [Any]
            do {
                let result: JSONValue
                switch scenario["operation"] as! String {
                case "get":
                    // Exercise all ten additional typed read methods, including nested Codable models.
                    switch name {
                    case "screenshot_sets_resolve_exact_locale": result = try await encoded(sdk.screenshotSets(appId: "app1", versionId: "version1", locale: "en-US", query: try facadeQuery(args[1])))
                    case "screenshots_preserve_nested_assets_and_null": result = try await sdk.get(args[0] as! String, query: try facadeQuery(args[1]))
                    case "preview_sets_preserve_future_read_enums": result = try await encoded(sdk.previewSets(appId: "app1", versionId: "version1", locale: "en-US"))
                    case "previews_preserve_delivery_and_upload_operations": result = try await sdk.get(args[0] as! String, query: try facadeQuery(args[1]))
                    case "placements_include_images_and_scoped_parent": result = try await encoded(sdk.placements(appId: "app1", versionId: "version1", locale: "en-US", query: try facadeQuery(args[1])))
                    case "library_image_preserves_nullable_upload_operations": result = try await sdk.get(args[0] as! String, query: try facadeQuery(args[1]))
                    case "builds_translate_app_platform_and_version_filters": result = try await encoded(sdk.builds(appId: "app1", query: try facadeQuery(args[1])))
                    case "build_proves_app_ownership": result = try await encoded(sdk.build(appId: "app1", buildId: "build1"))
                    case "version_build_proves_version_ownership": result = try await encoded(sdk.versionBuild(appId: "app1", versionId: "version1"))
                    case "prerelease_version_proves_build_ownership": result = try await encoded(sdk.buildPrereleaseVersion(appId: "app1", buildId: "build1"))
                    default: result = try await sdk.get(args[0] as! String, query: try facadeQuery(args[1]))
                    }
                case "update_metadata": result = try await sdk.updateMetadata(args[0] as! String, attributes: args[1] as! [String: String])
                case "create_version": result = try await encoded(sdk.createVersion(appId: args[0] as! String, platform: args[1] as! String, versionString: args[2] as! String))
                case "attach_build": result = try await encoded(sdk.attachBuild(appId: args[0] as! String, versionId: args[1] as! String, buildId: args[2] as! String))
                default: return XCTFail("Unknown scenario operation")
                }
                XCTAssertNil(scenario["error"], name)
                if let expect = scenario["expect"] { subset(result, try parseJSONValue(expect), name) }
            } catch let error as ApiError {
                guard let expected = scenario["error"] as? [String: Any] else { XCTFail("\(name): \(error)"); continue }
                XCTAssertEqual(error.code, expected["code"] as? String, name)
                if let status = expected["status"] as? Int { XCTAssertEqual(error.status, status, name) }
            }
            let remaining = await mock.remaining()
            XCTAssertEqual(remaining, 0, name)
        }
    }

    func testUploadCheckpointsAndVerifiedReceipt() async throws {
        let fixture = try loadFixtureObject("uploads")
        let uploadCase = (fixture["cases"] as! [[String: Any]]).first { $0["name"] as? String == "complete_chunked_upload" }!
        let mock = CoreMock(uploadCase["steps"] as! [[String: Any]])
        let sdk = try client(mock)
        let checkpoint = ListingImageUploadCheckpoint()
        let snapshots = SnapshotStore()
        let receipt = try await sdk.uploadListingImage(imageRequest, png: Data(base64Encoded: fixture["png"] as! String)!, checkpoint: checkpoint) {
            try await snapshots.save($0)
        }
        subset(try encoded(receipt), try parseJSONValue(fixture["receipt"]!), "receipt")
        let saved = await snapshots.values()
        XCTAssertEqual(saved.count, 7)
        XCTAssertEqual(saved.compactMap { if case .object(let object) = $0, case .string(let phase) = object["phase"] { return phase }; return nil },
                       ["reservation_started", "reserved", "upload_started", "commit_started", "committed", "placement_started", "placement_created"])
        subset(try encoded(checkpoint), .object(["phase": .string("placement_created"), "placementId": .string("placement1")]), "checkpoint")
        let remaining = await mock.remaining()
        XCTAssertEqual(remaining, 0)
    }

    func testPersistenceErrorPreservedAndCheckpointCannotBeReentered() async throws {
        let fixture = try loadFixtureObject("uploads")
        let uploadCase = (fixture["cases"] as! [[String: Any]]).first { $0["name"] as? String == "persistence_failure" }!
        let mock = CoreMock(uploadCase["steps"] as! [[String: Any]])
        let sdk = try client(mock)
        let checkpoint = ListingImageUploadCheckpoint()
        let png = Data(base64Encoded: fixture["png"] as! String)!
        do {
            _ = try await sdk.uploadListingImage(imageRequest, png: png, checkpoint: checkpoint) { _ in
                do {
                    _ = try await sdk.uploadListingImage(imageRequest, png: png, checkpoint: checkpoint) { _ in }
                    XCTFail("Concurrent checkpoint reuse should fail")
                } catch let error as ApiError { XCTAssertEqual(error.code, "checkpoint_in_use") }
                throw PersistenceFailure.diskFull
            }
            XCTFail("Expected persistence failure")
        } catch PersistenceFailure.diskFull { }
        subset(try encoded(checkpoint), .object(["phase": .string("reservation_started")]), "failed persistence checkpoint")
        let requests = await mock.requests()
        XCTAssertTrue(requests.allSatisfy { $0.httpMethod == "GET" })
        let remaining = await mock.remaining()
        XCTAssertEqual(remaining, 0)
    }

    func testResumeUncertainPlacementMakesOnlyReads() async throws {
        let fixture = try loadFixtureObject("uploads")
        let uploadCase = (fixture["cases"] as! [[String: Any]]).first { $0["name"] as? String == "resume_placement" }!
        let mock = CoreMock(uploadCase["steps"] as! [[String: Any]])
        let sdk = try client(mock)
        let checkpoint = try JSONDecoder().decode(ListingImageUploadCheckpoint.self,
            from: JSONSerialization.data(withJSONObject: ["binding": fixture["binding"]!, "imageId": "image1", "phase": "placement_started", "placementGroup": "CREATIVE_IOS"]))
        let receipt = try await sdk.uploadListingImage(imageRequest, png: Data(base64Encoded: fixture["png"] as! String)!, checkpoint: checkpoint) { _ in }
        subset(try encoded(receipt), try parseJSONValue(fixture["receipt"]!), "resumed receipt")
        let calls = await mock.requests()
        XCTAssertTrue(calls.allSatisfy { $0.httpMethod == "GET" })
        let remaining = await mock.remaining()
        XCTAssertEqual(remaining, 0)
    }

    func testSharedUncertainUploadsNeverReplayWrites() async throws {
        let fixture = try loadFixtureObject("uploads")
        for uploadCase in fixture["cases"] as! [[String: Any]] where uploadCase["error"] != nil {
            let name = uploadCase["name"] as! String
            let mock = CoreMock(uploadCase["steps"] as! [[String: Any]])
            let sdk = try client(mock)
            let checkpoint = try JSONDecoder().decode(ListingImageUploadCheckpoint.self,
                from: JSONSerialization.data(withJSONObject: uploadCase["checkpoint"] ?? [:]))
            let png = Data(base64Encoded: fixture["png"] as! String)!
            do { _ = try await sdk.uploadListingImage(imageRequest, png: png, checkpoint: checkpoint) { _ in }; XCTFail("Expected uncertain outcome: \(name)") }
            catch let error as ApiError { XCTAssertEqual(error.code, "write_outcome_unknown", name) }
            let calls = await mock.requests()
            let remaining = await mock.remaining()
            XCTAssertEqual(remaining, 0, name)
            if name == "uncertain_reservation" {
                // Reusing the saved state must never reserve a second image.
                do { _ = try await sdk.uploadListingImage(imageRequest, png: png, checkpoint: checkpoint) { _ in }; XCTFail("Expected uncertain reservation") }
                catch let error as ApiError { XCTAssertEqual(error.code, "write_outcome_unknown") }
                let after = await mock.requests()
                XCTAssertEqual(calls.count, after.count)
            }
            if name == "uncertain_placement_missing" { XCTAssertTrue(calls.allSatisfy { $0.httpMethod == "GET" }) }
        }
    }

    func testNestedTypedModels() async throws {
        let url = Bundle.module.url(forResource: "scenarios", withExtension: "json")!
        let scenarios = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [[String: Any]]
        for scenario in scenarios {
            let name = scenario["name"] as! String
            guard ["screenshots_preserve_nested_assets_and_null", "previews_preserve_delivery_and_upload_operations", "library_image_preserves_nullable_upload_operations"].contains(name) else { continue }
            let mock = CoreMock(scenario["steps"] as! [[String: Any]])
            let sdk = try client(mock)
            switch name {
            case "screenshots_preserve_nested_assets_and_null":
                let page = try await sdk.screenshots(screenshotSetId: "screens1")
                XCTAssertEqual(page.items.count, 1)
                XCTAssertNotNil(page.items[0].imageAsset?.width)
            case "previews_preserve_delivery_and_upload_operations":
                let page = try await sdk.previews(previewSetId: "previews1")
                XCTAssertEqual(page.items.count, 1)
                XCTAssertNotNil(page.items[0].uploadOperations)
            default:
                let image = try await sdk.libraryImage(libraryImageId: "image1")
                XCTAssertEqual(image.id, "image1")
            }
            let remaining = await mock.remaining()
            XCTAssertEqual(remaining, 0, name)
        }
    }

    func testCancellationAndClose() async throws {
        let started = expectation(description: "Custom HTTP started")
        let sdk = try Client(issuerID: "issuer", keyID: "key", privateKey: P256.Signing.PrivateKey().pemRepresentation) { request in
            started.fulfill()
            try await Task.sleep(nanoseconds: 30_000_000_000)
            throw URLError(.timedOut)
        }
        let task = Task { try await sdk.apps() }
        await fulfillment(of: [started], timeout: 2)
        task.cancel()
        do { _ = try await task.value; XCTFail("Expected cancelled read") }
        catch let error as ApiError { XCTAssertEqual(error.code, "operation_cancelled") }
        await sdk.close()
        await sdk.close()
        do { _ = try await sdk.apps(); XCTFail("Expected closed client") }
        catch let error as ApiError { XCTAssertEqual(error.code, "client_closed") }
    }

    func testCloseCancelsAnOutstandingRead() async throws {
        let started = expectation(description: "Read started")
        let sdk = try Client(issuerID: "issuer", keyID: "key", privateKey: P256.Signing.PrivateKey().pemRepresentation) { _ in
            started.fulfill()
            try await Task.sleep(nanoseconds: 30_000_000_000)
            throw URLError(.timedOut)
        }
        let task = Task { try await sdk.apps() }
        await fulfillment(of: [started], timeout: 2)
        await sdk.close()
        do { _ = try await task.value; XCTFail("Expected cancelled read") }
        catch let error as ApiError { XCTAssertEqual(error.code, "operation_cancelled") }
    }

    func testMutationCancellationReportsUncertainOutcome() async throws {
        let url = Bundle.module.url(forResource: "scenarios", withExtension: "json")!
        let scenarios = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [[String: Any]]
        let scenario = scenarios.first { $0["name"] as? String == "create_version_posts_once_status_201" }!
        let mock = CoreMock(Array((scenario["steps"] as! [[String: Any]]).prefix(1)))
        let started = expectation(description: "Mutation dispatched")
        let sdk = try Client(issuerID: "issuer", keyID: "key", privateKey: P256.Signing.PrivateKey().pemRepresentation) { request in
            if request.httpMethod == "POST" {
                started.fulfill()
                try await Task.sleep(nanoseconds: 30_000_000_000)
                throw URLError(.timedOut)
            }
            return try await mock.send(request)
        }
        let task = Task { try await sdk.createVersion(appId: "app1", platform: "IOS", versionString: "2.0") }
        await fulfillment(of: [started], timeout: 2)
        task.cancel()
        do { _ = try await task.value; XCTFail("Expected uncertain mutation") }
        catch let error as ApiError {
            XCTAssertEqual(error.code, "write_outcome_unknown")
            XCTAssertEqual(error.status, 503)
            XCTAssertFalse(error.retryable)
        }
    }

    func testCancelledAndExpiredCallbacksNeverDispatch() async throws {
        let cancelled = OperationCancellation(0)
        cancelled.cancel()
        let request = URLRequest(url: URL(string: "https://api.appstoreconnect.apple.com/v1/apps")!, timeoutInterval: 1)
        do {
            _ = try await invokeTransport({ _ in XCTFail("Cancelled HTTP dispatched"); throw URLError(.cancelled) }, request,
                cancellation: cancelled, deadline: Date().timeIntervalSince1970 + 1)
            XCTFail("Expected cancellation")
        } catch is CallbackAborted { }
        do {
            _ = try await invokeTransport({ _ in XCTFail("Expired HTTP dispatched"); throw URLError(.cancelled) }, request,
                cancellation: OperationCancellation(0), deadline: Date().timeIntervalSince1970 - 1)
            XCTFail("Expected timeout")
        } catch let error as URLError { XCTAssertEqual(error.code, .timedOut) }
        do {
            try await invokePersistence({ _ in XCTFail("Cancelled persistence dispatched") },
                snapshot: ListingImageUploadCheckpoint(), cancellation: cancelled)
            XCTFail("Expected cancellation")
        } catch is CallbackAborted { }
    }

    func testCancellationAndCloseSettleWhilePersistenceIsSuspended() async throws {
        let fixture = try loadFixtureObject("uploads")
        let uploadCase = (fixture["cases"] as! [[String: Any]]).first { $0["name"] as? String == "persistence_failure" }!
        for useClose in [false, true] {
            let mock = CoreMock(uploadCase["steps"] as! [[String: Any]])
            let sdk = try client(mock)
            let gate = CallbackGate()
            let started = expectation(description: "Checkpoint persistence started")
            let checkpoint = ListingImageUploadCheckpoint()
            let png = Data(base64Encoded: fixture["png"] as! String)!
            let task = Task {
                try await sdk.uploadListingImage(imageRequest, png: png, checkpoint: checkpoint) { _ in
                    started.fulfill()
                    await gate.wait()
                }
            }
            await fulfillment(of: [started], timeout: 2)
            if useClose { await sdk.close() } else { task.cancel() }
            do { _ = try await task.value; XCTFail("Expected uncertain cancelled upload") }
            catch let error as ApiError { XCTAssertEqual(error.code, "write_outcome_unknown") }
            subset(try encoded(checkpoint), .object(["phase": .string("reservation_started")]), "cancelled checkpoint")
            let calls = await mock.requests()
            XCTAssertTrue(calls.allSatisfy { $0.httpMethod == "GET" })
            // A late durable save cannot race another operation using this state.
            let other = try client(CoreMock([]))
            do { _ = try await other.uploadListingImage(imageRequest, png: png, checkpoint: checkpoint) { _ in }; XCTFail("Checkpoint save is still pending") }
            catch let error as ApiError { XCTAssertEqual(error.code, "checkpoint_in_use") }
            await gate.release()
        }
    }

    func testInvalidPNGDoesNotDispatchAndEmptyCheckpointRoundTrips() async throws {
        let mock = CoreMock([])
        let sdk = try client(mock)
        let checkpoint = ListingImageUploadCheckpoint()
        do { _ = try await sdk.uploadListingImage(imageRequest, png: Data("invalid".utf8), checkpoint: checkpoint) { _ in }; XCTFail("Expected invalid PNG") }
        catch let error as ApiError { XCTAssertEqual(error.code, "invalid_png") }
        XCTAssertEqual(try encoded(checkpoint), .object([:]))
        let restored = try JSONDecoder().decode(ListingImageUploadCheckpoint.self, from: JSONEncoder().encode(checkpoint))
        XCTAssertEqual(try encoded(restored), .object([:]))
        let calls = await mock.requests()
        XCTAssertTrue(calls.isEmpty)
    }
}
private enum PersistenceFailure: Error { case diskFull }
private actor CallbackGate {
    private var open = false
    private var continuation: CheckedContinuation<Void, Never>?
    func wait() async {
        if open { return }
        await withCheckedContinuation { continuation = $0 }
    }
    func release() { open = true; continuation?.resume(); continuation = nil }
}
private actor SnapshotStore {
    var saved: [JSONValue] = []
    func save(_ checkpoint: ListingImageUploadCheckpoint) throws { saved.append(try encoded(checkpoint)) }
    func values() -> [JSONValue] { saved }
}
