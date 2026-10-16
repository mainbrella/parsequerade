# Parsequerade for Rust

An async Rust crate that implements the Parsequerade facade locally. Your program calls Apple directly at `https://api.appstoreconnect.apple.com`, with flat resources and facade query names. No Cloudflare deployment, hosted connection, or facade API key is needed.

The SDK supports 22 reads covering core resources, media, and builds; age-rating and metadata updates; version creation; build attachment; and checkpointed PNG uploads for App Store listing images. The Python, TypeScript, Go, and Swift SDKs use this same execution core and support these operations. The hosted Worker retains its baseline route set. Reports, copy workflows, and other proposed operations are not implemented.

## Add to your project

The crate is not yet published to crates.io. Point Cargo at this checkout:

```toml
[dependencies]
parsequerade = { path = "/path/to/parsequerade/sdks/rust" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
serde_json = "1" # only needed for JSON access or age-rating updates
```

Supply an App Store Connect **team API key**, its issuer ID, key ID, and the PKCS#8 PEM contents of the downloaded `.p8` file. Keep the key in your application's private configuration.

```rust,no_run
use parsequerade::{Client, Query};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let private_key = std::fs::read_to_string(std::env::var("ASC_PRIVATE_KEY_PATH")?)?;
    let client = Client::new(
        &std::env::var("ASC_ISSUER_ID")?,
        &std::env::var("ASC_KEY_ID")?,
        &private_key,
    )?;

    let apps = client.apps(&Query::new().with("limit", 50)).await?;
    for app in apps.items {
        println!("{}: {}", app.id, app.name.as_deref().unwrap_or("(unnamed)"));
    }
    Ok(())
}
```

Reuse the client or its cheap clones to share pooled HTTP connections, cached JWTs, cursor signing, and the four-request concurrency bound. Constructing it validates the key locally and makes no network requests. It signs ES256 tokens with Apple's audience, caches them for up to ten minutes, and renews them before expiration.

The runnable [list-apps example](examples/list_apps.rs) handles pagination:

```sh
export ASC_ISSUER_ID='YOUR_ISSUER_UUID'
export ASC_KEY_ID='YOUR_KEY_ID'
export ASC_PRIVATE_KEY_PATH='/path/to/AuthKey.p8'
cargo run --manifest-path sdks/rust/Cargo.toml --example list_apps
```

## Read resources

Every read accepts a `&Query`, including single-resource reads where only `fields` is supported. Use facade names and comma-separated values; the SDK translates them to Apple's query parameters and rejects unsupported names, fields, values, and duplicates before requesting data.

```rust,ignore
let versions = client.versions(
    "1234567890",
    &Query::new()
        .with("platform", "IOS")
        .with("state", "PREPARE_FOR_SUBMISSION")
        .with("fields", "versionString,platform")
        .with("limit", 25),
).await?;

let localization = client.version_localization(
    "1234567890", "version_123", "en-US", &Query::new(),
).await?;
println!("{:?}", localization.description);
```

| Method | Facade resource | Return type |
| --- | --- | --- |
| `apps(query)` | `/apps` | `Page<App>` |
| `app(app_id, query)` | `/apps/{appId}` | `App` |
| `app_infos(app_id, query)` | `/apps/{appId}/app-infos` | `Page<AppInfo>` |
| `app_info(app_id, app_info_id, query)` | `/apps/{appId}/app-infos/{appInfoId}` | `AppInfo` |
| `app_info_localizations(app_id, app_info_id, query)` | `…/app-infos/{appInfoId}/localizations` | `Page<AppInfoLocalization>` |
| `app_info_localization(app_id, app_info_id, locale, query)` | `…/app-infos/{appInfoId}/localizations/{locale}` | `AppInfoLocalization` |
| `age_rating(app_id, app_info_id, query)` | `…/app-infos/{appInfoId}/age-rating` | `AgeRating` |
| `versions(app_id, query)` | `/apps/{appId}/versions` | `Page<Version>` |
| `version(app_id, version_id, query)` | `/apps/{appId}/versions/{versionId}` | `Version` |
| `version_localizations(app_id, version_id, query)` | `…/versions/{versionId}/localizations` | `Page<VersionLocalization>` |
| `version_localization(app_id, version_id, locale, query)` | `…/versions/{versionId}/localizations/{locale}` | `VersionLocalization` |
| `review_contact(app_id, version_id, query)` | `…/versions/{versionId}/review-contact` | `ReviewContact` |
| `screenshot_sets(app_id, version_id, locale, query)` | `…/versions/{versionId}/localizations/{locale}/screenshot-sets` | `Page<ScreenshotSet>` |
| `screenshots(screenshot_set_id, query)` | `/screenshot-sets/{screenshotSetId}/screenshots` | `Page<Screenshot>` |
| `preview_sets(app_id, version_id, locale, query)` | `…/versions/{versionId}/localizations/{locale}/preview-sets` | `Page<PreviewSet>` |
| `previews(preview_set_id, query)` | `/preview-sets/{previewSetId}/previews` | `Page<Preview>` |
| `placements(app_id, version_id, locale, query)` | `…/versions/{versionId}/localizations/{locale}/placements` | `Page<LibraryPlacement>` |
| `library_image(library_image_id, query)` | `/library-images/{libraryImageId}` | `LibraryImage` |

