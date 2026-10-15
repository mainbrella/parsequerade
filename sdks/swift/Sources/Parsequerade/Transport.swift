import Foundation
import CryptoKit

/// A trusted custom transport receives signed Apple requests and must honor their timeout.
public typealias AppleTransport = @Sendable (URLRequest) async throws -> (Data, HTTPURLResponse)

final class NoRedirect: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        completionHandler(nil)
    }
}
struct DefaultTransport {
    static let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 8
        config.timeoutIntervalForResource = 8
        config.urlCache = nil
        config.httpCookieStorage = nil
        return URLSession(configuration: config, delegate: NoRedirect(), delegateQueue: nil)
    }()
    static func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        try await withThrowingTaskGroup(of: (Data, HTTPURLResponse).self) { group in
            group.addTask { try await stream(request) }
            group.addTask {
                try await Task.sleep(nanoseconds: UInt64(request.timeoutInterval * 1_000_000_000))
                throw URLError(.timedOut)
            }
            defer { group.cancelAll() }
            return try await group.next()!
        }
    }
    private static func stream(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let (bytes, raw) = try await session.bytes(for: request)
        guard let response = raw as? HTTPURLResponse, response.expectedContentLength <= 4 * 1024 * 1024 else {
            bytes.task.cancel()
            throw ApiError.invalidUpstream()
        }
        return try await withTaskCancellationHandler {
            var data = Data()
            do {
                for try await byte in bytes {
                    data.append(byte)
                    if data.count > 4 * 1024 * 1024 { throw ApiError.invalidUpstream() }
                }
            } catch {
                bytes.task.cancel()
                throw error
            }
            return (data, response)
        } onCancel: {
            bytes.task.cancel()
        }
    }
}
struct Budget {
    var deadline = ProcessInfo.processInfo.systemUptime + 25
    var attempts = 0
    var bytes = 0
}
struct Auth {
    let issuerID: String
    let keyID: String
    let key: P256.Signing.PrivateKey
    var cached: (String, TimeInterval)?
    init(issuerID: String, keyID: String, pem: String) throws {
        do {
            guard !issuerID.isEmpty, issuerID.utf8.count <= 128, !keyID.isEmpty, keyID.utf8.count <= 128,
                  pem.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix("-----BEGIN PRIVATE KEY-----") else {
                throw ApiError.invalidUpstream()
            }
            key = try P256.Signing.PrivateKey(pemRepresentation: pem)
        } catch {
            throw ApiError(400, "invalid_credentials", "Provide an issuer ID, key ID and a P-256 PKCS#8 private key.")
        }
        self.issuerID = issuerID; self.keyID = keyID
    }
    mutating func token() throws -> String {
        let now = Date().timeIntervalSince1970
        if let cached, cached.1 > now + 60 { return cached.0 }
        let header = base64URL(try JSONSerialization.data(withJSONObject: ["alg": "ES256", "kid": keyID, "typ": "JWT"]))
        let payload = base64URL(try JSONSerialization.data(withJSONObject: ["iss": issuerID, "iat": Int(now) - 30, "exp": Int(now) + 600, "aud": "appstoreconnect-v1"]))
        let content = header + "." + payload
        let signature = try key.signature(for: Data(content.utf8))
        let jwt = content + "." + base64URL(signature.rawRepresentation)
        cached = (jwt, now + 600)
        return jwt
    }
    mutating func invalidate(_ rejected: String) { if cached?.0 == rejected { cached = nil } }
}

func upstreamError(_ status: Int, _ payload: [String: Any], _ response: HTTPURLResponse, _ route: Route) -> ApiError {
    let mapped: (Int, String, String)
    switch status {
    case 401: mapped = (502, "upstream_authentication_failed", "Apple rejected the configured credentials.")
    case 403: mapped = (403, "upstream_forbidden", "The Apple key cannot access this resource.")
    case 404: mapped = (404, "resource_not_found", "The requested resource was not found.")
    case 409: mapped = (409, "upstream_conflict", "Apple reported a resource conflict.")
    case 400, 422: mapped = (422, "validation_failed", "Apple rejected one or more request parameters.")
    case 429: mapped = (429, "rate_limited", "Apple is throttling this connection.")
    default: mapped = (status >= 500 ? 503 : 502, "upstream_unavailable", "Apple could not complete this request.")
    }
    var after: Int?
    if let raw = response.value(forHTTPHeaderField: "Retry-After") {
        if let n = Int(raw), n >= 0 { after = min(n, 86400) }
        else {
            let formatter = DateFormatter()
            formatter.locale = Locale(identifier: "en_US_POSIX")
            formatter.timeZone = TimeZone(secondsFromGMT: 0)
            formatter.dateFormat = "EEE',' dd MMM yyyy HH':'mm':'ss z"
            if let date = formatter.date(from: raw) { after = min(86400, max(0, Int(ceil(date.timeIntervalSinceNow)))) }
        }
    }
    let details = (payload["errors"] as? [[String: Any]] ?? []).prefix(32).map { item -> ErrorDetail in
        let code = item["code"] as? String ?? ""
        let safe = matches(code, "^[A-Za-z0-9_.-]{1,128}$") && !code.hasSuffix("\n") ? code : "upstream_error"
        let parameter = (item["source"] as? [String: Any])?["parameter"] as? String
        let field = route.parameters.first { $0.appleName != nil && $0.appleName == parameter }?.name
        return ErrorDetail(code: safe, message: mapped.2, field: field)
    }
    return ApiError(mapped.0, mapped.1, mapped.2, retryable: status == 429 || status >= 500, details: details, retryAfter: after)
}

