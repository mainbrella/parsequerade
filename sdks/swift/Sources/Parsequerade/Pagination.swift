import Foundation
import CryptoKit

let appleOrigin = "https://api.appstoreconnect.apple.com"
func base64URL(_ data: Data) -> String {
    data.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
}
func unbase64URL(_ text: String) -> Data? {
    let value = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
    return Data(base64Encoded: value + String(repeating: "=", count: (4 - value.count % 4) % 4))
}
func digest(_ text: String) -> String { base64URL(Data(SHA256.hash(data: Data(text.utf8)).prefix(16))) }
func appleURL(_ path: String, _ query: Query) throws -> URL {
    guard var components = URLComponents(string: appleOrigin + path) else { throw ApiError.invalidUpstream() }
    if !query.isEmpty { components.queryItems = query.sorted { $0.key < $1.key }.map { URLQueryItem(name: $0.key, value: $0.value) } }
    guard let url = components.url else { throw ApiError.invalidUpstream() }
    return url
}
func nextPage(_ payload: [String: Any], _ initial: URL) throws -> URL? {
    guard let raw = (payload["links"] as? [String: Any])?["next"], !(raw is NSNull) else { return nil }
    guard let link = raw as? String, link.utf8.count <= 8192,
          let url = URL(string: link, relativeTo: initial)?.absoluteURL,
          let components = URLComponents(url: url, resolvingAgainstBaseURL: true),
          url.scheme == initial.scheme, url.host == initial.host, url.port == initial.port,
          url.path == initial.path, url.user == nil, url.password == nil, url.fragment == nil else { throw ApiError.invalidUpstream() }
    let originalItems = URLComponents(url: initial, resolvingAgainstBaseURL: false)?.queryItems ?? []
    var original = Dictionary(uniqueKeysWithValues: originalItems.map { ($0.name, $0.value ?? "") })
    var seen = Set<String>()
    var cursor: String?
    for item in components.queryItems ?? [] {
        guard seen.insert(item.name).inserted else { throw ApiError.invalidUpstream() }
        if item.name == "cursor" { cursor = item.value }
        else if original[item.name] != item.value { throw ApiError.invalidUpstream() }
    }
    guard let cursor, !cursor.isEmpty, cursor.utf8.count <= 2048 else { throw ApiError.invalidUpstream() }
    original["cursor"] = cursor
    return try appleURL(initial.path, original)
}
struct Cursor: Codable {
    let binding: String
    let next: String
    let seen: [String]
    let expires: TimeInterval
}
struct Cursors {
    let key = SymmetricKey(size: .bits256)
    func encode(_ value: Cursor) throws -> String {
        let payload = base64URL(try JSONEncoder().encode(value))
        let signature = HMAC<SHA256>.authenticationCode(for: Data(payload.utf8), using: key)
        let token = payload + "." + base64URL(Data(signature))
        guard token.utf8.count <= 8192 else { throw ApiError.invalidUpstream() }
        return token
    }
    func decode(_ token: String, _ binding: String) throws -> Cursor {
        let invalid = ApiError(400, "invalid_cursor", "The cursor is invalid, expired, or belongs to another client or query.")
        let parts = token.components(separatedBy: ".")
        guard token.utf8.count <= 8192, parts.count == 2,
              let signature = unbase64URL(parts[1]), let data = unbase64URL(parts[0]),
              HMAC<SHA256>.isValidAuthenticationCode(signature, authenticating: Data(parts[0].utf8), using: key),
              let cursor = try? JSONDecoder().decode(Cursor.self, from: data),
              cursor.binding == binding, cursor.expires > Date().timeIntervalSince1970,
              cursor.expires <= Date().timeIntervalSince1970 + 900,
              !cursor.next.isEmpty, cursor.next.utf8.count <= 2048,
              !cursor.seen.isEmpty, cursor.seen.count <= 100, Set(cursor.seen).count == cursor.seen.count else { throw invalid }
        return cursor
    }
}
