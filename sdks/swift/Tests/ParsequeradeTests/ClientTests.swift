import XCTest
import Foundation
import CryptoKit
@testable import Parsequerade

private let key = P256.Signing.PrivateKey()
private let pem = key.pemRepresentation

private func unbase64URL(_ text: String) -> Data? {
    let value = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
    return Data(base64Encoded: value + String(repeating: "=", count: (4 - value.count % 4) % 4))
}

private struct Fixture: Decodable {
    let handler: String
    let collection: Bool
    let path: String
    let item: JSONValue
    let responses: [JSONValue]
}
private func fixtures() throws -> [Fixture] {
    let url = Bundle.module.url(forResource: "reads", withExtension: "json")!
    return try JSONDecoder().decode([Fixture].self, from: Data(contentsOf: url))
}
private struct Step: Sendable {
    var data: Data
    var status = 200
    var headers = ["Content-Type": "application/json"]
    var failure = false
    init(_ payload: Any, status: Int = 200, headers: [String: String] = [:], failure: Bool = false) {
        data = try! JSONSerialization.data(withJSONObject: payload)
        self.status = status
        self.headers.merge(headers) { _, v in v }
        self.failure = failure
    }
    init(_ json: JSONValue) { data = try! JSONEncoder().encode(json) }
}
private func envelope(_ data: Any, next: Any = NSNull()) -> [String: Any] { ["data": data, "links": ["next": next]] }
private func item(_ type: String = "apps", _ id: String = "123", _ attrs: [String: Any] = [:]) -> [String: Any] {
    ["type": type, "id": id, "attributes": attrs]
}
private actor Mock {
    var steps: [Step]
    var calls: [URLRequest] = []
    init(_ steps: [Step]) { self.steps = steps }
    func send(_ request: URLRequest) throws -> (Data, HTTPURLResponse) {
        XCTAssertEqual(request.url?.host, "api.appstoreconnect.apple.com")
        XCTAssertLessThanOrEqual(request.timeoutInterval, 8)
        let jwt = request.value(forHTTPHeaderField: "Authorization")!.dropFirst(7).components(separatedBy: ".")
        let header = try JSONSerialization.jsonObject(with: unbase64URL(jwt[0])!) as! [String: Any]
        let claims = try JSONSerialization.jsonObject(with: unbase64URL(jwt[1])!) as! [String: Any]
        XCTAssertEqual(header["alg"] as? String, "ES256")
        XCTAssertEqual(claims["iss"] as? String, "issuer")
        XCTAssertEqual(claims["aud"] as? String, "appstoreconnect-v1")
        XCTAssertGreaterThan(claims["exp"] as! Double, Date().timeIntervalSince1970)
        XCTAssertTrue(key.publicKey.isValidSignature(try P256.Signing.ECDSASignature(rawRepresentation: unbase64URL(jwt[2])!), for: Data((jwt[0] + "." + jwt[1]).utf8)))
        calls.append(request)
        guard !steps.isEmpty else { XCTFail("Unexpected request"); throw URLError(.badServerResponse) }
        let step = steps.removeFirst()
        if step.failure { throw URLError(.networkConnectionLost) }
        return (step.data, HTTPURLResponse(url: request.url!, statusCode: step.status, httpVersion: "HTTP/1.1", headerFields: step.headers)!)
    }
    func requests() -> [URLRequest] { calls }
}
private func makeClient(_ steps: [Step]) throws -> (Client, Mock) {
    let mock = Mock(steps)
    return (try Client(issuerID: "issuer", keyID: "key", privateKey: pem, transport: { try await mock.send($0) }), mock)
}
private func query(_ request: URLRequest) -> Query {
    Dictionary(uniqueKeysWithValues: URLComponents(url: request.url!, resolvingAgainstBaseURL: false)!.queryItems!.map { ($0.name, $0.value!) })
}

