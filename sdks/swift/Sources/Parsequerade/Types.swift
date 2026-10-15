import Foundation

/// Facade query names and comma-separated values. Invalid values fail before dispatch.
public typealias Query = [String: String]

public struct Page<Item: Codable & Sendable>: Codable, Sendable {
    public let items: [Item]
    public let nextCursor: String?
}

/// JSON that preserves explicit nulls. Used by raw reads and age-rating updates.
public enum JSONValue: Codable, Sendable, Equatable {
    case string(String), bool(Bool), number(Double), null
    case array([JSONValue]), object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer()
        if value.decodeNil() { self = .null }
        else if let v = try? value.decode(Bool.self) { self = .bool(v) }
        else if let v = try? value.decode(String.self) { self = .string(v) }
        else if let v = try? value.decode(Double.self) { self = .number(v) }
        else if let v = try? value.decode([JSONValue].self) { self = .array(v) }
        else { self = .object(try value.decode([String: JSONValue].self)) }
    }

    public func encode(to encoder: Encoder) throws {
        var value = encoder.singleValueContainer()
        switch self {
        case .null: try value.encodeNil()
        case .string(let v): try value.encode(v)
        case .bool(let v): try value.encode(v)
        case .number(let v): try value.encode(v)
        case .array(let v): try value.encode(v)
        case .object(let v): try value.encode(v)
        }
    }
}

public typealias AgeRatingUpdate = [String: JSONValue]

public struct ErrorDetail: Codable, Sendable, Equatable {
    public let code: String
    public let message: String
    public let field: String?
}

/// Stable facade errors. Apple messages, credentials and request URLs are excluded.
public struct ApiError: Error, LocalizedError, Sendable {
    public let status: Int
    public let code: String
    public let message: String
    public internal(set) var retryable: Bool
    public internal(set) var details: [ErrorDetail]
    public internal(set) var retryAfter: Int?
    public var errorDescription: String? { "\(code): \(message)" }

    init(_ status: Int, _ code: String, _ message: String, retryable: Bool = false,
         details: [ErrorDetail] = [], retryAfter: Int? = nil) {
        self.status = status; self.code = code; self.message = message
        self.retryable = retryable; self.details = details; self.retryAfter = retryAfter
    }

    static func invalidUpstream() -> Self {
        Self(502, "upstream_invalid_response", "Apple returned an invalid response.")
    }
    static func notFound(_ code: String = "resource_not_found") -> Self {
        Self(404, code, "The requested resource was not found.")
    }
    static func incomplete() -> Self {
        Self(502, "upstream_incomplete_result", "Apple pagination exceeded its bound or repeated a page.")
    }
    static func unknownWrite() -> Self {
        Self(503, "write_outcome_unknown", "Apple did not confirm this update. Read the age rating before trying again.")
    }
    static func invalidQuery(_ field: String) -> Self {
        Self(400, "invalid_query", "The request query is invalid.", details: [
            ErrorDetail(code: "invalid_parameter", message: "Provide a supported parameter and value.", field: field)
        ])
    }
}
