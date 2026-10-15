# Parsequerade for TypeScript

A Node.js 22+ ESM SDK that runs the Parsequerade facade in your process and calls App Store Connect directly. It covers all 12 implemented reads and the scoped age-rating update, with typed flat responses, query validation, exact-locale lookup, ownership checks, and opaque pagination. It has no runtime dependencies.

The SDK bundles the Worker's existing transport, JWT signing, pagination, resolvers, errors, and write validation. Its types and resource methods are generated from the same reviewed contracts as the Rust SDK. Facade behavior is maintained in one JavaScript implementation.

## Install from this checkout

The package is named `@parsequerade/sdk` and is not yet published to npm. Build it from the repository root:

```sh
npm ci
npm run build:sdks
```

Then add it to your application's dependencies:

```sh
npm install /path/to/parsequerade/sdks/typescript
```

For a self-contained install without a link to the checkout, create an npm tarball with `npm pack ./sdks/typescript --pack-destination /path/to/output` and install that tarball. Packing rebuilds the bundle and declarations. Consumers need neither Python nor this repository's Worker files or development dependencies.

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

`Client.create` validates the P-256 PKCS#8 key locally and makes no network requests. Reuse the client to share its JWT cache, four-read concurrency bound, and cursor key. Apple key permissions govern access directly, without hosted read/write grants.

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

Writes are never retried. A lost or malformed update response returns `write_outcome_unknown` with `retryable: false`. Read the age rating before deciding whether to submit another update. Other writes, reports, copy workflows, and proposed read routes are not implemented.

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

`ApiError` is the shared facade error class, with sanitized messages and field details. Raw Apple error text, request URLs, and credentials are excluded. HTTP status classifications and `Retry-After` headers match the Worker. The SDK accepts an optional trusted `fetch` implementation in `Client.create` for custom transport or tests; it receives the signed requests. The default uses the runtime's native fetch.

The shared runtime permits 25 seconds and 32 Apple attempts per operation, an eight-second attempt timeout, 4 MiB per response, and 16 MiB total bodies. Reads retry at most twice, refresh a rejected JWT at most once, and honor throttle delays up to two seconds. Parent and locale resolvers permit 20 pages. Redirects are disabled. Scoped operations still make the Apple lookups needed to prove ownership and resolve exact targets.

## Development

From the repository root:

```sh
npm run generate:sdks       # regenerate Rust and TypeScript artifacts
npm run build:sdk:typescript
npm run test:sdk:typescript # strict public type checks and mocked API tests
npm run check:sdks         # also validates and tests Rust
```

The generated model selection and contract inputs are shared in `scripts/sdk_contracts.py`. `npm run check` rejects generated-file drift and tests the TypeScript SDK alongside the Worker. Tests share the Worker's resource fixtures and verify signed JWTs; no real Apple account is contacted.
