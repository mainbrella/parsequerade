# Parsequerade

Parsequerade is an operator-deployed JSON REST facade for App Store Connect, running on Cloudflare Workers alongside a static documentation site. Its core reads turn Apple's resource envelopes, type identifiers, and relationship links into ordinary JSON and predictable resource URLs.

The goal is to let a client list apps, edit a localized description, update a review contact, or read sales data without signing Apple JWTs, looking up internal child IDs, or assembling relationship payloads.

**Status:** The read-only first slice is implemented: 12 GET routes for apps, app infos, versions, their localization collections and exact locales, age ratings, and review contacts. Authentication, Apple JWTs, ownership checks, bounded pagination, and structured errors are included. Writes, reports, copy workflows, and broader reads remain planned. Setup and deployment require your own Apple team key and Cloudflare account; no authenticated Apple calls or deployment have been performed as part of implementation. See [rest.md](rest.md) for the remaining milestones.

## Configure and run

Use Node.js 22 or newer and Python 3. Install dependencies with `npm ci`, then configure an Apple team key and the app IDs this client may read:

```sh
npm run configure -- \
  --connection conn_main \
  --issuer-id YOUR_ISSUER_UUID \
  --key-id YOUR_KEY_ID \
  --private-key /path/to/AuthKey.p8 \
  --app-id YOUR_APP_ID

npm run dev
```

Repeat `--app-id` for multiple apps, or explicitly use `--all-apps`. Setup validates a P-256 PKCS#8 key locally and generates three private, gitignored files with mode `0600`:

- `.dev.vars`: local Worker configuration and secrets, loaded by Wrangler.
- `.parsequerade.secrets.json`: the same configuration for deployment.
- `.parsequerade-client-key`: the newly issued facade bearer key. The Worker configuration contains only its SHA-256 hash.

The command refuses to overwrite existing files and makes no network requests. Set `ASC_FACADE_KEY` from the client-key file, then read an app:

```sh
ASC_FACADE_KEY="$(cat .parsequerade-client-key)"
BASE='http://localhost:8787/v1/connections/conn_main'

curl --fail-with-body "$BASE/apps?limit=50" \
  -H "Authorization: Bearer $ASC_FACADE_KEY"
```

For deployment, update the custom domain in `wrangler.jsonc`, authenticate Wrangler to your Cloudflare account, run `npm run check`, then run:

```sh
npm run deploy -- --secrets-file .parsequerade.secrets.json
```

The existing configuration serves `/` and `/docs/` as static assets and sends `/v1/*` to `worker/index.js`. API data and errors use `Cache-Control: private, no-store`; private configuration belongs outside `dist/`. Missing configuration returns `503 service_not_configured`, while the static site remains available.

Configuration consists of `APPLE_CONNECTIONS_JSON` (connection ID → issuer ID, key ID, credential revision, and private-key secret name), separately named `APPLE_PRIVATE_KEY` or `APPLE_PRIVATE_KEY_*` secrets, `FACADE_API_KEYS_JSON` (key IDs, hashes, and per-connection capability/app grants), and a 32-byte base64url `CURSOR_SIGNING_KEY`. The generated JSON file is an editable example. Grants accept `appIds: "*"` or an explicit list of up to 200 app IDs; an empty list grants no apps. All four values are deployed as Worker secrets.

Revoke a facade key by removing its record or setting `revoked: true`, then publish the updated secrets. Issue a replacement with a new random key and hash. To rotate an Apple key, update its issuer/key ID and private-key secret and increment `credentialRevision`; this invalidates cached JWTs and old cursors. Rotating `CURSOR_SIGNING_KEY` also invalidates existing cursors. The setup helper provisions one connection and client; additional connections and grants can be added explicitly to the configuration.

Known resource routes currently accept GET only. Mutation methods return `405`; planned routes return JSON `404 route_not_found`. There is no public connection-administration endpoint or arbitrary upstream URL proxy. Permission checks happen before resource reads, and app-scoped list requests constrain Apple's `filter[id]` rather than filtering an unbounded account crawl.

Each facade request permits 25 seconds and 32 Apple attempts, with an 8-second attempt timeout, 4 MiB response limit, and 16 MiB total body limit. Safe reads retry up to twice within that budget, respect short `Retry-After` delays, and refresh a rejected cached JWT at most once. A long throttle delay is returned to the caller. Locale/ownership resolvers permit 20 pages. Cursors expire after 15 minutes, permit 100 pages, and bind the caller/key grant, Apple credential revision, path, filters, fields, ordering, and limit. Concurrency is bounded to four Apple reads per connection **within each isolate**; shared connection-wide coordination is deferred to the Durable Object phase. No private resource cache is used in this slice.

