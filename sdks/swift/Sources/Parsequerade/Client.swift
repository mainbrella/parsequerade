import Foundation

func pageData(_ payload: [String: Any], _ kind: String, _ limit: Int) throws -> [[String: Any]] {
    guard let items = payload["data"] as? [Any], items.count <= limit, payload["links"] is [String: Any] else { throw ApiError.invalidUpstream() }
    var seen = Set<String>()
    return try items.map { raw in
        let item = try resource(raw, kind)
        guard seen.insert(item["id"] as! String).inserted else { throw ApiError.invalidUpstream() }
        return item
    }
}

/// A local facade using an Apple team key. Reuse this actor to share JWTs and cursor signing.
public actor Client {
    var auth: Auth
    let transport: AppleTransport
    let cursors = Cursors()
    var activeRequests = 0

    /// Validates a P-256 PKCS#8 PEM locally without a network request.
    public init(issuerID: String, keyID: String, privateKey: String, transport: AppleTransport? = nil) throws {
        auth = try Auth(issuerID: issuerID, keyID: keyID, pem: privateKey)
        self.transport = transport ?? { try await DefaultTransport.send($0) }
    }

    /// Read a supported facade path. Use JSONValue for exact JSON, including explicit nulls.
    public func get<T: Decodable & Sendable>(_ path: String, query: Query = [:], as type: T.Type = T.self) async throws -> T {
        var budget = Budget()
        let result = try await read(path, query, &budget)
        do { return try JSONDecoder().decode(type, from: JSONSerialization.data(withJSONObject: result)) }
        catch { throw ApiError(500, "response_type_mismatch", "The requested Swift type does not match the facade response.") }
    }

    func traverse(_ path: String, _ query: Query, _ route: Route, _ budget: inout Budget,
                  visit: ([[String: Any]]) throws -> Bool) async throws {
        let initial = try appleURL(path, query)
        var url = initial
        var seen = Set<String>()
        for _ in 0..<20 {
            guard seen.insert(digest(url.absoluteString)).inserted else { throw ApiError.incomplete() }
            let payload = try await apple(url, route: route, budget: &budget)
            if try visit(pageData(payload, route.type, 200)) { return }
            guard let next = try nextPage(payload, initial) else { return }
            url = next
        }
        throw ApiError.incomplete()
    }

    func read(_ path: String, _ query: Query, _ budget: inout Budget) async throws -> [String: Any] {
        let (route, p) = try matchRoute(path)
        var query = try parseQuery(route, query)
        let handler = route.handler
        let info = handler.hasPrefix("appInfo") || handler == "ageRating"
        let appID = p["appId"] ?? ""
        let parentID = p[info ? "appInfoId" : "versionId"]
        let parentType = info ? "appInfos" : "appStoreVersions"
        var context: [String: Any] = [:]
        if !["apps", "app"].contains(handler) {
            context["appId"] = appID
            if let parentID, !["appInfo", "version"].contains(handler) { context[info ? "appInfoId" : "versionId"] = parentID }
        }
        if let parentID {
            var found = false
            var params = ["limit": "200"]
            if !info { params["filter[id]"] = parentID }
            try await traverse("/v1/apps/\(appID)/\(parentType)", params, Contracts.shared.route(info ? "appInfos" : "versions"), &budget) { items in
                for item in items { try checkParent(item, "app", appID, "apps") }
                found = items.contains { $0["id"] as? String == parentID }
                return found
            }
            guard found else { throw ApiError.notFound() }
        }
        let id = parentID ?? ""
        var paths = [
            "apps": "/v1/apps", "app": "/v1/apps/\(appID)",
            "appInfos": "/v1/apps/\(appID)/appInfos", "appInfo": "/v1/appInfos/\(id)",
            "versions": "/v1/apps/\(appID)/appStoreVersions", "version": "/v1/appStoreVersions/\(id)",
            "appInfoLocalizations": "/v1/appInfos/\(id)/appInfoLocalizations",
            "versionLocalizations": "/v1/appStoreVersions/\(id)/appStoreVersionLocalizations",
            "ageRating": "/v1/appInfos/\(id)/ageRatingDeclaration",
            "reviewContact": "/v1/appStoreVersions/\(id)/appStoreReviewDetail",
        ]
        var selected: String?
        if let locale = p["locale"] {
            let localizations = info ? "appInfoLocalizations" : "appStoreVersionLocalizations"
            let params = ["limit": "200", "filter[locale]": locale, "fields[\(localizations)]": "locale"]
            try await traverse("/v1/\(parentType)/\(id)/\(localizations)", params,
                               Contracts.shared.route(info ? "appInfoLocalizations" : "versionLocalizations"), &budget) { items in
                for item in items {
                    try checkParent(item, info ? "appInfo" : "appStoreVersion", id, parentType)
                    guard let itemLocale = (item["attributes"] as? [String: Any])?["locale"] as? String else { throw ApiError.invalidUpstream() }
                    if itemLocale == locale {
                        guard selected == nil else { throw ApiError(409, "ambiguous_selection", "More than one localization matches this locale.") }
                        selected = item["id"] as? String
                    }
                }
                return false
            }
            guard let selected else { throw ApiError.notFound("localization_not_found") }
            paths[handler] = "/v1/\(localizations)/\(selected)"
            if let fields = query.fields, !fields.contains("locale") { query.upstream["fields[\(localizations)]"] = (fields + ["locale"]).joined(separator: ",") }
        }
        if ["appInfos", "appInfo"].contains(handler) {
            query.upstream["include"] = "primaryCategory,secondaryCategory"
            if let fields = query.fields { query.upstream["fields[appInfos]"] = (fields + ["primaryCategory", "secondaryCategory"]).joined(separator: ",") }
        }
        let initial = try appleURL(paths[handler]!, query.upstream)
        if !route.collection {
            let payload = try await apple(initial, route: route, budget: &budget)
            guard let raw = payload["data"], !(raw is NSNull) else { throw ApiError.notFound(handler == "reviewContact" ? "review_contact_not_found" : "resource_not_found") }
            let data = try resource(raw, route.type)
            let expected = handler == "app" ? appID : (["appInfo", "version"].contains(handler) ? parentID : selected)
            if let expected, data["id"] as? String != expected { throw ApiError.invalidUpstream() }
            if ["appInfo", "version"].contains(handler) { try checkParent(data, "app", appID, "apps") }
            if let locale = p["locale"] {
                guard (data["attributes"] as? [String: Any])?["locale"] as? String == locale else { throw ApiError.invalidUpstream() }
                try checkParent(data, info ? "appInfo" : "appStoreVersion", id, parentType)
            }
            if handler == "reviewContact" { try checkParent(data, "appStoreVersion", id, parentType) }
            return try flatten(data, route, context, query.fields)
        }
        let binding = digest(path + "\n" + initial.absoluteString)
        let prior = try query.cursor.map { try cursors.decode($0, binding) }
        let expires = prior?.expires ?? (Date().timeIntervalSince1970 + 900)
        var seen = prior?.seen ?? []
        var url = initial
        if let prior {
            var params = query.upstream; params["cursor"] = prior.next
            url = try appleURL(paths[handler]!, params)
        }
        let pageHash = digest(url.absoluteString)
        guard seen.count < 100, !seen.contains(pageHash) else { throw ApiError.incomplete() }
        seen.append(pageHash)
        let payload = try await apple(url, route: route, budget: &budget)
        let items = try pageData(payload, route.type, query.limit).map { item -> [String: Any] in
            if ["appInfos", "versions"].contains(handler) { try checkParent(item, "app", appID, "apps") }
            if ["appInfoLocalizations", "versionLocalizations"].contains(handler) {
                try checkParent(item, info ? "appInfo" : "appStoreVersion", id, parentType)
            }
            return try flatten(item, route, context, query.fields)
        }
        var next: Any = NSNull()
        if let nextURL = try nextPage(payload, initial) {
            guard !seen.contains(digest(nextURL.absoluteString)),
                  let cursor = URLComponents(url: nextURL, resolvingAgainstBaseURL: false)?.queryItems?.first(where: { $0.name == "cursor" })?.value else { throw ApiError.incomplete() }
            next = try cursors.encode(Cursor(binding: binding, next: cursor, seen: seen, expires: expires))
        }
        return ["items": items, "nextCursor": next]
    }

    /// Resolve ownership and send exactly one PATCH. Read back after write_outcome_unknown.
    public func updateAgeRating(appId: String, appInfoId: String, attributes: AgeRatingUpdate) async throws -> AgeRating {
        try validateSegment(appId)
        try validateSegment(appInfoId)
        let attributes = try validateUpdate(attributes)
        var budget = Budget()
        let current = try await read("/apps/\(appId)/app-infos/\(appInfoId)/age-rating", [:], &budget)
        let id = current["id"] as! String
        let route = Contracts.shared.route("ageRating")
        let body: [String: Any] = ["data": ["type": "ageRatingDeclarations", "id": id, "attributes": attributes]]
        let payload = try await apple(appleURL("/v1/ageRatingDeclarations/\(id)", [:]), route: route, budget: &budget, body: body)
        do {
            guard let data = payload["data"] as? [String: Any], data["id"] as? String == id else { throw ApiError.invalidUpstream() }
            let flat = try flatten(data, route, ["appId": appId, "appInfoId": appInfoId])
            return try JSONDecoder().decode(AgeRating.self, from: JSONSerialization.data(withJSONObject: flat))
        } catch { throw ApiError.unknownWrite() }
    }
}