Responses have `id` and flattened attributes and relationship IDs. Child resources also carry `app_id` and, where applicable, `app_info_id` or `version_id`. Rust fields use snake case; Serde uses the hosted facade's camel case JSON names. Optional attributes use `Option`, so omitted and null values both deserialize to `None` and are omitted when reserialized. For exact JSON including explicit nulls, use the generic facade entry point:

```rust,ignore
let app: serde_json::Value = client.get(
    "/apps/1234567890", &Query::new().with("fields", "name,bundleId"),
).await?;
println!("{}", app["name"]);
```

Paths are relative to a connection; there is no `/v1/connections/{id}` prefix. The generic entry point accepts only implemented facade paths, never arbitrary Apple URLs.

## Screenshots and previews

Media reads follow the [documented facade](https://parsequerade.mainbrella.com/docs/). Select sets for an exact version locale, then read each set's ordered assets. These are paginated collections, with the same cursor behavior as other reads. Set queries accept `screenshotDisplayType` or `previewType`, respectively, plus the documented localization filters, `fields`, `limit`, and `cursor`. Asset queries accept `fields`, `limit`, and `cursor`.

`placements` reads the selected version localization's asset placements and includes image and video relationships so their IDs are available. Its query accepts the documented placement filters, `sort`, `fields`, `limit`, and `cursor`. `library_image` reads one asset and accepts `fields`. These adapters return metadata only; they do not fetch image bytes.

```rust,ignore
let placements = client
    .placements(
        "1234567890",
        "version_123",
        "en-US",
        &Query::new().with(
            "placementType",
            "PRODUCT_PAGE_HEADER_ASSET,APP_STORE_SEARCH_RESULTS_ASSET",
        ),
    )
    .await?;
for placement in placements.items {
    if let Some(image_id) = placement.image_id.as_deref() {
        let image = client.library_image(image_id, &Query::new()).await?;
        println!("{:?}: {:?}", placement.placement_type, image.image_asset);
    }
}
```

```rust,ignore
let sets = client.screenshot_sets(
    "1234567890", "version_123", "en-US", &Query::new().with("limit", 200),
).await?;
for set in sets.items {
    let screenshots = client.screenshots(&set.id, &Query::new().with("limit", 200)).await?;
    for screenshot in screenshots.items {
        if let Some(image) = screenshot.image_asset {
            println!("{:?}: {:?} × {:?}", image.template_url, image.width, image.height);
        }
    }
}
```

`Screenshot::image_asset` contains Apple's image URL template and dimensions. `Preview::preview_frame_image` contains a nested `image` with the same fields and its processing `state`; a preview can also contain `video_url` and `video_delivery_state`. Missing or processing media remain optional. The SDK returns asset metadata without downloading image bytes or playing videos. Generic `get::<Page<serde_json::Value>>` reads preserve explicit nulls and additional fields inside nested asset metadata.

## Upload a listing image

The Rust SDK can upload one PNG as a product page header or App Store search result image. It validates the PNG dimensions and Apple's live reference data before reserving an image, uploads the reserved byte ranges, commits the image, and creates a placement for the exact version localization. Existing placements are never replaced. Apple must grant the key access to the App Asset Library and the selected version.

The runnable example reads Studio's flat credentials JSON (`connection.issuerId`, `connection.keyId`, and `connection.privateKey`), writes an atomic checkpoint before each Apple write, and prints a receipt only after reading the placement back:

```sh
cargo run --manifest-path sdks/rust/Cargo.toml --example upload_listing_image -- \
  --credentials ./credentials.json \
  --app-id 1234567890 --version-id VERSION_ID --locale en-US \
  --placement-type PRODUCT_PAGE_HEADER_ASSET --png ./header.png \
  --receipt ./header.receipt.json
```

Use `APP_STORE_SEARCH_RESULTS_ASSET` for search artwork. Header PNGs must be exactly 3840×1646; search PNGs must be exactly 1920×1280. The checkpoint defaults to `<receipt>.checkpoint.json`. Keep it with the same PNG and request to resume. A `write_outcome_unknown` error means Apple may have accepted the request; rerun with the same checkpoint so the SDK can reconcile the image or placement before continuing. The checkpoint contains IDs and a local content binding, never upload URLs or credentials. The command creates an exclusive `.lock` file while running; after a crash, inspect the checkpoint and remove a stale lock manually.

To inspect Apple's current listing image specifications without starting an upload, run the example's read-only catalog mode:

```sh
cargo run --manifest-path sdks/rust/Cargo.toml --example upload_listing_image -- \\
  --credentials ./credentials.json \\
  --reference-data ./listing-image-reference-data.json
```

This mode makes only the reference-data GET request and saves Apple's catalog JSON. It creates no upload checkpoint or lock and sends no upload, commit, or placement writes. Credentials are read locally and are never printed or copied into the output. The same read-only API is available as `Client::listing_image_reference_data().await?`, which returns the catalog as `serde_json::Value`.

The public API is `Client::upload_listing_image(&request, &png_bytes, &mut checkpoint, persist).await`. `ListingImageUpload` contains `app_id`, `version_id`, `locale`, `placement_type` (`ListingImagePlacementType::ProductPageHeader` or `SearchResults`), and `file_name`. Begin with `ListingImageUploadCheckpoint::default()`, then deserialize the saved checkpoint when resuming. The `persist` callback accepts `&ListingImageUploadCheckpoint` and returns `parsequerade::Result<()>`; it must save every supplied state durably before returning `Ok(())`. Serialize access to each checkpoint. The CLI example implements atomic saves and an exclusive lock.

An uncertain reservation without a returned image ID stops for manual reconciliation in the asset library; it never repeats the reservation. An uncertain placement is reconciled by the saved image ID and exact target. If Apple still returns no matching placement, the command remains uncertain and makes no additional placement request. Preserve the checkpoint rather than starting a fresh one.

The upload is a local SDK capability that calls App Store Connect and Apple's blob storage directly. It does not add hosted upload support. Blob transfers allow at most 128 operations covering the PNG exactly, with a 60-second timeout per PUT and a 120-second total transfer deadline; each Apple API segment has the read/request bounds below.

## Pagination

Collections return `Page<T> { items, next_cursor }`. Keep the original path and query and add the returned cursor to fetch the next page:

```rust,ignore
let query = Query::new().with("bundleId", "com.example.notes").with("limit", 50);
let first = client.apps(&query).await?;
if let Some(cursor) = first.next_cursor {
    let second = client.apps(&query.with("cursor", cursor)).await?;
}
```

Cursors expire after 15 minutes and bind the path, filters, fields, ordering, and limit. They work across clones of the same client, up to 100 pages. A newly constructed client has a new cursor key; SDK cursors are not interchangeable with hosted cursors or raw Apple cursors. Apple continuation links must keep the same origin and resource path. Missing filters in Apple's links are preserved locally. Repeated pages and incomplete bounded traversals return errors.

## Update an age rating

```rust,ignore
let rating = client.update_age_rating(
    "1234567890", "app_info_123",
    &serde_json::json!({"gambling": false, "ageRatingOverrideV2": "NONE"}),
).await?;
```

This is the local equivalent of the hosted age-rating POST. It validates a nonempty flat object against the same writable fields, enums, nullability, URL rules, and 8 KiB limit. Omitted fields stay unchanged; supported nullable fields can be cleared with null. The SDK verifies the app-info belongs to the app, resolves its age-rating declaration, and sends one typed Apple PATCH. Apple checks the key's write permission and the resource's editable state. The SDK uses the Apple key's permissions directly, without hosted read/write grants.

Writes are never retried, including authentication or throttle failures. An uncertain network outcome or malformed success response returns `write_outcome_unknown` with `retryable: false`. Read the age rating before deciding whether to submit another update. Dropping a write future after dispatch can also leave an uncertain outcome; the SDK cannot report an error after its future is dropped.

## Update Store metadata

`Client::update_metadata(path, &attributes).await` updates only these editable attributes on existing facade paths: `/apps/{appId}` (`contentRightsDeclaration`), `/apps/{appId}/versions/{versionId}` (`versionString`, `copyright`), `/apps/{appId}/app-infos/{appInfoId}/localizations/{locale}` (`privacyPolicyUrl`), and `/apps/{appId}/versions/{versionId}/localizations/{locale}` (`promotionalText`, `description`, `whatsNew`, `keywords`, `supportUrl`, `marketingUrl`). Every value must be a string; use `""` to clear a field. The method resolves the scoped resource and exact localization, sends one Apple PATCH containing only the supplied fields, and returns the merged flat resource.

Writes are never retried. A rejected write returns Apple's sanitized error; an uncertain network outcome or malformed success response returns `write_outcome_unknown` with `retryable: false`. Read the affected resource before deciding whether to retry.

## Errors and request bounds

Errors expose `status`, stable `code`, sanitized `message`, `retryable`, field `details`, and optional `retry_after` seconds. Raw Apple messages, request URLs, keys and tokens are excluded. Status values follow the facade's HTTP classifications, even though the caller receives a Rust error.

Each operation permits 25 seconds and 32 Apple attempts, with an eight-second attempt timeout, 4 MiB response limit and 16 MiB total body limit. Reads retry at most twice for network failures, timeouts, throttling and server errors. A rejected JWT is refreshed at most once per read. Short `Retry-After` delays are honored; delays over two seconds are returned to the caller. Parent and locale resolvers permit 20 pages. Redirects are disabled.

Removing the hosted hop does not remove the Apple lookups required by the facade. Scoped child reads and writes still prove ownership, and exact-locale reads traverse matching localizations to prove uniqueness. There is no private resource cache or shared coordination between independently constructed clients.

## Development

From the repository root:

```sh
npm run build:sdks
npm run check:sdks
cargo package --locked --manifest-path sdks/rust/Cargo.toml --allow-dirty
```

Generated response types, upload models, and bundled contracts come from the language-neutral `api/sdk-catalog.json` and its reviewed runtime manifests. The packaged crate is self-contained and requires neither Python nor the Worker. Tests use generated keys and a local mock Apple server; they do not access a real Apple account. The crate uses the repository's GPL-3.0-or-later license.

## Shared execution core

This crate is also the execution core for Python, TypeScript, Go, and Swift. Python's
native extension calls it directly; the other bindings use the shared host bridge in
`sdks/native`. They call these same read, metadata, version, build, and checkpointed
upload operations. The language-neutral
`api/sdk-catalog.json` selects the implemented contracts and generates response types
and resource methods. The hosted Worker retains its existing implementation and route set.

`HttpTransport` and `Client::with_transport` provide a trusted host boundary for language
bindings. The core prepares authentication and owns validation, retries, pagination,
size limits, write outcomes, and upload reconciliation. An adapter sends one request,
honors the supplied timeout and response-size limit, and never follows redirects or
retries. It receives signed Apple requests, so treat it as trusted application code.
Blob upload requests contain only Apple's supplied storage headers, without the API JWT.

The Rust, native bridge, and language binding suites consume the same operation scenarios from
`tests/sdk/scenarios.json`; `npm run generate:sdks` copies them into the packaged crate.
