import Foundation
import CoreFoundation

struct Schema: Decodable {
    let type: String
    let nullable: Bool?
    let `enum`: [String]?
    let items: ItemSchema?
    let maximum: Int?
    let format: String?
}
struct ItemSchema: Decodable { let `enum`: [String]? }
struct Relationship: Decodable { let field: String; let type: String }
struct Parameter: Decodable {
    let name: String
    let schema: Schema
    let appleName: String?
    let `default`: Int?
}
struct Route: Decodable {
    let handler: String
    let route: String
    let type: String
    let collection: Bool
    let attributes: [String: Schema]
    let relationships: [String: Relationship]
    let parameters: [Parameter]
}
struct Contracts: Decodable {
    let routes: [Route]
    let ageRatingWrite: [String: Schema]
    static let shared: Contracts = {
        // Both file and schema are generated and verified with the package.
        let url = Bundle.module.url(forResource: "contracts", withExtension: "json")!
        return try! JSONDecoder().decode(Contracts.self, from: Data(contentsOf: url))
    }()
    func route(_ handler: String) -> Route { routes.first { $0.handler == handler }! }
}

func matches(_ value: String, _ pattern: String) -> Bool {
    value.range(of: pattern, options: .regularExpression) != nil
}
func validID(_ value: String) -> Bool { matches(value, "^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$") && !value.hasSuffix("\n") }
func validateSegment(_ value: String, locale: Bool = false) throws {
    guard locale ? (matches(value, "^[A-Za-z0-9][A-Za-z0-9-]{1,34}$") && !value.hasSuffix("\n")) : validID(value) else {
        throw ApiError(400, "invalid_path", "The resource path is invalid.")
    }
}
func matchRoute(_ path: String) throws -> (Route, [String: String]) {
    let parts = path.components(separatedBy: "/")
    for route in Contracts.shared.routes {
        let pattern = route.route.components(separatedBy: "/")
        guard parts.count == pattern.count else { continue }
        var params: [String: String] = [:]
        var matched = true
        for (part, template) in zip(parts, pattern) {
            if template.hasPrefix("{") { params[String(template.dropFirst().dropLast())] = part }
            else if part != template { matched = false; break }
        }
        if matched {
            for (name, value) in params { try validateSegment(value, locale: name == "locale") }
            return (route, params)
        }
    }
    throw ApiError.notFound("route_not_found")
}
struct ParsedQuery {
    var upstream: Query = [:]
    var fields: [String]?
    var cursor: String?
    var limit = 50
}
func parseQuery(_ route: Route, _ query: Query) throws -> ParsedQuery {
    var result = ParsedQuery()
    for (name, raw) in query {
        guard let p = route.parameters.first(where: { $0.name == name }), !raw.isEmpty,
              raw.utf8.count <= (name == "cursor" ? 8192 : 4096) else { throw ApiError.invalidQuery(name) }
        switch p.schema.type {
        case "integer":
            guard matches(raw, "^[1-9][0-9]{0,2}$"), !raw.hasSuffix("\n"), let n = Int(raw), n <= (p.schema.maximum ?? 200) else {
                throw ApiError.invalidQuery(name)
            }
        case "boolean":
            guard ["true", "false"].contains(raw) else { throw ApiError.invalidQuery(name) }
        case "array":
            let parts = raw.components(separatedBy: ",")
            guard parts.count <= 200, parts.allSatisfy({ !$0.isEmpty && $0.utf8.count <= 256 && (p.schema.items?.enum?.contains($0) ?? true) }) else {
                throw ApiError.invalidQuery(name)
            }
        default: break
        }
        if let apple = p.appleName { result.upstream[apple] = raw }
        if name == "fields" { result.fields = raw.components(separatedBy: ",") }
        if name == "cursor" { result.cursor = raw }
        if name == "limit" { result.limit = Int(raw)! }
    }
    for p in route.parameters where query[p.name] == nil {
        if let value = p.default, let apple = p.appleName { result.upstream[apple] = String(value) }
    }
    return result
}
func resource(_ value: Any, _ kind: String) throws -> [String: Any] {
    guard let value = value as? [String: Any], value["type"] as? String == kind,
          let id = value["id"] as? String, validID(id),
          value["attributes"] == nil || value["attributes"] is [String: Any],
          value["relationships"] == nil || value["relationships"] is [String: Any] else { throw ApiError.invalidUpstream() }
    return value
}
func checkParent(_ value: [String: Any], _ relation: String, _ id: String, _ kind: String) throws {
    if let raw = (value["relationships"] as? [String: Any])?[relation] {
        guard let rel = raw as? [String: Any] else { throw ApiError.invalidUpstream() }
        if let rawData = rel["data"] {
            guard let data = rawData as? [String: Any], data["id"] as? String == id,
                  data["type"] as? String == kind else { throw ApiError.invalidUpstream() }
        }
    }
}
func flatten(_ raw: Any, _ route: Route, _ context: [String: Any], _ fields: [String]? = nil) throws -> [String: Any] {
    let value = try resource(raw, route.type)
    var result = context
    result["id"] = value["id"]
    let attrs = value["attributes"] as? [String: Any] ?? [:]
    for (name, schema) in route.attributes where fields?.contains(name) ?? true {
        if let item = attrs[name] {
            if !(item is NSNull) {
                let boolean = (item as? NSNumber).map { CFGetTypeID($0) == CFBooleanGetTypeID() } ?? false
                guard schema.type == "boolean" ? boolean : (item is String && !boolean) else { throw ApiError.invalidUpstream() }
            }
            result[name] = item
        }
    }
    let relationships = value["relationships"] as? [String: Any] ?? [:]
    for (name, relation) in route.relationships {
        if let rawRel = relationships[name] {
            guard let rel = rawRel as? [String: Any] else { throw ApiError.invalidUpstream() }
            if let data = rel["data"] {
                if data is NSNull { result[relation.field] = NSNull() }
                else { result[relation.field] = try resource(data, relation.type)["id"] }
            }
        }
    }
    return result
}
func validateUpdate(_ attributes: AgeRatingUpdate) throws -> [String: Any] {
    guard !attributes.isEmpty else { throw ApiError(400, "invalid_request", "Provide a nonempty object of supported age rating attributes.") }
    let encoded: Data
    do { encoded = try JSONEncoder().encode(attributes) }
    catch { throw ApiError(400, "invalid_request", "Provide valid JSON attributes.") }
    guard encoded.count <= 8192 else { throw ApiError(413, "request_too_large", "Age rating attributes must fit within 8 KiB.") }
    for (name, value) in attributes {
        var valid = false
        if let schema = Contracts.shared.ageRatingWrite[name] {
            switch value {
            case .null: valid = schema.nullable == true
            case .bool: valid = schema.type == "boolean"
            case .string(let text):
                valid = schema.type == "string" && text.utf16.count <= 2048 && (schema.enum?.contains(text) ?? true)
                if schema.format == "uri" {
                    let url = URLComponents(string: text)
                    valid = valid && ["http", "https"].contains(url?.scheme?.lowercased() ?? "") && !(url?.host?.isEmpty ?? true) && url?.user == nil && url?.password == nil
                }
            default: break
            }
        }
        guard valid else {
            throw ApiError(400, "invalid_request", "Unsupported age rating attribute or value.", details: [
                ErrorDetail(code: "invalid_attribute", message: "This age rating attribute or value is unsupported.", field: name)
            ])
        }
    }
    return try JSONSerialization.jsonObject(with: encoded) as! [String: Any]
}
