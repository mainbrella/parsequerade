# Parsequerade for Swift

An async Swift SDK backed by the shared Rust execution core. All 22 reads, age-rating and metadata updates, version creation, build attachment, and resumable listing-image uploads execute the same validation, ownership resolution, pagination, retry and reconciliation code as Rust and Python.

Requires Swift 5.9+ and an XCFramework slice for the target platform. The package retains macOS 12+, iOS/tvOS 15+, and watchOS 8+ deployment declarations. A host-only development build contains only the current macOS architecture; an Apple platform declaration does not create its native binary slice. Linux is unsupported. Keep Apple team keys in private tooling or trusted processes.

## Build and add the package

From this checkout, build the native artifact before opening the package:

```sh
python3 scripts/build_swift_sdk.py --host
swift test --package-path sdks/swift
```

Rust and Xcode are required to build native artifacts. Consumers of a prepared package need neither Rust, Python, Node.js nor this checkout. Distribute the complete `sdks/swift` directory, including `ParsequeradeCore.xcframework`, `Package.swift`, and `Sources`; the binary target contains its C header and module map and references no checkout paths. The repository does not check in generated binaries or download them automatically.

Add a local Swift package dependency:

```swift
.package(name: "Parsequerade", path: "/path/to/prepared/swift-package")
// In the target dependencies:
.product(name: "Parsequerade", package: "Parsequerade")
```

To prepare device and simulator slices, explicitly choose the Rust targets:

```sh
python3 scripts/build_swift_sdk.py --targets aarch64-apple-darwin,aarch64-apple-ios,aarch64-apple-ios-sim,aarch64-apple-watchos,aarch64-apple-watchos-sim
```

Install each supported target with `rustup target add` first. The helper merges architectures belonging to the same platform into universal libraries and replaces the XCFramework with exactly the requested slices. Intel macOS/iOS simulator builds require their corresponding `x86_64` targets. Older watches require `arm64_32-apple-watchos` or `armv7k-apple-watchos`; an `aarch64` watch binary alone does not establish compatibility with every watchOS 8 device. tvOS targets and some watch architectures require a Rust toolchain that supports building their standard library; with nightly and `rust-src` installed, use `--toolchain nightly --build-std --targets aarch64-apple-tvos,aarch64-apple-tvos-sim`. Select every platform/architecture needed by the package you distribute; an absent slice fails the consumer build. Cross-compilation verifies compilation and packaging, not execution on a device. The arm64 macOS, iOS/tvOS/watchOS device and simulator archives have been built and assembled together; the copied package runs on macOS arm64, and Swift source compiles for iOS 15, tvOS 15, and watchOS 9 simulators. The installed Xcode 27 SDK rejects watchOS 8 simulator deployment targets, so that declared minimum and older watch architectures remain unverified.

`--skip-build` packages existing static libraries, `--debug` uses debug libraries, and `--test-package` builds and runs a macOS consumer from an isolated copy of the prepared package.

## Connect and read

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

Construction validates the P-256 PKCS#8 key locally without a network request. Reuse the client actor to share cached JWTs, cursor signing and the core's concurrency bound. Apple key permissions govern access. `close()` cancels outstanding operations and releases the client; repeated closes are safe. Cancellation after a mutation starts can return `write_outcome_unknown`, so read back or resume with the saved upload checkpoint before retrying.

All methods are `async throws`, with an optional trailing `query: Query = [:]`. Query values are comma-separated strings using facade names. Models are Codable and Sendable, with public initializers; nested media assets, integer dimensions, headers and upload operations are typed. Optional attributes combine absent and null values into `nil`; use `JSONValue` when the distinction matters. Read enums remain strings to allow new Apple values.

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
| `screenshotSets(appId:versionId:locale:query:)` | `Page<ScreenshotSet>` |
| `screenshots(screenshotSetId:query:)` | `Page<Screenshot>` |
| `previewSets(appId:versionId:locale:query:)` | `Page<PreviewSet>` |
| `previews(previewSetId:query:)` | `Page<Preview>` |
| `placements(appId:versionId:locale:query:)` | `Page<LibraryPlacement>` |
| `libraryImage(libraryImageId:query:)` | `LibraryImage` |
| `builds(appId:query:)` | `Page<Build>` |
| `build(appId:buildId:query:)` | `Build` |
| `versionBuild(appId:versionId:query:)` | `Build` |
| `buildPrereleaseVersion(appId:buildId:query:)` | `PrereleaseVersion` |

