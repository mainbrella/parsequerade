# Parsequerade for TypeScript

A Node.js 22+ ESM SDK that calls App Store Connect directly through the same Rust execution core as the Rust and Python SDKs. It supports all 22 reads, age-rating and metadata updates, version creation, build attachment, and resumable listing-image uploads. Its public methods return promises and generated TypeScript models.

Validation, ownership checks, authentication, pagination, retries, and write reconciliation execute in Rust. A small Node-API addon connects that core to JavaScript; polling, custom fetch calls, and checkpoint persistence keep the Node event loop available. The npm package has no runtime npm dependencies.

## Install from this checkout

The package is named `@parsequerade/sdk` and is not yet published to npm. Build it from the repository root:

```sh
npm ci
npm run build:sdk:typescript
```

Then add it to your application's dependencies:

```sh
npm install /path/to/parsequerade/sdks/typescript
```

For a self-contained install without a link to the checkout, create an npm tarball with `npm pack ./sdks/typescript --pack-destination /path/to/output` and install that tarball. Packing rebuilds the bundle, declarations, and native addon and requires Rust and a native linker. The tarball bundles the addon for the build host's OS and architecture; build and pack on each deployment target. Installed tarballs need no Rust compiler, Worker files, or runtime npm dependencies. Native packaging is verified on macOS arm64; other targets require their own build and verification.

## Connect and read

Use an App Store Connect **team API key**, its issuer ID, key ID, and the PEM contents of the downloaded `.p8` file. Credentials and JWT signing stay in your server process; there is no hosted connection or facade API key.

```ts
import { readFile } from "node:fs/promises";
import { Client } from "@parsequerade/sdk";

const client = await Client.create({
  issuerId: process.env.ASC_ISSUER_ID!,
  keyId: process.env.ASC_KEY_ID!,
  privateKey: await readFile(process.env.ASC_PRIVATE_KEY_PATH!, "utf8"),
});

const apps = await client.apps({ bundleId: "com.example.notes", limit: 50 });
for (const app of apps.items) {
  console.log(app.id, app.name);
}
```

`Client.create` validates the P-256 PKCS#8 key locally and makes no network requests. Reuse the client to share its JWT cache, four-read concurrency bound, and cursor key. Apple key permissions govern access directly, without hosted read/write grants. Call `client.close()` when finished to release its native resources and cancel active operations; later requests reject with `client_closed`. Garbage collection also releases idle clients.

The [runnable JavaScript example](examples/list-apps.mjs) reads the environment variables above and paginates through apps:

```sh
node sdks/typescript/examples/list-apps.mjs
```

## Resource methods

All reads accept an optional query object as their last argument. Query names use the facade vocabulary; array values can be a single string or a string array. Method-specific query types check fields and filter enums at compile time. Runtime validation also applies to JavaScript callers and `URLSearchParams`.

```ts
const versions = await client.versions("1234567890", {
  platform: ["IOS", "MAC_OS"],
  state: "PREPARE_FOR_SUBMISSION",
  fields: ["versionString", "platform"],
  limit: 25,
});

const locale = await client.versionLocalization(
  "1234567890", "version_123", "en-US", { fields: "description" },
);
console.log(locale.description);
```

| Method | Return type |
| --- | --- |
| `apps(query?)` | `Page<App>` |
| `app(appId, query?)` | `App` |
| `appInfos(appId, query?)` | `Page<AppInfo>` |
| `appInfo(appId, appInfoId, query?)` | `AppInfo` |
| `appInfoLocalizations(appId, appInfoId, query?)` | `Page<AppInfoLocalization>` |
| `appInfoLocalization(appId, appInfoId, locale, query?)` | `AppInfoLocalization` |
| `ageRating(appId, appInfoId, query?)` | `AgeRating` |
| `versions(appId, query?)` | `Page<Version>` |
| `version(appId, versionId, query?)` | `Version` |
| `versionLocalizations(appId, versionId, query?)` | `Page<VersionLocalization>` |
| `versionLocalization(appId, versionId, locale, query?)` | `VersionLocalization` |
| `reviewContact(appId, versionId, query?)` | `ReviewContact` |
| `screenshotSets(appId, versionId, locale, query?)` | `Page<ScreenshotSet>` |
| `screenshots(screenshotSetId, query?)` | `Page<Screenshot>` |
| `previewSets(appId, versionId, locale, query?)` | `Page<PreviewSet>` |
| `previews(previewSetId, query?)` | `Page<Preview>` |
| `placements(appId, versionId, locale, query?)` | `Page<LibraryPlacement>` |
| `libraryImage(libraryImageId, query?)` | `LibraryImage` |
| `builds(appId, query?)` | `Page<Build>` |
| `build(appId, buildId, query?)` | `Build` |
| `versionBuild(appId, versionId, query?)` | `Build` |
| `buildPrereleaseVersion(appId, buildId, query?)` | `PrereleaseVersion` |

All methods return promises. Responses keep the hosted facade's camel case field names and flatten Apple envelopes into ordinary objects. Optional properties reflect omitted or sparse attributes; explicit nulls, empty strings, false, and zero are preserved. Read enums remain strings so new Apple values can be returned; supported write and query enums are checked strictly.

`client.get<T>(path, query?)` is also available for implemented facade paths, such as `/apps/1234567890`. Its result defaults to `unknown`; the caller-supplied generic is a type assertion. Prefer the typed methods when possible. Hosted connection prefixes, arbitrary URLs, and proposed routes are unsupported.

## Pagination

Collections return `{ items, nextCursor }`. Reuse the original method, path parameters, and query, adding the returned cursor:

