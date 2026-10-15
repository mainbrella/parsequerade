# Parsequerade for Go

A Go 1.22+ SDK that runs the Parsequerade facade in your process and calls App Store Connect directly. It covers all 12 implemented reads and the scoped age-rating update, with generated models and queries, exact-locale selection, ownership checks, and opaque pagination.

The SDK uses only the Go standard library, including ES256 signing and HTTPS. Credentials and JWT signing stay in your process. No hosted connection, facade API key, Node.js, or Worker source is needed by consumers.

## Install from this checkout

The module is `github.com/mainbrella/parsequerade/sdks/go`. There is no tagged Go module release yet. From your application's module, use a local replacement:

```sh
go mod edit -require=github.com/mainbrella/parsequerade/sdks/go@v0.0.0
go mod edit -replace=github.com/mainbrella/parsequerade/sdks/go=/path/to/parsequerade/sdks/go
go mod tidy
```

Add the import below before running `go mod tidy`. The SDK directory is self-contained and can also be copied outside this repository.

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

Construction validates a P-256 PKCS#8 key locally and makes no network requests. Reuse the client to share cached JWTs, cursor signing, and the four-request concurrency bound. Clients are safe for concurrent use; do not copy them. Every operation accepts a `context.Context` and respects earlier caller deadlines and cancellation.

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

Array query fields use `[]string`; `Limit: 0` and `Cursor: ""` mean omitted. Empty non-nil selections, unsupported fields, invalid enums, and invalid limits fail locally. Read enums remain strings so new Apple values can be returned. Go fields use standard initialisms, such as `BundleID` and `PrivacyPolicyURL`; JSON names retain the facade's camel case spelling.

Optional response fields use `Optional[T]`. `Present == false` means absent; `Present && Null` means explicit JSON null; otherwise `Value` contains the value, including empty strings or false. Generated models preserve these distinctions when marshaled back to JSON. Required IDs are ordinary strings.

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

Writes are never retried. A lost or malformed success response returns `write_outcome_unknown`, with `Retryable == false`, including cancellation after dispatch. Read the age rating before deciding whether to resubmit. Other writes, reports, copy workflows, and proposed routes are unsupported.

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

For a custom transport, pass `&parsequerade.Options{Transport: roundTripper}` to `NewClient`. This is a trusted `http.RoundTripper` that receives signed requests and must honor request contexts. The SDK bounds response reading and prevents redirects. The default transport uses standard Go TLS verification and connection pooling.

## Development

From the repository root:

```sh
npm run generate:sdks
npm run build:sdk:go
npm run check:sdk:go
npm run test:sdk:go
```

Generation and drift checks require Python and Node.js, without requiring Go. Go build, vet, and race tests require Go 1.22+ (race detection also requires a supported C toolchain). Generated models, queries, methods, contracts, license, and native fixtures share the reviewed inputs used by the other SDKs. Tests use the Worker's fixtures and generated keys without contacting Apple. The module uses GPL-3.0-or-later.
