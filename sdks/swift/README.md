# Parsequerade for Swift

An async Swift package that runs the Parsequerade facade locally and calls App Store Connect directly. It covers all 12 implemented reads and the scoped age-rating update, with Codable models, validated facade queries, exact-locale selection, ownership checks, and opaque pagination.

The package uses Foundation and CryptoKit, with no external dependencies. It requires Swift 5.9+ on Apple platforms: macOS 12+, iOS/tvOS 15+, or watchOS 8+. Linux is not supported. Use it in private tooling or server processes where your Apple team key can remain private.

## Add to your project

The package is not yet published as a standalone repository. Add a local package dependency:

```swift
// Package.swift
.package(name: "Parsequerade", path: "/path/to/parsequerade/sdks/swift")

// In your target's dependencies:
.product(name: "Parsequerade", package: "Parsequerade")
```

In Xcode, use **Add Local Package** and select `sdks/swift`. The package bundles its contracts and requires neither Python, Node.js, nor Worker source at runtime.

## Connect and read

Supply an App Store Connect **team API key**, its issuer ID, key ID, and the PEM contents of the downloaded `.p8` file from private configuration:

```swift
import Foundation
import Parsequerade

let env = ProcessInfo.processInfo.environment
let client = try Client(
    issuerID: env["ASC_ISSUER_ID"]!,
    keyID: env["ASC_KEY_ID"]!,
    privateKey: String(contentsOfFile: env["ASC_PRIVATE_KEY_PATH"]!, encoding: .utf8)
)

let apps = try await client.apps(query: ["bundleId": "com.example.notes", "limit": "50"])
for app in apps.items { print(app.id, app.name ?? "(unnamed)") }
```

Construction validates the P-256 PKCS#8 key locally without a network request. Reuse the `Client` actor to share cached JWTs, cursor signing, and a four-request concurrency bound. Apple key permissions govern access; hosted connections and facade API keys are unnecessary.

The [runnable example](Examples/ListApps.swift) uses the same environment variables:

```sh
swift run --package-path sdks/swift list-apps
```

## Resource methods

All methods are `async throws`, with an optional trailing `query: Query = [:]`. Queries use facade names and comma-separated string values.

| Method | Return type |
| --- | --- |
| `apps(query:)` | `Page<App>` |
| `app(appId:query:)` | `App` |
| `appInfos(appId:query:)` | `Page<AppInfo>` |
| `appInfo(appId:appInfoId:query:)` | `AppInfo` |
| `appInfoLocalizations(appId:appInfoId:query:)` | `Page<AppInfoLocalization>` |
| `appInfoLocalization(appId:appInfoId:locale:query:)` | `AppInfoLocalization` |
| `ageRating(appId:appInfoId:query:)` | `AgeRating` |
| `versions(appId:query:)` | `Page<Version>` |
| `version(appId:versionId:query:)` | `Version` |
| `versionLocalizations(appId:versionId:query:)` | `Page<VersionLocalization>` |
| `versionLocalization(appId:versionId:locale:query:)` | `VersionLocalization` |
| `reviewContact(appId:versionId:query:)` | `ReviewContact` |

```swift
let versions = try await client.versions(appId: "1234567890", query: [
    "platform": "IOS,MAC_OS", "state": "PREPARE_FOR_SUBMISSION",
    "fields": "versionString,platform", "limit": "25",
])
let localization = try await client.versionLocalization(
    appId: "1234567890", versionId: "version_123", locale: "en-US",
    query: ["fields": "description"]
)
```

Runtime validation rejects unknown query names, unsupported fields, invalid enums and limits before dispatch. Models use the facade's camel case property names. Optional attributes use `Optional`, combining missing and explicit null values into `nil`; re-encoding omits them. Read enums remain strings so new Apple values can be returned.

For exact flat JSON, including explicit nulls, use the generic entry point with `JSONValue`:

```swift
let app: JSONValue = try await client.get("/apps/1234567890", query: ["fields": "name,bundleId"])
```

Only implemented relative facade paths are accepted. Hosted connection prefixes, arbitrary URLs, and proposed routes are unsupported.

## Pagination

Collections return `Page<Item>` with `items` and `nextCursor`. Keep the original query and path arguments, adding each cursor:

```swift
var query: Query = ["bundleId": "com.example.notes", "limit": "50"]
repeat {
    let page = try await client.apps(query: query)
    for app in page.items { print(app.id) }
    query["cursor"] = page.nextCursor
} while query["cursor"] != nil
```

Cursors expire after 15 minutes, bind the client and original request, and permit up to 100 pages. They cannot be exchanged with hosted or other SDK cursors. Apple continuation links must retain the origin, path, and query; missing filters are preserved. Repeated pages fail. Parent and exact-locale resolvers permit 20 pages and fail on incomplete or ambiguous results.

## Update an age rating

```swift
let rating = try await client.updateAgeRating(
    appId: "1234567890", appInfoId: "app_info_123",
    attributes: ["gambling": .bool(false), "ageRatingOverrideV2": .string("NONE"), "kidsAgeBand": .null]
)
```

`AgeRatingUpdate` is a dictionary of `JSONValue`. The SDK validates a nonempty flat object against the reviewed field allowlist, enums, nullability, URL rules, and 8 KiB limit; verifies ownership; resolves the declaration ID; then sends one Apple PATCH. Omitted fields stay unchanged. Supported nullable fields can be cleared with `.null`. Apple enforces key permissions and editable states.

Writes are never retried. A lost or malformed update response throws `write_outcome_unknown`, with `retryable == false`. Read the age rating before deciding whether to resubmit. Cancellation after dispatch can also leave an uncertain write outcome. Other writes, reports, copy workflows, and proposed routes are unsupported.

## Errors and bounds

```swift
do {
    _ = try await client.apps()
} catch let error as ApiError {
    print(error.status, error.code, error.message)
    print(error.retryable, error.retryAfter as Any, error.details)
}
```

Errors expose stable facade codes, sanitized messages and field details; raw Apple errors, URLs and credentials are excluded. Reads retry at most twice, refresh a rejected JWT once, and honor throttle delays up to two seconds. Redirects, shared cookies, and URL caching are disabled. Each operation has a 25-second budget, 32 Apple attempts, an eight-second attempt timeout, 4 MiB per response, and 16 MiB of total bodies.

For a trusted custom transport, pass an `AppleTransport` closure to the initializer. It receives signed `URLRequest`s and returns `(Data, HTTPURLResponse)`. It must honor `request.timeoutInterval`, bound body reading, and avoid redirects. The client still validates the response and total byte limits.

## Development

From the repository root:

```sh
npm run generate:sdks
npm run build:sdk:swift
npm run test:sdk:swift
```

Generated models and bundled contracts use the same reviewed inputs as the other SDKs. Tests share the Worker's resource fixtures and verify generated JWT signatures without contacting Apple. The package uses GPL-3.0-or-later.
