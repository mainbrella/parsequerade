import Foundation

// The core validates all operation input. Check generated method arguments before
// interpolation too, so a slash cannot turn an identifier into a different route.
func validateSegment(_ value: String, locale: Bool = false) throws {
    let pattern = locale ? "^[A-Za-z0-9][A-Za-z0-9-]{1,34}$" : "^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$"
    guard !value.hasSuffix("\n"), value.range(of: pattern, options: .regularExpression) != nil else {
        throw ApiError(400, "invalid_path", "The resource path is invalid.")
    }
}
