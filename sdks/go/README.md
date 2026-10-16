# Parsequerade for Go

A Go 1.22+ SDK that runs the Parsequerade facade in your process and calls App Store Connect directly. It covers the same 22 reads and all supported mutations as Rust and Python, including metadata updates, version creation, verified build attachment, and resumable listing-image uploads.

Operation validation, signing, resolution, pagination, retries, and write recovery execute in the shared Rust core. Go provides contexts, typed models, errors, and an `http.RoundTripper` adapter. Credentials stay in your process. Consumers need no hosted connection, Node.js, or Worker source.

**Native build requirement:** this version requires Go 1.22+, `CGO_ENABLED=1`, a C compiler, and a Rust static library matching the target OS and architecture. Source checkouts also need the Rust toolchain to build that library. This replaces the earlier pure-Go runtime.

## Install from this checkout

The module is `github.com/mainbrella/parsequerade/sdks/go`. There is no tagged Go module release yet. First build the native library from the repository root:

```sh
python3 scripts/build_go_sdk.py
```

Then, from your application's module, use a local replacement:

```sh
go mod edit -require=github.com/mainbrella/parsequerade/sdks/go@v0.0.0
go mod edit -replace=github.com/mainbrella/parsequerade/sdks/go=/path/to/parsequerade/sdks/go
go mod tidy
```

Add the import below before running `go mod tidy`. After the native build, the SDK directory is self-contained and can be copied outside this repository, including `native/libparsequerade_native.a`, `native/parsequerade.h`, and `native_build.go`. The archive is a build artifact excluded from Git; a fresh source checkout must run the build helper. A packaged SDK must bundle the archive for its stated platform.

For cross compilation, pass `--target <rust-target-triple>` to the build helper and configure matching Go `GOOS`/`GOARCH` and a cgo C compiler. An archive built for one target cannot be reused on another. Native packaging has been verified on macOS arm64; other platforms need their own builds and validation.

## Connect and read

Supply an App Store Connect **team API key**, its issuer ID, key ID, and the PEM contents of the downloaded `.p8` file from private application configuration:

```go
import (
    "context"
    "fmt"
    "os"

    parsequerade "github.com/mainbrella/parsequerade/sdks/go"
)

privateKey, err := os.ReadFile(os.Getenv("ASC_PRIVATE_KEY_PATH"))
if err != nil {
    return err
}
client, err := parsequerade.NewClient(parsequerade.Credentials{
    IssuerID:   os.Getenv("ASC_ISSUER_ID"),
    KeyID:      os.Getenv("ASC_KEY_ID"),
    PrivateKey: privateKey,
}, nil)
if err != nil {
    return err
}

defer client.Close()

page, err := client.Apps(context.Background(), &parsequerade.AppsQuery{
    BundleID: []string{"com.example.notes"},
    Limit:    50,
})
if err != nil {
    return err
}
for _, app := range page.Items {
    fmt.Println(app.ID, app.Name.Value)
}
```

Construction validates a P-256 PKCS#8 key locally and makes no network requests. Reuse the client to share cached JWTs, cursor signing, and the four-request concurrency bound. Clients are safe for concurrent use; do not copy them. Call `Close()` to release a client and cancel active operations; it is idempotent and safe to call concurrently. Every operation accepts a `context.Context` and respects earlier caller deadlines and cancellation.

The [runnable example](examples/list-apps/main.go) uses the same environment variables, handles pagination, and stops on Ctrl-C:

```sh
cd sdks/go
go run ./examples/list-apps
```

## Resource methods

Every read takes `ctx` first and a pointer to its generated query struct last. Pass `nil` for default query options. All methods return `(value, error)`.

