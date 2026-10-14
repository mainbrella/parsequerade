# App Store Connect JSON REST facade plan

Build a small HTTP service that owns Apple authentication, translates ordinary JSON into Apple's resource/relationship format, resolves child IDs, and converts sales reports into JSON. Developers should be able to list apps, edit a locale, update a review contact, and read sales without signing JWTs or traversing Apple's relationship graph.

Apple already exposes a REST API. The value of this facade is a smaller, predictable application contract: plain objects, explicit resource selection, consistent pagination and errors, and safe handling of operations that take several Apple requests.

This document combines the API contract and remaining implementation plan. The Cloudflare Worker now implements 12 core GET routes, connection/app-scoped authentication, Apple JWTs, ownership checks, pagination, and errors. All mutations, reports, copy workflows, and read-parity/extended adapters remain planned. The historical inventory covers calls found in Studio and `../../dev/DiscoNectar`, reviewed on 2026-10-14; its source links refer to those original integrations, not Worker files in this repository. No authenticated Apple calls or production deployment have been made during implementation.

## 1. What the projects actually do

All Apple paths in this section are relative to `https://api.appstoreconnect.apple.com`. Both integrations primarily use `/v1`; the facade's version must remain independent of Apple's endpoint versions.

| Capability | Apple requests observed | Source and behavior |
| --- | --- | --- |
| Authentication | Bearer JWT signed with ES256 using key ID, issuer ID, and a `.p8` private key | [Studio `apple.rs`](src/apple.rs), `generate_token`, uses a 20-minute token. Disco's [JWT generator](../../dev/DiscoNectar/DiscoNectar/util/JWTGenerator.swift) and [authentication manager](../../dev/DiscoNectar/DiscoNectar/util/AuthenticationManager.swift) cache tokens and probe `/v1/users?limit=1` during login. |
| Apps | `GET /v1/apps`, `GET /v1/apps/{id}` | Studio follows all pages at `limit=200`. Disco's [AppsAPIService](../../dev/DiscoNectar/DiscoNectar/api/AppsAPIService.swift) uses pages of four, then enriches each app sequentially and writes directly into its database. |
| App information and categories | `GET /v1/apps/{id}/appInfos`; primary/secondary category relationship links | Studio's [copy loader](src/apple_copy/api.rs) requests `include=primaryCategory`. Disco's [AppInfoAPIService](../../dev/DiscoNectar/DiscoNectar/api/AppInfoAPIService.swift) separately follows category relationship links. |
| App localizations | `GET /v1/appInfos/{id}/appInfoLocalizations` | App name, subtitle, and privacy metadata belong here, separately from version descriptions. Studio detail display keeps only English locales; its copy workflow loads all locales. Disco stores the first localization. |
| Age rating | `GET /v1/appInfos/{id}/ageRatingDeclaration` | Loaded by both projects. The declaration answers and computed rating are different things. |
| Versions | `GET /v1/apps/{id}/appStoreVersions` | [Studio details](src/apple/details.rs) filter to iOS and prefer an editable version, then live, then other states. Copy selects one version per platform. Disco's [AppStoreVersionsAPIService](../../dev/DiscoNectar/DiscoNectar/api/AppStoreVersionsAPIService.swift) uses the first version. |
| Version localizations | `GET /v1/appStoreVersions/{id}/appStoreVersionLocalizations` | Description, keywords, marketing/support URLs, promotional text, and release notes live here. Disco derives the related URL from a relationship URL and stores the first localization. |
| Review contact | `GET /v1/appStoreVersions/{id}/appStoreReviewDetail` | The read relationship is singular; the create/update resource type is plural `appStoreReviewDetails`. |
| Builds and icons | `GET /v1/builds`; version-to-build and build-to-icons related links | Disco's [BuildsAPIService](../../dev/DiscoNectar/DiscoNectar/api/BuildsAPIService.swift) retrieves only the first nine `PROCESSING` builds. Its version service follows build/icon relationships. Public artwork also comes from `itunes.apple.com`, a separate integration. |
| Users and bundle IDs | `GET /v1/users`, `GET /v1/bundleIds` | Disco's [UsersAPIService](../../dev/DiscoNectar/DiscoNectar/api/UsersAPIService.swift) and [AppStoreConnectService](../../dev/DiscoNectar/DiscoNectar/util/AppStoreConnectService.swift) paginate these; their page caps can terminate with partial results. |
| Expanded app inspection | `GET /v1/apps/{id}?fields[apps]=...&include=...`; arbitrary related-link reads | Disco's `fetchFullAppData` includes app info, versions, beta review/groups/license, builds, EULA, Game Center, IAPs, prerelease versions, promoted purchases, and review submissions. Inclusion is read access, not an implemented management workflow for each resource. |
| Sales | `GET /v1/salesReports` with `SALES`, `SUMMARY`, vendor, daily/monthly frequency, and period filters | [Studio report transport](src/apple_sales/api.rs), [parser](src/apple_sales/data.rs), and [period handling](src/apple_sales/period.rs); Disco's [SalesData.swift](../../dev/DiscoNectar/DiscoNectar/models/SalesData.swift). Downloads are compressed tabular data, not resource JSON. Both implementations aggregate reports for display. |