final class ClientTests: XCTestCase {
    @discardableResult
    private func expectError(_ code: String, _ operation: () async throws -> Void, file: StaticString = #filePath, line: UInt = #line) async -> ApiError? {
        do { try await operation(); XCTFail("Expected \(code)", file: file, line: line) }
        catch let error as ApiError { XCTAssertEqual(error.code, code, file: file, line: line); return error }
        catch { XCTFail("Unexpected \(error)", file: file, line: line) }
        return nil
    }
    private func typedID(_ client: Client, _ handler: String) async throws -> String {
        switch handler {
        case "apps": return try await client.apps().items[0].id
        case "app": return try await client.app(appId: "123").id
        case "appInfos": return try await client.appInfos(appId: "123").items[0].id
        case "appInfo": return try await client.appInfo(appId: "123", appInfoId: "info_1").id
        case "appInfoLocalizations": return try await client.appInfoLocalizations(appId: "123", appInfoId: "info_1").items[0].id
        case "appInfoLocalization": return try await client.appInfoLocalization(appId: "123", appInfoId: "info_1", locale: "en-US").id
        case "ageRating": return try await client.ageRating(appId: "123", appInfoId: "info_1").id
        case "versions": return try await client.versions(appId: "123").items[0].id
        case "version": return try await client.version(appId: "123", versionId: "version_1").id
        case "versionLocalizations": return try await client.versionLocalizations(appId: "123", versionId: "version_1").items[0].id
        case "versionLocalization": return try await client.versionLocalization(appId: "123", versionId: "version_1", locale: "en-US").id
        case "reviewContact": return try await client.reviewContact(appId: "123", versionId: "version_1").id
        default: throw ApiError(404, "resource_not_found", "Unexpected fixture handler.")
        }
    }

