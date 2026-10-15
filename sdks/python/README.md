# Parsequerade for Python

A Python 3.11+ SDK that runs the Parsequerade facade in your process and calls App Store Connect directly. It covers all 12 implemented reads and the scoped age-rating update, with flat typed dictionaries, query validation, exact-locale selection, ownership checks, and opaque pagination.

It uses `cryptography` for ES256 signing and the standard library for HTTPS. Credentials and JWT signing stay in your process. No hosted connection, facade API key, Node.js, or Worker source is needed by installed consumers.

## Install from this checkout

The package is named `parsequerade` and is not yet published to PyPI:

```sh
python3 -m venv .venv
source .venv/bin/activate
python -m pip install /path/to/parsequerade/sdks/python
```

To distribute a self-contained wheel, run `python -m pip wheel --no-deps ./sdks/python --wheel-dir /path/to/output` from the repository root. Install the resulting wheel with pip; pip also installs the signing dependency.

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

Construction validates the P-256 PKCS#8 key locally and makes no network requests. Reuse the client to share cached JWTs, cursor signing, and the four-request concurrency bound. Methods are synchronous and can be called from multiple threads. In an async application, run them with `asyncio.to_thread`.

The [runnable example](examples/list_apps.py) uses the same environment variables and handles pagination:

```sh
python sdks/python/examples/list_apps.py
```

## Resource methods

Every read takes an optional `query` as its last argument. Query keys and response fields retain the facade's camel case spelling; Python method and path argument names use snake case.

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

```python
versions = client.versions("1234567890", {
    "platform": ["IOS", "MAC_OS"],
    "state": "PREPARE_FOR_SUBMISSION",
    "fields": ["versionString", "platform"],
    "limit": 25,
})
localization = client.version_localization(
    "1234567890", "version_123", "en-US", {"fields": "description"},
)
```

Queries accept a mapping or a sequence of `(name, value)` pairs. Values can be strings, integers, booleans, or string lists; selections can also be comma-separated strings. Unknown keys, duplicate pairs, unsupported fields, invalid enums, and invalid limits fail locally. Per-route `TypedDict` query types are exported for annotations; runtime validation applies to every call.

Responses are ordinary dictionaries with generated `TypedDict` annotations. Missing attributes stay absent. Explicit nulls, empty strings, false and zero are preserved. Read enums remain strings so new Apple values can be returned. `client.get(path, query=None)` returns raw flat JSON for implemented relative facade paths such as `/apps/123`. Arbitrary URLs and hosted connection prefixes are unsupported.

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

Cursors last 15 minutes, bind this client and the original request, and permit up to 100 pages. They cannot be exchanged with hosted or other SDK cursors. Apple continuation links must retain the origin, path, and query; missing filters are preserved. Repeated pages fail. Parent and exact-locale resolvers permit 20 pages and fail if the result is incomplete or ambiguous.

## Update an age rating

```python
rating = client.update_age_rating("1234567890", "app_info_123", {
    "gambling": False,
    "ageRatingOverrideV2": "NONE",
    "kidsAgeBand": None,
})
```

Updates validate and snapshot a nonempty flat `AgeRatingUpdate` dictionary before lookups. The SDK enforces the reviewed field allowlist, enums, nullability, URL rules, and 8 KiB limit; verifies ownership; resolves the declaration ID; then sends one Apple PATCH. Omitted fields stay unchanged and supported nullable fields can be cleared with `None`. Apple key permissions and editable states apply directly.

Writes are never retried. A lost or malformed success response raises `write_outcome_unknown`, with `retryable=False`. Read the age rating before deciding whether to resubmit. Other writes, reports, copy workflows, and proposed routes are unsupported.

## Errors and bounds

```python
from parsequerade import ApiError

try:
    client.apps()
except ApiError as error:
    print(error.status, error.code, error.message)
    print(error.retryable, error.retry_after, error.details)
```

Errors use stable facade codes and sanitized messages; raw Apple errors, URLs, and credentials are excluded. Reads retry at most twice, refresh a rejected JWT once, and honor throttle delays of up to two seconds. Redirects are disabled. Operations have a 25-second budget, 32 Apple attempts, an eight-second network timeout per attempt, 4 MiB per response, and 16 MiB of total bodies. System DNS resolution follows the platform's resolver timeout.

For custom transport, pass a trusted callable as `transport=`. It receives `(method, url, headers, body_bytes_or_none, timeout_seconds)` and returns `parsequerade.Response(status, headers, body_bytes)`. It must bound body reading and honor the timeout; it receives signed requests. Default HTTPS verifies TLS and opens a connection per attempt.

## Development

From the repository root, activate a virtual environment and run:

```sh
python -m pip install -e sdks/python
npm run generate:sdks
npm run test:sdk:python
```

Generated types and bundled contracts share the same reviewed inputs as the other SDKs. Tests use the Worker's resource fixtures and verify generated JWT signatures without contacting Apple. The package uses GPL-3.0-or-later.
