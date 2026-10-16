# Parsequerade for Python

A synchronous Python 3.11+ SDK that calls App Store Connect directly. Python uses the same Rust core as the Rust SDK for all 22 reads, metadata and age-rating updates, version creation, build attachment, and resumable listing-image uploads. Validation, ownership checks, pagination, signing, retries, and write reconciliation have one implementation.

Credentials and JWT signing stay in your process. Installed wheels contain the native extension and need no Python runtime dependencies, Node.js, Worker source, or hosted connection.

## Install from this checkout

The package is named `parsequerade` and is not yet published to PyPI. Building from source requires a Rust toolchain and Cargo; pip installs the Maturin build backend automatically:

```sh
python3 -m venv .venv
source .venv/bin/activate
python -m pip install /path/to/parsequerade/sdks/python
```

Build a wheel for the current platform with `python -m pip wheel --no-deps ./sdks/python --wheel-dir /path/to/output`. Consumers installing a compatible wheel do not need Rust. A source distribution includes the Rust core and builds it during installation. Native wheels must be built for each supported operating system and architecture.

## Connect and read

Supply an App Store Connect **team API key**, its issuer ID, key ID, and the PEM contents of the downloaded `.p8` file from private application configuration:

```python
import os
from pathlib import Path
from parsequerade import Client

client = Client(
    issuer_id=os.environ["ASC_ISSUER_ID"],
    key_id=os.environ["ASC_KEY_ID"],
    private_key=Path(os.environ["ASC_PRIVATE_KEY_PATH"]).read_text(),
)

page = client.apps({"bundleId": "com.example.notes", "limit": 50})
for app in page["items"]:
    print(app["id"], app.get("name"))
```

Construction validates the P-256 PKCS#8 key locally without a network request. Reuse the client to share cached JWTs, cursor signing, and the four-request concurrency bound. Methods release the GIL while the Rust operation runs and can be called from multiple Python threads. In an async application, use `asyncio.to_thread`; cancelling the await does not cancel an already dispatched write. Let uploads finish and persist their checkpoints before shutting down.

The [runnable example](examples/list_apps.py) uses the same environment variables and handles pagination:

```sh
python sdks/python/examples/list_apps.py
```

## Resource methods

Every read takes an optional `query` as its last argument. Query keys and response fields retain camel case; Python method and path argument names use snake case.

| Method | Return type |
| --- | --- |
| `apps(query=None)` | `Page[App]` |
| `app(app_id, query=None)` | `App` |
| `app_infos(app_id, query=None)` | `Page[AppInfo]` |
| `app_info(app_id, app_info_id, query=None)` | `AppInfo` |
| `app_info_localizations(app_id, app_info_id, query=None)` | `Page[AppInfoLocalization]` |
| `app_info_localization(app_id, app_info_id, locale, query=None)` | `AppInfoLocalization` |
| `age_rating(app_id, app_info_id, query=None)` | `AgeRating` |
| `versions(app_id, query=None)` | `Page[Version]` |
| `version(app_id, version_id, query=None)` | `Version` |
| `version_localizations(app_id, version_id, query=None)` | `Page[VersionLocalization]` |
| `version_localization(app_id, version_id, locale, query=None)` | `VersionLocalization` |
| `review_contact(app_id, version_id, query=None)` | `ReviewContact` |
| `screenshot_sets(app_id, version_id, locale, query=None)` | `Page[ScreenshotSet]` |
| `screenshots(screenshot_set_id, query=None)` | `Page[Screenshot]` |
| `preview_sets(app_id, version_id, locale, query=None)` | `Page[PreviewSet]` |
| `previews(preview_set_id, query=None)` | `Page[Preview]` |
| `placements(app_id, version_id, locale, query=None)` | `Page[LibraryPlacement]` |
| `library_image(library_image_id, query=None)` | `LibraryImage` |
| `builds(app_id, query=None)` | `Page[Build]` |
| `build(app_id, build_id, query=None)` | `Build` |
| `version_build(app_id, version_id, query=None)` | `Build` |
| `build_prerelease_version(app_id, build_id, query=None)` | `PrereleaseVersion` |

```python
versions = client.versions("1234567890", {
    "platform": ["IOS", "MAC_OS"],
    "state": "PREPARE_FOR_SUBMISSION",
    "fields": ["versionString", "platform"],
    "limit": 25,
})
shots = client.screenshot_sets("1234567890", "version_123", "en-US")
builds = client.builds("1234567890", {
    "platform": "IOS", "prereleaseVersion": "2.0", "sort": "-uploadedDate",
})
```

Queries accept a mapping or a sequence of `(name, value)` pairs. Values can be strings, integers, booleans, or string lists; selections can also be comma-separated strings. Unknown keys, duplicate pairs, unsupported fields, invalid enums, and invalid limits fail locally. Per-route `TypedDict` query types are exported for annotations.

Responses are ordinary dictionaries with generated `TypedDict` annotations, including nested image assets and upload operations. Missing attributes stay absent; explicit nulls, empty strings, false, and zero are preserved. Read enums remain strings so new Apple values can be returned. `client.get(path, query=None)` reads implemented relative facade paths such as `/apps/123`. Arbitrary URLs and hosted connection prefixes are unsupported.

## Pagination

Collections return `{"items": [...], "nextCursor": str | None}`. Keep the original query and path arguments, adding the cursor:

```python
query = {"bundleId": "com.example.notes", "limit": 50}
while True:
    page = client.apps(query)
    for app in page["items"]:
        print(app["id"])
    if page["nextCursor"] is None:
        break
    query = {**query, "cursor": page["nextCursor"]}
```