    func testAllTwelveTypedMethodsUseWorkerFixtures() async throws {
        for fixture in try fixtures() {
            let (client, mock) = try makeClient(fixture.responses.map(Step.init))
            let initial = await mock.requests()
            XCTAssertTrue(initial.isEmpty)
            let id = try await typedID(client, fixture.handler)
            if case .object(let item) = fixture.item, case .string(let expected) = item["id"] { XCTAssertEqual(id, expected, fixture.handler) }
            else { XCTFail("Malformed fixture") }
            let calls = await mock.requests()
            XCTAssertEqual(calls.count, fixture.responses.count, fixture.handler)
            XCTAssertTrue(calls.allSatisfy { $0.httpMethod == "GET" })
            let (rawClient, _) = try makeClient(fixture.responses.map(Step.init))
            let raw: JSONValue = try await rawClient.get(fixture.path)
            let bytes = try JSONEncoder().encode(raw)
            let json = try JSONSerialization.jsonObject(with: bytes) as! [String: Any]
            let flat = fixture.collection ? (json["items"] as! [[String: Any]])[0] : json
            for excluded in ["attributes", "relationships", "notes", "demoAccountPassword"] { XCTAssertNil(flat[excluded]) }
            if fixture.handler == "ageRating" {
                XCTAssertEqual(flat["gambling"] as? Bool, false)
                XCTAssertEqual(flat["ageRatingOverrideV2"] as? String, "FUTURE_READ_VALUE")
            }
        }
    }
    func testQueryTranslationProjectionAndNulls() async throws {
        let (client, mock) = try makeClient([Step(envelope([item("apps", "123", ["name": "Notes", "isOrEverWasMadeForKids": false, "subscriptionStatusUrl": NSNull()])]))])
        let result: JSONValue = try await client.get("/apps", query: ["bundleId": "one,two", "fields": "isOrEverWasMadeForKids,subscriptionStatusUrl", "limit": "2"])
        guard case .object(let page) = result, case .array(let items) = page["items"], case .object(let app) = items[0] else { return XCTFail("Missing page") }
        XCTAssertEqual(app["isOrEverWasMadeForKids"], .bool(false))
        XCTAssertEqual(app["subscriptionStatusUrl"], .null)
        XCTAssertNil(app["name"])
        let calls = await mock.requests()
        XCTAssertEqual(query(calls[0])["filter[bundleId]"], "one,two")
        XCTAssertEqual(query(calls[0])["limit"], "2")
    }
    func testInvalidQueriesAndPathSegmentsDoNotDispatch() async throws {
        let (client, mock) = try makeClient([])
        for query in [["limit": "0"], ["limit": "01"], ["fields": "secret"], ["other": "yes"], ["limit": "2\n"]] {
            await expectError("invalid_query") { _ = try await client.apps(query: query) }
        }
        for id in ["a/b", "../123", "", "x?y", "x\n", "é"] {
            await expectError("invalid_path") { _ = try await client.app(appId: id) }
        }
        await expectError("route_not_found") { let _: JSONValue = try await client.get("https://evil.example/apps") }
        let calls = await mock.requests()
        XCTAssertTrue(calls.isEmpty)
    }
    func testCursorsBindQueryAndClientAndPreserveFilters() async throws {
        let first = Step(envelope([item()], next: "/v1/apps?cursor=page2"))
        let (client, mock) = try makeClient([first, Step(envelope([item("apps", "456")]))])
        let page = try await client.apps(query: ["bundleId": "notes"])
        let cursor = try XCTUnwrap(page.nextCursor)
        let (other, _) = try makeClient([])
        await expectError("invalid_cursor") { _ = try await other.apps(query: ["bundleId": "notes", "cursor": cursor]) }
        await expectError("invalid_cursor") { _ = try await client.apps(query: ["cursor": cursor]) }
        await expectError("invalid_cursor") { _ = try await client.apps(query: ["bundleId": "notes", "cursor": cursor + "x"]) }
        let second = try await client.apps(query: ["bundleId": "notes", "cursor": cursor])
        XCTAssertNil(second.nextCursor)
        let calls = await mock.requests()
        XCTAssertEqual(query(calls.last!)["filter[bundleId]"], "notes")
        let (loop, _) = try makeClient([first, first])
        let one = try await loop.apps()
        await expectError("upstream_incomplete_result") { _ = try await loop.apps(query: ["cursor": one.nextCursor!]) }
    }
    func testUntrustedContinuationsAreRejected() async throws {
        for link in ["https://evil.example/v1/apps?cursor=x", "/v1/other?cursor=x", "/v1/apps?cursor=x&cursor=y", "/v1/apps?cursor=x&limit=2", "/v1/apps?cursor=x#fragment", "/v1/apps"] {
            let (client, mock) = try makeClient([Step(envelope([], next: link))])
            await expectError("upstream_invalid_response") { _ = try await client.apps() }
            let calls = await mock.requests()
            XCTAssertEqual(calls.count, 1)
        }
    }
    func testOwnershipAndLocaleUniqueness() async throws {
        let (client, mock) = try makeClient([Step(envelope([]))])
        await expectError("resource_not_found") { _ = try await client.version(appId: "123", versionId: "foreign") }
        let calls = await mock.requests()
        XCTAssertEqual(calls.count, 1)
        let parent = Step(envelope([item("appStoreVersions", "version_1")]))
        let loc = item("appStoreVersionLocalizations", "locale_1", ["locale": "en-US"])
        let duplicate = item("appStoreVersionLocalizations", "locale_2", ["locale": "en-US"])
        let (ambiguous, _) = try makeClient([parent, Step(envelope([loc, duplicate]))])
        await expectError("ambiguous_selection") { _ = try await ambiguous.versionLocalization(appId: "123", versionId: "version_1", locale: "en-US") }
        let (changed, _) = try makeClient([parent, Step(envelope([loc])), Step(envelope(item("appStoreVersionLocalizations", "locale_1", ["locale": "fr-FR"])))])
        await expectError("upstream_invalid_response") { _ = try await changed.versionLocalization(appId: "123", versionId: "version_1", locale: "en-US") }
    }
    func testReadRetriesAuthRefreshAndSanitizedErrors() async throws {
        let (client, mock) = try makeClient([Step([:], status: 401), Step([:], status: 503), Step(envelope([]))])
        let result = try await client.apps()
        XCTAssertTrue(result.items.isEmpty)
        let calls = await mock.requests()
        XCTAssertNotEqual(calls[0].value(forHTTPHeaderField: "Authorization"), calls[1].value(forHTTPHeaderField: "Authorization"))
        XCTAssertEqual(calls[1].value(forHTTPHeaderField: "Authorization"), calls[2].value(forHTTPHeaderField: "Authorization"))
        let (invalid, _) = try makeClient([Step(["errors": [["code": "BAD.PARAM", "detail": "secret", "source": ["parameter": "limit"]]]], status: 422)])
        let error = await expectError("validation_failed") { _ = try await invalid.apps() }
        XCTAssertEqual(error?.details.first?.field, "limit")
        XCTAssertFalse(error?.message.contains("secret") ?? true)
        let (throttled, throttleMock) = try makeClient([Step([:], status: 429, headers: ["Retry-After": "10"])])
        let throttle = await expectError("rate_limited") { _ = try await throttled.apps() }
        XCTAssertEqual(throttle?.retryAfter, 10)
        let throttleCalls = await throttleMock.requests()
        XCTAssertEqual(throttleCalls.count, 1)
    }
    func testWritesValidateAndAreNeverRetried() async throws {
        for attrs: AgeRatingUpdate in [[:], ["unknown": .bool(true)], ["gambling": .null], ["gambling": .number(0)], ["kidsAgeBand": .string("bad")], ["developerAgeRatingInfoUrl": .string("https://user:pass@example.com")]] {
            let (client, mock) = try makeClient([])
            await expectError("invalid_request") { _ = try await client.updateAgeRating(appId: "123", appInfoId: "info_1", attributes: attrs) }
            let calls = await mock.requests(); XCTAssertTrue(calls.isEmpty)
        }
        let fixture = try fixtures().first { $0.handler == "ageRating" }!
        let setup = fixture.responses.map(Step.init)
        let (client, mock) = try makeClient(setup + [Step(fixture.responses.last!)])
        let result = try await client.updateAgeRating(appId: "123", appInfoId: "info_1", attributes: ["gambling": .bool(false), "kidsAgeBand": .null])
        XCTAssertEqual(result.gambling, false)
        let calls = await mock.requests()
        XCTAssertEqual(calls.last!.httpMethod, "PATCH")
        let body = try JSONSerialization.jsonObject(with: calls.last!.httpBody!) as! [String: Any]
        let attributes = (body["data"] as! [String: Any])["attributes"] as! [String: Any]
        XCTAssertTrue(attributes["kidsAgeBand"] is NSNull)
        for (step, code) in [(Step([:], failure: true), "write_outcome_unknown"), (Step([:], headers: ["Content-Type": "text/html"]), "write_outcome_unknown"), (Step([:], status: 401), "upstream_authentication_failed"), (Step(envelope(item("ageRatingDeclarations", "wrong"))), "write_outcome_unknown")] {
            let (failed, failedMock) = try makeClient(setup + [step])
            let error = await expectError(code) { _ = try await failed.updateAgeRating(appId: "123", appInfoId: "info_1", attributes: ["gambling": .bool(false)]) }
            XCTAssertEqual(error?.retryable, false)
            let failedCalls = await failedMock.requests(); XCTAssertEqual(failedCalls.count, 3)
        }
    }
    func testResponseValidationAndBodyLimits() async throws {
        var large = Step([:]); large.data = Data(repeating: 120, count: 4 * 1024 * 1024 + 1)
        var malformed = Step([:]); malformed.data = Data("invalid".utf8)
        for step in [large, malformed, Step([:], status: 302), Step(envelope([item(), item()])), Step(envelope([item("apps", "123", ["name": 42])]))] {
            let (client, _) = try makeClient([step])
            await expectError("upstream_invalid_response") { _ = try await client.apps() }
        }
    }
    func testResolverBound() async throws {
        let (client, mock) = try makeClient((0..<20).map { Step(envelope([], next: "/v1/apps/123/appInfos?cursor=\($0)")) })
        await expectError("upstream_incomplete_result") { _ = try await client.appInfo(appId: "123", appInfoId: "info_1") }
        let calls = await mock.requests()
        XCTAssertEqual(calls.count, 20)
    }
    func testInvalidCredentialsAreLocal() throws {
        for invalid in ["invalid", P384.Signing.PrivateKey().pemRepresentation] {
            XCTAssertThrowsError(try Client(issuerID: "issuer", keyID: "key", privateKey: invalid)) { error in
                XCTAssertEqual((error as? ApiError)?.code, "invalid_credentials")
            }
        }
    }
}