Existing mutation coverage comes from [Studio's copy planner](src/apple_copy/api.rs) and [Disco's AppsUpdateAPIService](../../dev/DiscoNectar/DiscoNectar/api/AppsUpdateAPIService.swift):

| Apple mutation | Fields/workflow already represented |
| --- | --- |
| `PATCH /v1/apps/{id}` | `contentRightsDeclaration` |
| `PATCH /v1/appInfos/{id}` | `primaryCategory` relationship |
| `PATCH /v1/appInfoLocalizations/{id}` | `privacyPolicyUrl` |
| `POST /v1/appInfoLocalizations` | Studio creates a missing locale with destination app name, locale, privacy URL, and destination app-info relationship. |
| `PATCH /v1/appStoreVersions/{id}` | Studio copies `copyright`; Disco updates `versionString` and optional `copyright`. |
| `POST /v1/appStoreReviewDetails`, `PATCH /v1/appStoreReviewDetails/{id}` | `contactFirstName`, `contactLastName`, `contactPhone`, `contactEmail`; creates require the version relationship. |
| `POST /v1/appStoreVersionLocalizations`, `PATCH /v1/appStoreVersionLocalizations/{id}` | Description, keywords, marketing/support URLs; Disco additionally supports `promotionalText` and `whatsNew`. Creates include locale and version relationship. |
| `PATCH /v1/ageRatingDeclarations/{id}` | Writable questionnaire fields and overrides. Studio has a deliberate field allowlist; do not copy an entire GET response into PATCH. |

Useful behavior to preserve:

- Studio validates Apple origins and pagination paths, rejects repeated pages, and disables redirects in [the transport](src/apple/api.rs).
- Its copy workflow loads and validates the plan before writing, matches platform and locale, stops after a failed write, and keeps completed changes. Cancellation does not undo writes.
- Its copy planner omits absent/empty source values while preserving `false`. That is a copy policy, not a general PATCH rule.
- Its report transport limits compressed and expanded sizes, handles gzip, separates proceeds by currency, and partitions its cache by issuer and vendor.

Behavior to replace:

- First-item selection and English-only display filters must not become API defaults. Return all accessible locales and require an unambiguous write target.
- Disco sometimes swallows relationship failures or returns a capped list without indicating incompleteness. Missing data and failed requests need distinct outcomes.
- Disco couples HTTP, database writes, notifications, and logging. Keep those client concerns outside the service's Apple adapter.
- Disco's [copy UI](../../dev/DiscoNectar/DiscoNectar/views/AppDetailView.swift) uses a hardcoded version string and catches individual copy errors. Do not carry that orchestration into the contract.
- Disco's generic related-URL fetcher accepts full URLs and logs bodies. A public facade must expose registered resources, not an authenticated arbitrary-URL proxy.

## 2. Public contract

Use a connection-scoped base URL:

```text
https://asc.example.com/v1/connections/{connectionId}
```

Every request uses `Authorization: Bearer <facade-api-key>`. A connection identifies a configured Apple account/key; possession of its ID grants no access. Check the caller's connection, app, and read/write/report permissions on every request. Keep connection administration separate from the resource routes.

Use camelCase JSON fields, string IDs, RFC 3339 timestamps, `application/json`, and Apple's existing enum strings. Keep names such as `bundleId`, `primaryLocale`, and `versionString` unless a deliberate mapping improves the contract. Clients never send `data.type`, `attributes`, `relationships`, or Apple's pagination URLs.

Single-resource responses are plain objects. Collections have one consistent envelope:

