import Foundation
import Parsequerade

@main
struct ListApps {
    static func main() async throws {
        let env = ProcessInfo.processInfo.environment
        guard let issuer = env["ASC_ISSUER_ID"], let keyID = env["ASC_KEY_ID"], let path = env["ASC_PRIVATE_KEY_PATH"] else {
            throw NSError(domain: "ListApps", code: 1, userInfo: [NSLocalizedDescriptionKey: "Set ASC_ISSUER_ID, ASC_KEY_ID and ASC_PRIVATE_KEY_PATH."])
        }
        let client = try Client(issuerID: issuer, keyID: keyID, privateKey: String(contentsOfFile: path, encoding: .utf8))
        var query: Query = ["limit": "50"]
        repeat {
            let page = try await client.apps(query: query)
            for app in page.items { print(app.id, app.name ?? "(unnamed)") }
            query["cursor"] = page.nextCursor
        } while query["cursor"] != nil
    }
}
