import Foundation

/// Facade query names and comma-separated values. Invalid values fail before dispatch.
public typealias Query = [String: String]

public struct Page<Item: Codable & Sendable>: Codable, Sendable {
    public let items: [Item]
    public let nextCursor: String?

    enum CodingKeys: String, CodingKey { case items, nextCursor }
    public func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(items, forKey: .items)
        try values.encode(nextCursor, forKey: .nextCursor)
    }
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
public struct ApiError: Error, LocalizedError, Sendable, Decodable {
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

    enum CodingKeys: String, CodingKey { case status, code, message, retryable, details, retryAfter }
    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        status = try values.decode(Int.self, forKey: .status)
        code = try values.decode(String.self, forKey: .code)
        message = try values.decode(String.self, forKey: .message)
        retryable = try values.decodeIfPresent(Bool.self, forKey: .retryable) ?? false
        details = try values.decodeIfPresent([ErrorDetail].self, forKey: .details) ?? []
        retryAfter = try values.decodeIfPresent(Int.self, forKey: .retryAfter)
    }

    static func invalidUpstream() -> Self {
        Self(502, "upstream_invalid_response", "Apple returned an invalid response.")
    }
}

/// Opaque, durable upload state. Save the entire Codable value and reuse it with
/// the same request and PNG to reconcile an interrupted or uncertain upload.
/// One checkpoint can participate in only one upload at a time, across clients.
public final class ListingImageUploadCheckpoint: Codable, @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String: JSONValue]
    private var active = false
    private var pendingPersistence = 0

    public init() { values = [:] }
    public init(from decoder: Decoder) throws {
        values = try decoder.singleValueContainer().decode([String: JSONValue].self)
    }
    public func encode(to encoder: Encoder) throws {
        lock.lock(); let snapshot = values; lock.unlock()
        var container = encoder.singleValueContainer()
        try container.encode(snapshot)
    }
    func begin() throws {
        lock.lock(); defer { lock.unlock() }
        guard !active, pendingPersistence == 0 else { throw ApiError(409, "checkpoint_in_use", "This checkpoint already has an upload or checkpoint save in progress.") }
        active = true
    }
    func end() { lock.lock(); active = false; lock.unlock() }
    func retainPersistence() { lock.lock(); pendingPersistence += 1; lock.unlock() }
    func releasePersistence() { lock.lock(); pendingPersistence -= 1; lock.unlock() }
    func replace(_ snapshot: [String: JSONValue]) {
        lock.lock(); values = snapshot; lock.unlock()
    }
}