```json
{
  "items": [
    {
      "id": "1234567890",
      "name": "Example App",
      "bundleId": "com.example.app",
      "sku": "example-ios",
      "primaryLocale": "en-US",
      "contentRightsDeclaration": "DOES_NOT_USE_THIRD_PARTY_CONTENT"
    }
  ],
  "nextCursor": null
}
```

Return `200` for reads/updates, `201` plus `Location` for creates, and `202` plus an operation URL only for explicitly asynchronous workflows. An empty collection is `200` with `items: []`. Do not expose generic CRUD verbs where Apple or the facade does not support the operation.

### Resource routes

All routes below are relative to the connection base. `{appInfoId}` and `{versionId}` are Apple's IDs returned by list/read routes. Resolve localization and review-detail IDs internally. Validate each nested resource's ownership, including on direct-ID requests.

| Facade route | Purpose / Apple mapping | Delivery |
| --- | --- | --- |
| `GET /apps?bundleId=...&limit=50&cursor=...` | Flat app list; translate `bundleId` to `filter[bundleId]`. | First release |
| `GET /apps/{appId}` | Base app attributes, with no implicit relationship fan-out. | First release |
| `PATCH /apps/{appId}` | Initially accepts only `contentRightsDeclaration`. | First release |
| `GET /apps/{appId}/app-infos` | List app-info IDs, states, and category IDs. | First release |
| `GET /apps/{appId}/app-infos/{appInfoId}` | Read a specific app-info resource. | First release |
| `PATCH /apps/{appId}/app-infos/{appInfoId}` | `{ "primaryCategoryId": "UTILITIES" }` becomes a category relationship update. | First release |
| `GET /apps/{appId}/app-infos/{appInfoId}/localizations` | All app-level locales, paginated. | First release |
| `GET`, `PATCH /apps/{appId}/app-infos/{appInfoId}/localizations/{locale}` | Resolve locale to localization ID; initially PATCH privacy URL. | First release |
| `POST /apps/{appId}/app-infos/{appInfoId}/localizations` | Create using `locale`, required `name`, and optional privacy URL. | First release |
| `GET`, `PATCH /apps/{appId}/app-infos/{appInfoId}/age-rating` | Resolve the age-rating declaration; validate a writable field allowlist. | First release |
| `GET /apps/{appId}/versions?platform=IOS&state=PREPARE_FOR_SUBMISSION` | Translate supported filters; return all matching versions across pages. | First release |
| `GET`, `PATCH /apps/{appId}/versions/{versionId}` | Read selected version; PATCH `versionString` and/or `copyright`. | First release |
| `GET /apps/{appId}/versions/{versionId}/localizations` | All version-level locales, paginated. | First release |
| `GET`, `PATCH /apps/{appId}/versions/{versionId}/localizations/{locale}` | Resolve locale; edit the six listing fields in the mutation inventory. | First release |
| `POST /apps/{appId}/versions/{versionId}/localizations` | Create a locale; facade supplies the version relationship. | First release |
| `GET`, `POST`, `PATCH /apps/{appId}/versions/{versionId}/review-contact` | Four contact fields; facade resolves or creates `appStoreReviewDetails`. | First release |
| `GET /apps/{appId}/metadata?platform=IOS&stage=draft&locale=en-US` | Convenience read joining selected app info, version, and localizations. | After resource routes |
| `GET /apps/{appId}/builds?processingState=PROCESSING` | Map app/state filters; paginate without Disco's nine-item limitation. | Read parity |
| `GET /apps/{appId}/versions/{versionId}/build` | Read the version's assigned build, preserving the existing detail workflow. | Read parity |
| `GET /apps/{appId}/builds/{buildId}/icons` | Flatten build-icon resources; ownership checked through the build. | Read parity |
| `GET /users`, `GET /bundle-ids?identifier=...` | Connection-level paginated lists. | Read parity |
| `GET /sales-reports?frequency=monthly&period=2026-09&vendorNumber=...` | Retrieve one report as typed JSON rows; see report contract below. | Report milestone |
| `POST /copy-plans`, `POST /copy-plans/{planId}/executions` | Preview then execute an explicit metadata copy. | Workflow milestone |
| `GET /operations/{operationId}` | Inspect durable workflow/write-attempt outcomes. | Before mutation rollout |

This deliberately starts with observed writes. Name/subtitle editing, secondary-category updates, version creation, release/submission, uploads, pricing, TestFlight management, and financial/analytics reports need separate contract work. Read-only fields are not implicitly writable. The arbitrary expanded app browser is not an excuse to promise management of every included resource.