extension Client {
    func apple(_ url: URL, route: Route, budget: inout Budget, body: [String: Any]? = nil) async throws -> [String: Any] {
        guard url.scheme == "https", url.host == "api.appstoreconnect.apple.com", url.port == nil,
              url.user == nil, url.password == nil, url.fragment == nil,
              url.path.hasPrefix("/v1/"), matches(url.path, "^/v1/[A-Za-z0-9/_-]+$"), !url.path.hasSuffix("\n") else {
            throw ApiError.invalidUpstream()
        }
        let write = body != nil
        guard activeRequests < 4 else {
            throw ApiError(429, "rate_limited", "This client has too many concurrent Apple requests.", retryable: !write, retryAfter: 1)
        }
        activeRequests += 1
        defer { activeRequests -= 1 }
        var refreshed = false
        for attempt in 0..<(write ? 1 : 3) {
            let remaining = budget.deadline - ProcessInfo.processInfo.systemUptime
            guard budget.attempts < 32, remaining > 0 else {
                throw ApiError(504, "upstream_budget_exceeded", "The bounded Apple operation could not finish.", retryable: !write)
            }
            try Task.checkCancellation()
            budget.attempts += 1
            let jwt = try auth.token()
            var request = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: min(8, remaining))
            request.httpMethod = write ? "PATCH" : "GET"
            request.setValue("Bearer " + jwt, forHTTPHeaderField: "Authorization")
            request.setValue("application/json", forHTTPHeaderField: "Accept")
            request.setValue("parsequerade-swift/0.1.0", forHTTPHeaderField: "User-Agent")
            if let body {
                request.httpBody = try JSONSerialization.data(withJSONObject: body)
                request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            }
            var error: ApiError
            var retryAuth = false
            do {
                let (data, response) = try await transport(request)
                guard ProcessInfo.processInfo.systemUptime < budget.deadline else { throw URLError(.timedOut) }
                budget.bytes += data.count
                guard data.count <= 4 * 1024 * 1024, budget.bytes <= 16 * 1024 * 1024,
                      !(300..<400).contains(response.statusCode) else { throw ApiError.invalidUpstream() }
                let payload = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
                if (200..<300).contains(response.statusCode) {
                    let mime = response.value(forHTTPHeaderField: "Content-Type")?.components(separatedBy: ";").first?.trimmingCharacters(in: .whitespaces).lowercased() ?? ""
                    guard mime == "application/json" || (mime.hasPrefix("application/") && mime.hasSuffix("+json")), payload["data"] != nil else { throw ApiError.invalidUpstream() }
                    return payload
                }
                retryAuth = response.statusCode == 401 && !write && !refreshed
                if retryAuth { auth.invalidate(jwt); refreshed = true }
                error = upstreamError(response.statusCode, payload, response, route)
                if write { error.retryable = false }
            } catch let caught as ApiError {
                error = write && caught.code == "upstream_invalid_response" ? .unknownWrite() : caught
            } catch let caught {
                if !write && (caught is CancellationError || Task.isCancelled) { throw CancellationError() }
                let timedOut = (caught as? URLError)?.code == .timedOut
                error = write ? .unknownWrite() : ApiError(timedOut ? 504 : 503, timedOut ? "upstream_timeout" : "upstream_unavailable", "The Apple read could not complete.", retryable: true)
            }
            if write || attempt == 2 || (!error.retryable && !retryAuth) { throw error }
            let delay = retryAuth ? 0 : (error.retryAfter.map(Double.init) ?? (0.2 * pow(2, Double(attempt))))
            guard delay <= 2, ProcessInfo.processInfo.systemUptime + delay < budget.deadline else { throw error }
            try await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
        }
        throw ApiError.invalidUpstream()
    }
}