Generic reads preserve flat JSON and explicit nulls:

```swift
let app: JSONValue = try await client.get("/apps/1234567890", query: ["fields": "name,bundleId"])
```

Collections return `Page<Item>` with `items` and `nextCursor`. Keep the same path and original query while adding `query["cursor"] = page.nextCursor` until it becomes `nil`. Cursors expire after 15 minutes, bind the client and query, and permit up to 100 pages. Parent and locale resolvers permit 20 pages and reject incomplete or ambiguous results.

## Writes

```swift
let rating = try await client.updateAgeRating(
    appId: "123", appInfoId: "info1",
    attributes: ["gambling": .bool(false), "kidsAgeBand": .null]
)
let metadata = try await client.updateMetadata(
    "/apps/123/versions/version1/localizations/en-US",
    attributes: ["promotionalText": "A clearer way to take notes."]
)
let version = try await client.createVersion(appId: "123", platform: "IOS", versionString: "2.0")
let attached = try await client.attachBuild(appId: "123", versionId: version.id, buildId: "build1")
```

Writes validate scope, ownership and supported attributes in Rust. Build attachment also checks draft state, eligibility, platform/version matching and confirmation. Writes are never retried. An uncertain response or cancellation can return `write_outcome_unknown` with `retryable == false`; read back before deciding whether to resubmit.

## Resumable listing images

```swift
let request = ListingImageUpload(appId: "123", versionId: "version1", locale: "en-US",
    placementType: .productPageHeaderAsset, fileName: "listing.png")
let checkpoint = ListingImageUploadCheckpoint() // or restore with JSONDecoder
let receipt = try await client.uploadListingImage(request,
    png: Data(contentsOf: pngURL), checkpoint: checkpoint) { snapshot in
        try JSONEncoder().encode(snapshot).write(to: checkpointURL, options: .atomic)
    }
```

`listingImageReferenceData()` reads Apple's live placement and PNG specifications. Uploads validate the PNG and live policy, reserve once, transfer bounded chunks, commit once, and verify placement. The checkpoint is synchronized, Codable state and is updated even when an operation fails. Persist each independent snapshot durably before returning; a persistence error is propagated unchanged and prevents the next write. One checkpoint cannot be used concurrently, even across clients. If cancellation returns while a persistence callback is still running, the checkpoint remains busy until that callback exits. Reuse the original request, PNG and saved checkpoint to reconcile uncertainty; never start again with empty state after an uncertain reservation. Checkpoints exclude signed upload URLs and headers.

## Errors and transport

`ApiError` exposes `status`, `code`, `message`, `retryable`, `retryAfter` and sanitized field `details`. The shared core owns authentication, retries, limits and response validation. Reads have a 25-second operation budget, up to 32 Apple attempts, an eight-second attempt timeout, 4 MiB per response and 16 MiB total bodies; uploads use their own larger bounded transfer budget.

The default transport runs inside Rust. A trusted custom `AppleTransport` closure receives signed `URLRequest`s and returns `(Data, HTTPURLResponse)`; preserve the timeout and avoid redirects and retries. Upload blob requests receive no Apple authorization header. The adapter enforces response limits and races callbacks against cancellation and timeout, discarding late results. Blocking core polling runs on Dispatch workers, keeping Swift's cooperative executor available.

Generated models come from the common SDK catalog. Tests cover the shared read/write/upload scenarios, JWT signatures, recovery, persistence failures and cancellation without contacting Apple. The package uses GPL-3.0-or-later.