The static [GET reference](dist/docs/reference/index.html) now documents proposed read translations for all 457 GET pages reached through Apple’s App Store topic tree, including relationship-ID reads. The first-release and read-parity routes above retain their milestones. Other pages are labeled **Extended read design** or **Legacy read design** and do not add mutation coverage or imply an implemented adapter. Their route mappings, query allowlists, flat response schemas, and examples live in `scripts/docs_contracts.py`, with editorial overrides in `docs/facade-overrides.json` and a versioned source snapshot in `docs/apple-get-reference.json`. `/legacy` distinguishes older Apple resource shapes; it is independent of the facade’s API version. CSV-only code downloads and structured performance metrics have explicit read designs rather than being treated as ordinary resource JSON.

### Ordinary client examples

The base includes the connection once; all remaining URLs describe application resources.

```sh
BASE='https://asc.example.com/v1/connections/conn_example'

curl "$BASE/apps?bundleId=com.example.app" \
  -H "Authorization: Bearer $ASC_FACADE_KEY"

curl -X PATCH "$BASE/apps/1234567890/versions/version_123/localizations/en-US" \
  -H "Authorization: Bearer $ASC_FACADE_KEY" \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: listing-change-001' \
  -d '{"description":"A clearer description.","supportUrl":"https://example.com/support"}'
```

The second request resolves `(connection, app, version, en-US)` to an Apple localization ID and sends the following internally:

```json
{
  "data": {
    "type": "appStoreVersionLocalizations",
    "id": "localization_456",
    "attributes": {
      "description": "A clearer description.",
      "supportUrl": "https://example.com/support"
    }
  }
}
```

The response exposes the facade object, with `locale`, the supported listing fields, and its ID; it does not return this internal envelope. An absent locale produces `404 localization_not_found`; creation is an explicit POST. POSTing an existing locale or existing review contact produces `409 resource_exists`. Copy workflows may choose create versus update internally.

## 3. Selection, pagination, and update semantics

**Selection:** Explicit IDs are the canonical mutation targets. Validate the version's platform and parent app. Never select a version by sorting version strings or taking the first result. Use locale strings as exact keys; do not silently substitute the primary locale. Preserve Apple's platform values and support the documented platforms rather than treating all apps as iOS.

For the optional metadata convenience read, require `platform`, `stage`, and `locale`. Define `stage=draft` narrowly as `PREPARE_FOR_SUBMISSION`; define `stage=live` using the supported live-state aliases. This is a selection convenience, not a claim that only those states permit every edit. Return the resolved app-info and version IDs. No candidate yields `404 selection_not_found`; multiple candidates yield `409 ambiguous_selection` with candidate IDs. Do not silently switch from draft to live. Model `state`, `appVersionState`, and legacy `appStoreState` differences in the Apple adapter using schema-backed fixtures.

**Pagination:** Default to 50 items, with per-route maxima bounded by Apple's endpoint limits. A request returns one bounded page, not a hidden full-account crawl. Use opaque signed cursors bound to connection, route, filters, ordering, and expiry. Reauthorize each page. Preserve supported upstream ordering; do not sort individual pages and claim global sorting. Unsupported filters/sorts return `400`.

Internal resolvers sometimes need all pages to locate a locale or prove unique selection. Follow validated `links.next` only within the expected origin and collection path, detect loops, and enforce page/time/size budgets. Exceeding a budget is an explicit upstream/incomplete-result error, never a successful truncated result. If a route needs filtering that Apple cannot perform, fill the page with bounded traversal and a correct continuation or reject that filter; do not filter one page and declare the collection complete.

**PATCH:** Omitted fields remain unchanged. `null` clears only fields whose supported Apple update schema and endpoint behavior permit clearing. An empty string remains an empty string where valid; `false` and `0` are values. Reject unknown, computed, or read-only fields with `422`. Do not apply the copy planner's “skip empty source fields” policy to ordinary updates.

Separate create and update schemas. For example, creating an app-info localization requires a name and locale, while updating its privacy URL does not. Start with the supported fields in this plan even where Apple's schema permits more. Contact routes expose only the four contact fields, preserving unrelated demo-account credentials and review notes. Age-rating mutations use the current writable schema rather than a blanket object merge.

**Read composition:** Base resources must not recursively expand relationships. A metadata read has a fixed, documented set of sections and bounded concurrent requests. Fail a required section normally. If later offering `allowPartial=true`, include explicit per-section `available`, `missing`, or `error` states and structured warnings; `null` must not disguise a permission error. An absent optional singleton and a failed upstream request are different results.