Cursors last 15 minutes, bind this client and the original request, and permit up to 100 pages. They cannot be exchanged with other clients or hosted cursors. Apple continuation links must retain the origin, path, and query; missing filters are preserved. Repeated pages fail. Parent and exact-locale resolvers permit 20 pages and fail if the result is incomplete or ambiguous.

## Updates, versions, and builds

```python
rating = client.update_age_rating("1234567890", "app_info_123", {
    "gambling": False,
    "ageRatingOverrideV2": "NONE",
    "kidsAgeBand": None,
})
localization = client.update_metadata(
    "/apps/1234567890/versions/version_123/localizations/en-US",
    {"description": "Updated description", "keywords": "notes,writing"},
)
draft = client.create_version("1234567890", "IOS", "2.0")
version = client.attach_build("1234567890", draft["id"], "build_123")
```

`update_metadata(path, attributes)` supports the reviewed editable fields on apps, versions, app-info localizations, and version localizations. Updates validate and snapshot a nonempty flat dictionary before lookups, preserve omitted fields, and verify ownership and exact locale selection. Age-rating updates use their separate reviewed field allowlist, enums, nullability, URL rules, and 8 KiB limit.

Version creation validates the app, platform, and version number before one POST. Build attachment proves the version and build belong to the app, checks draft state and build eligibility, matches platform and version, sends one PATCH, and confirms the selected build through a read.

Writes are never retried. A lost or malformed success response raises `write_outcome_unknown` with `retryable=False`. Read the affected resource before deciding whether to submit another write. Apple key permissions and editable states apply directly.

## Resumable listing-image uploads

`upload_listing_image(request, png, checkpoint, persist)` uploads one complete PNG and creates a listing-image placement for an exact version locale. Supported placement types are `PRODUCT_PAGE_HEADER_ASSET` (3840 × 1646) and `APP_STORE_SEARCH_RESULTS_ASSET` (1920 × 1280). The SDK validates the file against Apple's live reference data, sends only validated upload chunks, commits once, and verifies the placement. It refuses to replace an existing placement.

The checkpoint is a mutable dictionary. Load it from durable storage before the call and pass a persistence callback that saves every supplied snapshot before returning `None`. For example, using a SQLite transaction:

```python
import json
import sqlite3

with sqlite3.connect("listing-upload.sqlite3") as store:
    store.execute("CREATE TABLE IF NOT EXISTS checkpoint (id TEXT PRIMARY KEY, value TEXT NOT NULL)")
    row = store.execute("SELECT value FROM checkpoint WHERE id = ?", ("header-en-US",)).fetchone()
    checkpoint = json.loads(row[0]) if row else {}

    def persist(snapshot):
        with store:
            store.execute(
                "INSERT OR REPLACE INTO checkpoint (id, value) VALUES (?, ?)",
                ("header-en-US", json.dumps(snapshot)),
            )

    receipt = client.upload_listing_image({
        "appId": "1234567890", "versionId": "version_123", "locale": "en-US",
        "placementType": "PRODUCT_PAGE_HEADER_ASSET", "fileName": "header.png",
    }, Path("header.png").read_bytes(), checkpoint, persist)
    print(receipt["imageId"], receipt["placementId"])
```

The persistence callback runs on the calling thread and receives an independent dictionary snapshot. Its original exception propagates and prevents the next write. The passed checkpoint receives the latest state even when an operation fails. Checkpoints contain identifiers and request binding, excluding signed upload URLs and headers. Keep the same request, PNG, and saved checkpoint when resuming; serialize access to each stored checkpoint, including across processes. Reentrant or concurrent use of the same dictionary is rejected.

An uncertain reservation blocks another reservation. Uncertain commits and placements are reconciled through reads without repeating those writes. If reconciliation still raises `write_outcome_unknown`, retain the checkpoint. `listing_image_reference_data()` returns Apple's JSON:API reference-data envelope for diagnostics.

## Errors and bounds

```python
from parsequerade import ApiError

try:
    client.apps()
except ApiError as error:
    print(error.status, error.code, error.message)
    print(error.retryable, error.retry_after, error.details)
```

Errors use stable facade codes and sanitized messages; raw Apple errors, URLs, and credentials are excluded. Reads retry at most twice, refresh a rejected JWT once, and honor throttle delays of up to two seconds. Redirects are disabled. Ordinary operations have a 25-second budget, 32 Apple attempts, an eight-second timeout per attempt, 4 MiB per response, and 16 MiB of total bodies. Upload transfers have a separate 120-second budget and up to 60 seconds per chunk.

For a custom transport, pass a trusted callable as `transport=`. It receives `(method, url, headers, body_bytes_or_none, timeout_seconds)` and returns `parsequerade.Response(status, headers, body_bytes)`. Headers are case-insensitive. The callback runs on a worker thread; it must bound body reading and honor the timeout. Apple requests contain a JWT; blob uploads contain only Apple's validated upload headers. Default HTTP uses the Rust core's TLS client. Calling SDK methods from a transport or persistence callback raises `RuntimeError`; return control to the original operation before starting another call.

## Development

From the repository root, activate a virtual environment with Rust available and run:

```sh
python -m pip install -e 'sdks/python[dev]'
npm run generate:sdks
npm run test:sdk:python
```

Maturin builds the native extension. `npm run test:sdk:python` rebuilds the editable package before running tests; rebuild/reinstall after Rust changes when running Python directly. `cryptography` is a test dependency used only to verify generated JWT signatures. Generated types use the shared SDK catalog, and Python and Rust execute the same operation scenarios from `tests/sdk/scenarios.json`. Upload integration tests cover exact chunks, durable checkpoint callbacks, uncertain writes, and read-only recovery. Package tests install both wheels and source distributions outside the checkout without Python runtime dependencies. No tests contact Apple. The package uses GPL-3.0-or-later.