| Method | Value type | Query type |
| --- | --- | --- |
| `Apps(ctx, query)` | `Page[App]` | `*AppsQuery` |
| `App(ctx, appID, query)` | `App` | `*AppQuery` |
| `AppInfos(ctx, appID, query)` | `Page[AppInfo]` | `*AppInfosQuery` |
| `AppInfo(ctx, appID, appInfoID, query)` | `AppInfo` | `*AppInfoQuery` |
| `AppInfoLocalizations(ctx, appID, appInfoID, query)` | `Page[AppInfoLocalization]` | `*AppInfoLocalizationsQuery` |
| `AppInfoLocalization(ctx, appID, appInfoID, locale, query)` | `AppInfoLocalization` | `*AppInfoLocalizationQuery` |
| `AgeRating(ctx, appID, appInfoID, query)` | `AgeRating` | `*AgeRatingQuery` |
| `Versions(ctx, appID, query)` | `Page[Version]` | `*VersionsQuery` |
| `Version(ctx, appID, versionID, query)` | `Version` | `*VersionQuery` |
| `VersionLocalizations(ctx, appID, versionID, query)` | `Page[VersionLocalization]` | `*VersionLocalizationsQuery` |
| `VersionLocalization(ctx, appID, versionID, locale, query)` | `VersionLocalization` | `*VersionLocalizationQuery` |
| `ReviewContact(ctx, appID, versionID, query)` | `ReviewContact` | `*ReviewContactQuery` |
| `ScreenshotSets(ctx, appID, versionID, locale, query)` | `Page[ScreenshotSet]` | `*ScreenshotSetsQuery` |
| `Screenshots(ctx, screenshotSetID, query)` | `Page[Screenshot]` | `*ScreenshotsQuery` |
| `PreviewSets(ctx, appID, versionID, locale, query)` | `Page[PreviewSet]` | `*PreviewSetsQuery` |
| `Previews(ctx, previewSetID, query)` | `Page[Preview]` | `*PreviewsQuery` |
| `Placements(ctx, appID, versionID, locale, query)` | `Page[LibraryPlacement]` | `*PlacementsQuery` |
| `LibraryImage(ctx, imageID, query)` | `LibraryImage` | `*LibraryImageQuery` |
| `Builds(ctx, appID, query)` | `Page[Build]` | `*BuildsQuery` |
| `Build(ctx, appID, buildID, query)` | `Build` | `*BuildQuery` |
| `VersionBuild(ctx, appID, versionID, query)` | `Build` | `*VersionBuildQuery` |
| `BuildPrereleaseVersion(ctx, appID, buildID, query)` | `PrereleaseVersion` | `*BuildPrereleaseVersionQuery` |

Array query fields use `[]string`; `Limit: 0` and `Cursor: ""` mean omitted. Empty non-nil selections, unsupported fields, invalid enums, and invalid limits fail locally. Read enums remain strings so new Apple values can be returned. Go fields use standard initialisms, such as `BundleID` and `PrivacyPolicyURL`; JSON names retain the facade's camel case spelling.

Optional response fields use `Optional[T]`. `Present == false` means absent; `Present && Null` means explicit JSON null; otherwise `Value` contains the value, including empty strings or false. Generated models preserve these distinctions when marshaled back to JSON. Required IDs are ordinary strings. Nested media objects and arrays have generated types; unknown nested fields are retained in their `Extra` maps. Numeric media dimensions, sizes, and offsets use `int64`.

`client.Get(ctx, path, query)` returns `json.RawMessage` for implemented relative facade paths such as `/apps/123`. Its `Query` is an alias for `url.Values`, using facade names, for example `parsequerade.Query{"limit": {"25"}, "bundleId": {"one,two"}}`. Unknown keys, duplicate values, and malformed paths fail locally. Arbitrary URLs and hosted connection prefixes are unsupported.

## Pagination

Collections return `Page[T]` with `Items` and `NextCursor`. Keep the original query and path arguments, adding the cursor:

```go
query := &parsequerade.AppsQuery{BundleID: []string{"com.example.notes"}, Limit: 50}
for {
    page, err := client.Apps(ctx, query)
    if err != nil {
        return err
    }
    for _, app := range page.Items {
        fmt.Println(app.ID)
    }
    if page.NextCursor == nil {
        break
    }
    query.Cursor = *page.NextCursor
}
```

Cursors last 15 minutes, bind this client and the original request, and permit up to 100 pages. They cannot be exchanged with hosted or other SDK cursors. Apple continuation links must retain the origin, path, and query; missing filters are preserved. Repeated pages fail. Parent and exact-locale resolvers permit 20 pages and fail if the result is incomplete or ambiguous.

## Update an age rating

```go
rating, err := client.UpdateAgeRating(ctx, "1234567890", "app_info_123", parsequerade.AgeRatingUpdate{
    Gambling:            parsequerade.Some(false),
    AgeRatingOverrideV2: parsequerade.Some("NONE"),
    KidsAgeBand:         parsequerade.Null[string](),
})
```

The zero value of an update field means omitted. Use `Some(value)` to set it and `Null[T]()` to clear a supported nullable field. Updates snapshot and validate the nonempty payload before lookups, enforcing the reviewed field allowlist, enums, nullability, URL rules, and 8 KiB limit. The SDK verifies ownership, resolves the declaration ID, then sends one Apple PATCH. Apple key permissions and editable states apply directly.