**Concurrent changes:** Re-read target state and parent relationships before mutations; invalidate affected caches after confirmed writes. A stored snapshot hash can detect changes before a copy starts, but it cannot make Apple's multi-request workflow atomic or prevent edits made directly in App Store Connect between requests. Do not advertise strong `If-Match` or compare-and-swap guarantees without verified upstream support.

## 4. Authentication, transport, and errors

Provision Apple credentials through operator configuration. The implemented slice supports team keys, matching both repositories. Use Worker secrets in deployment and private `.dev.vars` files locally; keep them out of static assets, responses, and logs. Facade API keys are hashed and revoked through operator configuration. Each key has explicit connection, capability, and app grants. Hosted deployments require TLS; local Wrangler development still requires a token. See the README setup command for the exact configuration format.

Cache Apple JWTs per credential revision and refresh shortly before expiry. Use a conservative short lifetime within Apple's normal 20-minute limit, handle clock skew, and invalidate tokens after key rotation. Apple documents different claims for individual keys; add those explicitly later rather than accepting an issuer-less key in the team-key path. [Apple token documentation](https://developer.apple.com/documentation/appstoreconnectapi/generating-tokens-for-api-requests).

Do not treat `/users` access as the definition of a valid connection. Validate locally that credentials can sign, then probe only the capability being configured. A key may be useful for apps or reports while lacking user-management access. Capability probes should record success/denial/unknown separately, not require administrator permissions. Sales and provisioning capabilities also depend on key type. [Apple key types](https://developer.apple.com/documentation/appstoreconnectapi/creating-api-keys-for-app-store-connect-api).

Use one transport for resource JSON and a separate bounded binary/report path. Carry status, relevant headers, request IDs when available, and all structured Apple errors through an internal error type. Studio's current `(u16, Value)`/string-error interface loses headers needed for throttling and error classification.

Retain HTTPS origin checks, endpoint/path allowlists, redirect rejection, request timeouts, ID validation, and pagination-loop detection. Do not fetch arbitrary client-supplied URLs or automatically fetch image URLs returned in metadata. Check authorization before reading any cache, and partition caches, cursors, plans, jobs, and locks by tenant and connection/credential revision.

Return a stable error object:

```json
{
  "error": {
    "code": "validation_failed",
    "message": "One or more fields are invalid.",
    "requestId": "req_example",
    "retryable": false,
    "details": [
      {
        "field": "supportUrl",
        "code": "invalid_url",
        "message": "Provide a valid support URL."
      }
    ]
  }
}
```

| HTTP status | Facade meaning |
| --- | --- |
| `400` | Malformed JSON, invalid cursor, unsupported query or selector. |
| `401` | Missing/invalid facade credential. An Apple JWT failure is not this status. |
| `403` | Caller lacks a facade capability, or Apple denies the requested operation; distinguish `forbidden` from `upstream_forbidden`. |
| `404` | Requested resource/locale is absent or inaccessible under the caller's scope; use a specific code for an unavailable report. |
| `409` | Ambiguous target, existing resource, incompatible edit state, stale copy plan, or idempotency conflict. |
| `422` | Recognized request with invalid fields or a mapped Apple validation error. |
| `429` | Facade or Apple throttling; include a meaningful `Retry-After` when available. |
| `502` / `503` / `504` | Upstream credentials rejected, invalid upstream payload, service unavailable, or timeout, with distinct codes. |

Translate Apple's source pointers to facade field names, preserve useful upstream codes and all relevant validation errors, and sanitize upstream messages. Never echo credentials, demo passwords, or entire private response bodies. Include a request ID in response headers and logs.

Bound concurrency and request rate per Apple connection. Retry safe reads with capped exponential backoff and jitter for temporary failures, respecting `Retry-After` within the request budget. Refresh a rejected cached JWT at most once where safe. A valid facade token with invalid Apple credentials should receive `502 upstream_authentication_failed` and a connection-repair message.

For writes, require `Idempotency-Key` in the initial contract and persist an intent before contacting Apple. Scope keys by tenant, connection, caller, method, and route; hash the body and reject reuse with different content. Retain results for a documented period, initially 24 hours. Concurrent requests with the same key share one attempt. This prevents duplicate facade execution; it does not give Apple an idempotency guarantee.

A disconnect, unreadable success response, timeout, or process crash after dispatch may leave the write outcome unknown. Return a durable `operationId`, `mutationState: "unknown"`, and `retryable: false`; do not blindly replay. Reconcile with a fresh resource read and, for creates, the parent/locale uniqueness key. If evidence cannot establish the outcome, leave it unresolved for inspection. For a definitive rejection, preserve the error and permit a new corrected request. A `202` means the service accepted work, never that Apple completed it.

## 5. Sales reports as JSON

The first report endpoint supports the existing `SALES/SUMMARY` daily and monthly use cases. Require `frequency=daily|monthly`, an explicit `period` (`YYYY-MM-DD` or `YYYY-MM`), and a vendor number unless exactly one vendor is configured on the connection. Validate actual calendar periods, not just a regex. Do not silently substitute yesterday or the previous month.

Translate those parameters to Apple's filters and choose the supported report version explicitly. Both repositories send `YYYY-MM` for monthly upstream requests; verify the adapter's exact date encoding against current documentation and a captured successful response before release. The facade period format stays stable if Apple's encoding changes. Other report types/frequencies are later capabilities. [Apple sales report endpoint](https://developer.apple.com/documentation/appstoreconnectapi/get-v1-salesreports).

Download with bounded compressed/expanded sizes; Studio's existing 32 MiB/128 MiB limits are a reasonable starting policy. Detect gzip and support concatenated gzip members, parse UTF-8/BOM and tab-separated fields with a real parser, reject malformed/incomplete rows, and identify columns by header. These are report files, not ordinary JSON responses. [Apple Sales and Finance](https://developer.apple.com/documentation/appstoreconnectapi/sales-and-finance).

Return normalized rows before adding opinionated aggregation:

```json
{
  "report": {
    "type": "SALES",
    "subType": "SUMMARY",
    "frequency": "monthly",
    "period": "2026-09",
    "vendorNumber": "12345678",
    "fetchedAt": "2026-10-14T12:00:00Z",
    "cached": true
  },
  "items": [
    {
      "sku": "example-ios",
      "appleIdentifier": "1234567890",
      "parentIdentifier": null,
      "productTypeIdentifier": "1",
      "countryCode": "US",
      "units": 12,
      "developerProceedsPerUnit": "0.7000",
      "currencyOfProceeds": "USD"
    }
  ],
  "nextCursor": null
}
```

Preserve source precision using decimal strings for money and exact decimal arithmetic for totals. Studio currently parses proceeds as cents with at most two fractional digits; do not promote that limitation into the public contract. Keep signed units/refunds and product type identifiers. Never add different currencies together or equate every unit with a first-time download. Validate JSON integer range or use a documented exact representation if counts exceed it.

Cache a parsed report snapshot by connection, credential/authorization revision, vendor, report type/subtype/version, frequency, and period. Page over that immutable snapshot; bind the cursor to its content version so a refresh cannot mix pages. Expose `fetchedAt` and allow `refresh=true`; a refresh failure is an error, not an unlabeled stale success. Deduplicate simultaneous downloads. A bounded request that cannot finish returns an explicit timeout; it must not return half a report.

Offer later summaries as a separate route or explicit view, with documented grouping and the full-report snapshot as input. App filtering must use Apple identifiers and parent SKU relationships, following [Studio's filtering tests](src/apple_sales/tests.rs), rather than assuming a bundle ID appears in the report.

An Apple `404` becomes `report_unavailable`, not an empty report or zero revenue. It may mean a report has not been generated, no report qualifies, or the retention window has elapsed. Report availability is delayed; monthly data is not guaranteed on the first day of the next month. [Apple report availability](https://developer.apple.com/help/app-store-connect/reference/reporting/sales-and-trends-reports-availability/).

## 6. Copying metadata is a workflow

Use Studio's snapshot/plan/apply separation as the starting point, keeping its destination-only writes and explicit partial progress.

1. `POST /copy-plans` accepts `sourceAppId`, `destinationAppId`, explicit source/destination app-info and version IDs, locales, and a field-group allowlist. Start with same-connection copies. Reject self-copy, mismatched platforms, duplicate locale targets, and noneditable destination selections before any write.
2. Resolve parents and resources, load all necessary pages, validate all intended fields, and return a stored plan containing the proposed changes, creates, skipped fields, resolved IDs, snapshot hashes, and expiry. Building a plan writes no Apple data.
3. Copy only selected groups: content rights, primary category, privacy URLs, age answers, copyright, four review-contact fields, and selected listing fields. Exclude version numbers, build associations, names, demo credentials, release state, and submission actions by default. Preserve destination names when a new privacy localization needs a name. Never clear destination values merely because the source omits them.
4. `POST /copy-plans/{planId}/executions` with an idempotency key revalidates authorization, expiry, ownership, state, and snapshot hashes, then returns `202` and a durable operation URL. Reject changed plans with `409 stale_plan`; require a new preview rather than changing the approved field set.
5. Execute in a deterministic dependency order under a destination-app lock. Record every step before/after dispatch. Stop on failure, mark remaining steps skipped, and retain confirmed successful changes. Cross-process workers require a durable lease, not an in-memory mutex.
6. Expose `pending`, `running`, `succeeded`, `failed`, `partiallySucceeded`, `cancelled`, and `outcomeUnknown`, plus per-step results and request IDs. Success requires all intended writes to be confirmed. Restart recovery reconciles dispatched steps before considering a retry.
7. A later `POST /operations/{id}/cancellation` requests a stop before the next dispatch. It does not reverse completed or in-flight changes. Do not attempt automatic rollback; that could overwrite subsequent edits or itself fail.

Plans and operation results can contain private metadata. Authorize reads, redact sensitive values from logs, define retention, and expire artifacts. A workflow API should make partial results inspectable without claiming a multi-resource transaction that Apple does not provide.

## 7. Implementation shape and rollout

Use the Cloudflare Worker in this repository alongside the existing static assets. `wrangler.jsonc` deploys Workers Static Assets and routes `/v1/*` through `worker/index.js`; `/` and `/docs/` retain static hosting. The facade base is the deployed domain plus `/v1/connections/{connectionId}`. The Worker uses small JavaScript modules and native Web Crypto, without runtime dependencies.

```text
Worker HTTP routes + facade auth + query validation
                   |
resource services / selectors / copy planner
                   |
Apple transport + JSON adapter + report decoder
                   |
App Store Connect

Now: Worker secrets and isolate-local JWT/concurrency state
Before writes: SQLite-backed Durable Object per connection
Reports: bounded streaming and immutable snapshots, R2 for larger artifacts
```

Implemented modules separate facade authentication, ES256 JWTs/Apple transport, query validation, signed cursors, resource selection, and response normalization. `api/read-routes.json` selects 12 adapters; `scripts/export_api.py` derives their reviewed query and response contracts from the same snapshot/mappings used by the documentation, producing `api/read-contracts.json` and a facade OpenAPI document. Builds are offline. The broader 457-page reference labels implementation status separately from delivery stage.

The read slice has no resource cache or durable mutation state. JWT reuse and a four-read connection concurrency limit are isolate-local, not a globally enforced Apple quota. Apple reads share 25 seconds, 32 attempts, an 8-second attempt timeout, 4 MiB per-response and 16 MiB total-body limits. Internal resolvers permit 20 pages. Signed client cursors expire after 15 minutes and permit 100 pages; limits fail explicitly rather than returning an incomplete result as complete. Read-only verification uses generated keys and fictional Apple fixtures in unit tests and the local workerd runtime.

Before enabling any mutations, add one SQLite-backed Durable Object per connection to hold write intents, idempotency records, operations, and later copy plans, and coordinate shared request limits. Persist intent and dispatch state before contacting Apple; a dispatched but unconfirmed write is reconciled, not replayed automatically. Use explicit destination-app locking shared by ordinary writes and copy execution, including across async network calls. Durable Object alarms provide restart recovery and background work; their at-least-once delivery must inspect durable step state before dispatching. D1 and an external queue are unnecessary for the initial operator deployment.

For reports, the original 128 MiB expanded-report allowance cannot be buffered in a Worker with a 128 MB isolate memory limit. Stream decoding/parsing into bounded immutable report snapshots, and use R2 for larger artifacts. Verify concatenated gzip behavior and monthly date encoding with captured test fixtures before enabling that route.

| Milestone | Deliverable | Acceptance gate |
| --- | --- | --- |
| 1. Contract and fixtures — implemented for reads | Facade OpenAPI, runtime contract manifest, query/field/method allowlists, fictional read fixtures; pinned Apple schema version/hash. | Generated contracts stay in sync with documentation; mutation fixtures remain future work. |
| 2. Transport and read routes — implemented | Connection/app-scoped auth, JWT lifecycle, safe pagination, app/app-info/version/localization/age-rating/review-contact reads, structured errors. | Unit/security and workerd fixture tests pass; an operator must still configure a real account and perform read-only integration verification. |
| 3. Ordinary mutations | Durable write intents/idempotency, observed PATCH/POST routes, validation and cache invalidation. | Supported edits and missing-localization creation work; ambiguous timeout/create recovery does not duplicate writes. |
| 4. Read parity and reports | Users, bundle IDs, builds/icons, fixed metadata read, daily/monthly normalized reports. | Both clients can replace their supported reads; errors and unavailable reports remain distinguishable from empty data. |
| 5. Copy workflow | Preview records, execution worker, progress, restart reconciliation, cancellation. | Destination-only writes, partial failure, stale plans, duplicate execution, and cancellation behave as specified. |
| 6. Client migration | Studio and Disco use the facade behind an explicit configuration switch. | Matching output for representative accounts and the same editing/copy workflows, with no Apple JWT or relationship traversal in clients. |

Migrate reads first and compare normalized results using sanitized fixtures or an explicitly configured test account. Keep an explicit direct/facade mode during rollout, but never automatically fall back to direct Apple writes after a facade error: that can duplicate a successful but unconfirmed mutation. Replace Disco's database synchronization with consumption of facade objects; retain UI caching/notifications in the app. Migrate Studio's copy progress display to operation polling. Remove duplicated Apple code only after parity is demonstrated.

## 8. Verification and schema maintenance

Use [Studio copy tests](src/apple_copy/api_tests.rs), [detail tests](src/apple/details/tests.rs), and report/parser tests as behavioral fixtures. Extend them at the service boundary rather than replacing them with tests that only check serialization.

- Contract tests: flat resources, all supported routes and fields, create/update differences, null/empty/false behavior, unknown fields, category relationship translation, and localized error pointers.
- Selection tests: several platforms and versions, missing/duplicate draft selections, legacy/live state aliases, primary versus nonprimary locales, and exact nested-resource ownership.
- Pagination tests: multiple pages, repeated/foreign links, cross-collection links, invalid/tampered/cross-connection cursors, unsupported filters, and exhausted traversal budgets.
- Failure tests: facade versus Apple authentication, permission denial, malformed responses, multiple Apple errors, throttling/backoff, timeout before/after dispatch, and response-loss reconciliation.
- Workflow tests: dry preview makes zero Apple mutations; execution writes only destination IDs; stale plans fail before writes; concurrent same-key calls, worker crash, partial success, and cancellation preserve truthful state.
- Report tests: gzip/multiple members, BOM, changed column order, duplicate/missing headers, malformed rows, decompression limits, signed units, precision beyond cents, multiple currencies, date boundaries, unavailable reports, cache isolation, and paging across refreshes.
- Security/operational tests: permission checks before cache reads, redaction, key rotation/revocation, URL injection/redirect attempts, bounded fan-out, and durable state recovery.

For this repository, run `npm run check`: it validates generated contracts and documentation, exercises the Worker against mocked Apple responses, and runs the dry-run bundle in local workerd through Miniflare. Tests include ownership, grant isolation, JWT signatures/rotation, cursor binding/tampering, pagination failures, timeout/body budgets, error redaction, read concurrency, and generated setup files. Original Studio tests remain useful fixtures for later reports/copy/client migration work. Real-account verification must be explicitly configured and read-only by default; mutation tests need designated fixtures and a test app. Ordinary checks never contact Apple or deploy.

Apple publishes an [OpenAPI specification](https://developer.apple.com/sample-code/app-store-connect/app-store-connect-openapi-specification.zip), linked from its [API overview](https://developer.apple.com/app-store-connect/api/). The download inspected for this plan identifies itself as **4.5.1**. It confirms separate create/update schemas, relationship-based category updates, required name/locale on app-info localization creation, nullable listing fields, and evolving age-rating fields. Treat that as a versioned input, not a permanent guarantee of server behavior.

Pin the downloaded schema and checksum during implementation. Generate or validate internal Apple types from it where useful, but hand-design the facade's smaller OpenAPI contract. Review schema diffs for new required fields, enum values, deprecations, and changed paths. In particular, do not infer write permission from a GET schema or reject new upstream read enum values by crashing. Preserve unknown read values and restrict writes to reviewed supported values. Verify state-dependent editability and monthly report encoding with targeted integration fixtures before release.

The first usable slice is apps → explicit version/app-info selection → locale read/edit, backed by consistent auth, pagination, and errors. Reports and copy workflows then reuse that foundation without exposing Apple's graph and transport details to every client.