```ts
const query = { bundleId: "com.example.notes", limit: 50 };
let cursor: string | null = null;
do {
  const page = await client.apps({ ...query, ...(cursor ? { cursor } : {}) });
  for (const app of page.items) console.log(app.id, app.name);
  cursor = page.nextCursor;
} while (cursor);
```

Cursors expire after 15 minutes, bind the client and original request, and permit up to 100 pages. A newly created client has a new cursor key. SDK cursors are not interchangeable with hosted or Rust SDK cursors, or raw Apple cursors. Continuation links must retain Apple's origin and the expected resource path. Missing filters in Apple's links are preserved, and repeated pages are rejected.

## Update an age rating

```ts
const rating = await client.updateAgeRating("1234567890", "app_info_123", {
  gambling: false,
  ageRatingOverrideV2: "NONE",
  kidsAgeBand: null,
});
```

The update takes a nonempty flat `AgeRatingUpdate` object, with the same field allowlist, enums, nullability, URL rules, and 8 KiB limit as the Worker. Omitted fields stay unchanged; supported nullable fields can be cleared. Inputs are snapshotted before awaited lookups. The SDK verifies ownership, resolves the declaration ID, and sends one Apple PATCH. Apple checks key permissions and the resource's editable state.

Writes are never retried. A lost or malformed update response returns `write_outcome_unknown` with `retryable: false`. Read the age rating before deciding whether to submit another update.

## Metadata, versions, and builds

```ts
await client.updateMetadata("/apps/1234567890/versions/version_123", {
  copyright: "2026 Example Company",
});
const draft = await client.createVersion("1234567890", "IOS", "2.0");
const builds = await client.builds("1234567890", { platform: "IOS", prereleaseVersion: "2.0" });
await client.attachBuild("1234567890", draft.id, builds.items[0].id);
```

Metadata uses the catalog's writable fields for apps, versions, app-info localizations, and version localizations. Build attachment proves app ownership, validates draft and build eligibility, checks version and platform, sends one PATCH, and reads back to confirm. These writes share Rust's validation and uncertain-outcome behavior.

## Resumable listing images

`listingImageReferenceData()` returns Apple's live asset specifications. `uploadListingImage(request, png, checkpoint, persist)` uploads one PNG and creates its placement after verification. `png` accepts a `Uint8Array` or Node `Buffer`.

```ts
import { readFile, writeFile, rename } from "node:fs/promises";
import type { ListingImageUploadCheckpoint } from "@parsequerade/sdk";

// On resume, load the previously saved JSON into this object first.
const checkpoint: ListingImageUploadCheckpoint = {};
const receipt = await client.uploadListingImage({
  appId: "1234567890", versionId: "version_123", locale: "en-US",
  placementType: "PRODUCT_PAGE_HEADER_ASSET", fileName: "listing.png",
}, await readFile("listing.png"), checkpoint, async snapshot => {
  await writeFile("checkpoint.json.tmp", JSON.stringify(snapshot));
  await rename("checkpoint.json.tmp", "checkpoint.json");
});
console.log(receipt.imageId, receipt.placementId);
```

Use persistence appropriate to your storage's durability requirements. `persist` receives an independent snapshot and must finish saving it before resolving without a return value. A thrown error or rejected promise stops execution before the next write and is returned unchanged. The caller's checkpoint is updated even when the operation fails; mutating a persistence snapshot cannot change the native workflow. Concurrent uploads using the same checkpoint object reject with `checkpoint_in_use`.

Resume with the same request, PNG bytes, and saved checkpoint. If cancellation returns while a persistence callback is still running, the checkpoint remains busy until that callback settles, so an older save cannot race a resumed upload. An uncertain reservation never reserves again; uncertain commit and placement operations reconcile using reads. Upload URLs and headers are checked in Rust, chunks contain exact byte ranges, and upload requests never carry the Apple bearer token. Checkpoints exclude signed upload URLs and headers.

## Errors and bounds

```ts
import { ApiError } from "@parsequerade/sdk";

try {
  await client.apps();
} catch (error) {
  if (error instanceof ApiError) {
    console.error(error.status, error.code, error.message, error.details);
    console.error(error.retryable, error.headers["Retry-After"]);
  } else {
    throw error;
  }
}
```

`ApiError` exposes the shared core's facade errors, with sanitized messages and field details. Raw Apple error text, request URLs, and credentials are excluded. HTTP status classifications and `Retry-After` headers match the Worker. The SDK accepts an optional trusted `fetch` implementation in `Client.create` for custom transport or tests; it receives signed Apple requests and unsigned upload chunks. It must honor the supplied abort signal, perform one attempt, and preserve `redirect: "manual"`. The default uses Rust's HTTPS transport.

The shared runtime permits 25 seconds and 32 Apple attempts per operation, an eight-second attempt timeout, 4 MiB per response, and 16 MiB total bodies. Reads retry at most twice, refresh a rejected JWT at most once, and honor throttle delays up to two seconds. Parent and locale resolvers permit 20 pages. Redirects are disabled. Scoped operations still make the Apple lookups needed to prove ownership and resolve exact targets. PNG uploads accept up to 32 MiB and use bounded upload attempts in the shared Rust core.

## Development

From the repository root:

```sh
npm run generate:sdks       # regenerate all SDK artifacts
npm run build:sdk:typescript
npm run test:sdk:typescript # strict public type checks and mocked API tests
npm run check:sdks         # also validates and tests Rust
```

The generated types use `api/sdk-catalog.json` through `scripts/sdk_contracts.py`. The addon under `native/` delegates every operation to `sdks/native`, which executes `sdks/rust`. There are no Worker imports in the shipped SDK. Tests run the shared operation and upload scenarios, validate JWTs, exercise checkpoint recovery and cancellation, and install the packed addon outside the checkout. No real Apple account is contacted.