## Verify

`npm run check` validates documentation, generated runtime/OpenAPI contracts, and JavaScript, then runs unit/security tests and the bundled Worker in local `workerd` via Miniflare. Tests use fictional fixtures and generated test keys; all outbound Apple requests are mocked. The deployment build uses `--dry-run`, so checks do not publish anything. Real-account verification is a separate, explicitly configured read-only check.

## Documentation site

The static site lives in `dist/`; `/` is the Parsequerade landing page and the documentation starts at `/docs/`. Run `npm install` once, then use `npm run dev` to preview it locally. `npm run deploy` publishes through the existing Wrangler configuration.

The GET reference covers all **457 GET endpoint pages** reached through Apple’s [App Store documentation topic tree](https://developer.apple.com/documentation/appstoreconnectapi/app-store), organized into 84 topics. Pages identify implementation status separately from delivery stage; documentation coverage is broader than runtime coverage. Additional resource reads are extended designs, and older resource shapes have separate proposed `/legacy` routes. The [facade OpenAPI document](dist/docs/openapi.json) covers only the 12 implemented routes.

- `npm run build:docs` exports `api/read-contracts.json` and the facade OpenAPI document, then regenerates the reference and guides offline from the reviewed Apple snapshot, facade mappings, editorial overrides, and guide sources. `api/read-routes.json` explicitly selects implemented adapters.
- `npm run check` verifies local links, endpoint coverage, examples, contract drift, syntax, API behavior, and local Worker execution. Miniflare is a test-only dependency; the Worker adds no runtime packages.
- `npm run sync:docs` explicitly downloads Apple’s current topic pages and OpenAPI specification and replaces the source snapshot. Review source changes before rebuilding. The current snapshot records OpenAPI 4.5.1 and the source checksums; ordinary builds make no network requests.

## Why a facade?

Apple already provides a REST API that returns JSON. The difficulty is its resource and relationship model. For example, even Apple's [List apps endpoint](https://developer.apple.com/documentation/appstoreconnectapi/get-v1-apps) asks readers to understand query parameters such as `fields[apps]`, which can mix ordinary app attributes with related resources. The reference makes routine app-listing tasks harder to discover because readers must first learn Apple's resource graph; it does not mean every listed field is required or that every resource can be changed.

**Before: Apple's API documentation**

![Apple's List apps API documentation showing the long fields[apps] parameter list of app attributes and related resources.](docs/images/app-store-connect-api-docs.png)

**After: Parsequerade concept**

![Parsequerade concept showing Apple's raw API alongside a simplified JSON facade with flat responses and locale-aware endpoints.](docs/images/parsequerade-after.png)

An app's metadata also spans several resources. App names and privacy URLs belong to app-info localizations. Descriptions and release notes belong to version localizations. A review contact is another resource associated with a version. Editing one field can require finding the right parent, following a relationship, resolving a child ID, and then constructing a typed update body.

Parsequerade moves that translation into one service. Clients work with app IDs, explicit app-info or version IDs, locale strings, and ordinary JSON fields.

The planned shape has some overlap with [applelink](https://github.com/tramlinehq/applelink), which already offers flat JSON and multi-request release recipes. Parsequerade's design focuses on server-owned Apple credentials, explicit resource targets, and durable plans and results that callers can inspect.

## Before and after

These examples use fictional values and show only selected fields.

### List apps

Apple request:

```http
GET https://api.appstoreconnect.apple.com/v1/apps?filter[bundleId]=com.example.notes
```

Apple response shape:

```json
{
  "data": [
    {
      "type": "apps",
      "id": "1234567890",
      "attributes": {
        "name": "Example Notes",
        "bundleId": "com.example.notes",
        "primaryLocale": "en-US"
      },
      "relationships": {
        "appStoreVersions": {
          "links": {
            "related": "https://api.appstoreconnect.apple.com/v1/apps/1234567890/appStoreVersions"
          }
        }
      }
    }
  ]
}
```

Proposed Parsequerade request, relative to a configured connection:

```http
GET /apps?bundleId=com.example.notes
```

Proposed response:

```json
{
  "items": [
    {
      "id": "1234567890",
      "name": "Example Notes",
      "bundleId": "com.example.notes",
      "primaryLocale": "en-US"
    }
  ],
  "nextCursor": null
}
```

The client can read `response.items[0].name` directly. Single-resource reads return a plain object; collections use `items` and `nextCursor`.

### Edit a localized description

First, list versions with `GET /apps/{appId}/versions` and choose the intended version. Then address its locale directly:

```sh
BASE='https://asc.example.com/v1/connections/conn_example'

curl -X PATCH "$BASE/apps/1234567890/versions/version_123/localizations/en-US" \
  -H "Authorization: Bearer $ASC_FACADE_KEY" \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: description-change-001' \
  -d '{"description":"Keep your notes organized.","supportUrl":"https://example.com/support"}'
```

`asc.example.com` is a placeholder for your future deployment. The service would resolve `en-US` to the Apple localization ID, check its ownership, and send Apple's required payload internally:

```json
{
  "data": {
    "type": "appStoreVersionLocalizations",
    "id": "localization_456",
    "attributes": {
      "description": "Keep your notes organized.",
      "supportUrl": "https://example.com/support"
    }
  }
}
```

Clients would receive the updated flat localization object. A missing locale returns `404 localization_not_found`; creating a locale is an explicit `POST`.

## The proposed API

Each Apple connection has a base URL:

```text
https://asc.example.com/v1/connections/{connectionId}
```

Requests use a facade API key. The service owns the configured Apple credentials and JWT lifecycle, and checks the caller's permissions for each connection and resource.

Routes below are relative to that base. This is a summary; [rest.md](rest.md#resource-routes) defines the full planned contract and delivery stages.

| Task | Proposed route |
| --- | --- |
| List or read apps | `GET /apps`, `GET /apps/{appId}` |
| Select app information | `GET /apps/{appId}/app-infos` |
| Read or edit an app-level locale | `GET`, `PATCH /apps/{appId}/app-infos/{appInfoId}/localizations/{locale}` |
| Select a version | `GET /apps/{appId}/versions?platform=IOS&state=PREPARE_FOR_SUBMISSION` |
| Read or edit a version-level locale | `GET`, `PATCH /apps/{appId}/versions/{versionId}/localizations/{locale}` |
| Read, create, or edit a review contact | `GET`, `POST`, `PATCH /apps/{appId}/versions/{versionId}/review-contact` |
| Read sales as JSON | `GET /sales-reports?frequency=monthly&period=2026-09&vendorNumber=12345678` |
| Preview a metadata copy | `POST /copy-plans` |
| Execute a previewed copy | `POST /copy-plans/{planId}/executions` |
| Inspect workflow progress | `GET /operations/{operationId}` |

Writes start with the specific fields listed in the plan. Reading a field does not make it writable. Uploads, submission, pricing, and broader TestFlight management need separate contract work.

## What the service handles

- **Resource translation.** Flatten `attributes`, build Apple's typed request envelopes, and translate supported relationship edits. For example, `{"primaryCategoryId":"UTILITIES"}` becomes a category relationship update.
- **Target selection.** Resolve localization and review-contact IDs internally. Require explicit app-info and version targets for writes; never silently pick the first version or substitute another locale.
- **Pagination.** Return bounded pages with opaque cursors. Clients pass `cursor` back to the facade instead of following Apple pagination URLs. Incomplete upstream traversal is an error.
- **Updates and errors.** Omitted PATCH fields stay unchanged. Clearing with `null` is allowed only for supported fields. Preserve valid empty strings, `false`, and `0`; reject unsupported fields. Return stable error codes and field-level details.
- **Sales decoding.** Download and decode Apple's compressed tabular reports into typed JSON rows. Preserve monetary precision as decimal strings and keep currencies separate. An unavailable report is an error, rather than zero sales.
- **Write tracking.** Require idempotency keys and record write attempts. If a dispatched write times out, expose its uncertain outcome for reconciliation instead of blindly repeating it.
- **Copy workflows.** Preview selected metadata changes before execution, then expose progress and per-step outcomes. Multi-request copies can partially succeed; cancellation does not undo completed writes.

Apple's permissions, validation rules, and editable states still apply. The facade gives clients a consistent way to address resources and inspect outcomes while keeping Apple's authentication, graph traversal, and report parsing inside the service.

## Plan

The design is for a self-hosted service: operators configure Apple credentials, while clients use a facade key scoped to a connection. Reads return data without changing Apple resources.

Writes should name the app, platform, version or app-info resource, and locale they target. The existing `POST /copy-plans` contract previews a selected metadata copy field by field, stores the resolved targets and proposed steps, and performs no Apple writes. Applying that stored plan revalidates it and executes only its approved steps. This preview/apply contract is currently specified for copies in [rest.md](rest.md#6-copying-metadata-is-a-workflow); previewing arbitrary supplied multi-locale changes is a possible extension whose request and response contract still needs to be designed there.

Execution records would persist each step's confirmed success, failure, or unknown outcome when a request may have reached Apple but its result cannot be confirmed. The overall result would distinguish partial success from a completed workflow. After a restart, dispatched steps must be reconciled before retrying, so callers can inspect what completed without treating a multi-request workflow as a transaction.