Writes are never retried. A lost or malformed success response returns `write_outcome_unknown`, with `Retryable == false`, including cancellation after dispatch. Read the age rating before deciding whether to resubmit. The supported metadata, version, build, and image operations below use the same shared write-safety rules.

## Metadata, versions, and builds

```go
updated, err := client.UpdateMetadata(ctx, "/apps/123/versions/version_1/localizations/en-US", map[string]any{
    "whatsNew": "Improved offline search.",
})
version, err := client.CreateVersion(ctx, "123", "IOS", "2.0")
version, err = client.AttachBuild(ctx, "123", "version_1", "build_1")
```

`UpdateMetadata` returns `json.RawMessage`. It supports reviewed fields on apps, versions, app-info localizations, and version localizations. `CreateVersion` and `AttachBuild` return `Version`. The core verifies ownership and editability; attaching a build checks eligibility and matching version/platform, sends one PATCH, and verifies the selected build through reads.

## Resumable listing-image uploads

```go
checkpoint := parsequerade.ListingImageUploadCheckpoint{}
receipt, err := client.UploadListingImage(ctx, parsequerade.ListingImageUpload{
    AppID: "123", VersionID: "version_1", Locale: "en-US",
    PlacementType: "PRODUCT_PAGE_HEADER_ASSET", FileName: "listing.png",
}, png, &checkpoint, func(snapshot parsequerade.ListingImageUploadCheckpoint) error {
    data, err := json.Marshal(snapshot)
    if err != nil { return err }
    return saveCheckpointDurably(data)
})
```

The callback must durably save each snapshot before returning nil. Its original error is returned and prevents the next write. The checkpoint pointer is updated before the callback and again on completion or error. Callback snapshots are independent values. Callbacks run in a goroutine so context cancellation or `Close` can stop the operation even while persistence is blocked. A blocked callback may continue after the operation returns; it must release its own resources. Late callback results cannot change the returned checkpoint. The checkpoint remains busy until both the operation and any outstanding callback finish, preventing a late durable save from racing a resumed upload. Reentrant or concurrent use of the same checkpoint pointer fails with `checkpoint_in_use`; do not read or modify an active checkpoint from another goroutine.

Persist the checkpoint on failure and resume with the same request and PNG. The core validates live asset specifications, transfers exact chunks to trusted Apple blob hosts, commits once, and verifies placement. An uncertain reservation is never replayed; uncertain placement is reconciled through reads. Cancellation after a mutation starts conservatively returns `write_outcome_unknown`. Use `ListingImageReferenceData(ctx)` to read the live specification catalog as `json.RawMessage`.

## Errors and bounds

```go
var api *parsequerade.APIError
if errors.As(err, &api) {
    fmt.Println(api.Status, api.Code, api.Message)
    fmt.Println(api.Retryable, api.RetryAfter, api.Details)
}
```

Errors use stable facade codes and sanitized messages; raw Apple errors, URLs, and credentials are excluded. `RetryAfter` is an optional number of seconds. Reads return standard context errors on caller cancellation or deadline expiry. Reads retry at most twice, refresh a rejected JWT once, and honor throttle delays of up to two seconds. Redirects are disabled.

Each operation has a 25-second budget, 32 Apple attempts, an eight-second network timeout per attempt, 4 MiB per response, and 16 MiB of total bodies. A scoped update shares one operation budget across resolution and PATCH.

For a custom transport, pass `&parsequerade.Options{Transport: roundTripper}` to `NewClient`. This is a trusted `http.RoundTripper` that receives signed requests and must honor request contexts. The SDK bounds response reading and prevents redirects. The default adapter uses `http.DefaultTransport`, including Go TLS verification and connection pooling. All facade behavior still runs in Rust.

## Development

From the repository root:

```sh
npm run generate:sdks
npm run build:sdk:go
npm run check:sdk:go
npm run test:sdk:go
```

Generation and drift checks require Python and Node.js. Build, vet, and race tests also require Go 1.22+, Rust, and a supported C compiler; the npm commands stage the native archive first. Generated models and methods consume the common catalog. Tests exercise the shared Rust operation and upload scenarios, context cancellation, concurrent close, durable checkpoints, and an isolated consumer build without contacting Apple. The module uses GPL-3.0-or-later.
