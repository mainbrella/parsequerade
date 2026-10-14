I’d build the API Worker inside this repository, alongside the existing static site.

wrangler.jsonc (wrangler.jsonc:1) already uses Workers Static Assets, rather than Pages-specific configuration. It simply has no Worker entry point yet. Also, rest.md’s implementation section (rest.md:285) proposes a separate Rust server; I’d replace that section with the Cloudflare architecture below while retaining its API contracts.

1. Add API handling to the existing deployment.

   Keep the current domain and use:

   /                              Static homepage
/docs/*                        Static documentation
/v1/connections/{connectionId}/* API Worker

   The initial Wrangler changes would be:

   "main": "./worker/index.js",
"assets": {
  "directory": "./dist",
  "binding": "ASSETS",
  "run_worker_first": ["/v1", "/v1/*"],
  "html_handling": "auto-trailing-slash",
  "not_found_handling": "404-page"
}

   API failures and unknown API routes must return JSON. Cloudflare supports selective Worker-first routing through this configuration. Cloudflare documentation (https://developers.cloudflare.com/workers/static-assets/binding/)
2. Build one shared Apple transport and explicit resource adapters.

   The request flow should be:

   Facade authentication and validation
    → resource adapter and ownership checks
    → Apple authentication and HTTP transport
    → flat response or normalized error

   Start with JavaScript modules, matching the repository’s existing code. Separate authentication, transport, pagination, resource mapping, and write tracking so they can be tested independently.

   The transport owns timeouts, redirect rejection, Apple-origin validation, throttling, and bounded read retries. Resource adapters own query translation, localization lookup, relationship payloads, and response field allowlists.

   For example, a version-localization PATCH resolves the exact locale under the selected version, verifies that version belongs to the app, constructs Apple’s typed update envelope, and returns the flat localization object.
3. Use operator-configured credentials for the first release.

   Support configured Apple team keys and facade API keys scoped to connections, app access, and read/write/report capabilities.

   Store Apple private keys as Worker secrets. Store only hashes of high-entropy facade keys; document issuance, rotation, and revocation. Authenticate before accessing cached data or operation records. Worker secrets (https://developers.cloudflare.com/workers/configuration/secrets/)

   Generate and cache short-lived ES256 Apple JWTs per credential revision. Clients continue using only their facade key. Apple documents the required claims and token lifetime rules. Apple authentication (https://developer.apple.com/documentation/appstoreconnectapi/generating-tokens-for-api-requests)

   Begin with deployment configuration as the administration mechanism. A connection-management UI can follow when there is a concrete need.
4. Add durable coordination before enabling writes.

   I recommend one SQLite-backed Durable Object per connection, holding idempotency records, write attempts, operation results, and later copy plans. It can also coordinate connection-wide Apple request limits. Durable Object storage (https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/)

   Every mutation follows this sequence:

   Validate and resolve target
    → persist intent
    → persist dispatch marker
    → call Apple
    → persist confirmed result or unknown outcome

   Reusing an idempotency key with the same request returns the existing attempt; different content returns a conflict. A crash or timeout after dispatch requires reconciliation before another write.

   Implement explicit destination-app locking shared by ordinary mutations and copy execution. Async network calls must not permit conflicting writes to overlap.

   Use Durable Object alarms for recovery and later background execution. Their delivery is at least once, so recovery must inspect persisted state before dispatching any mutation. Alarm semantics (https://developers.cloudflare.com/durable-objects/api/alarms/)
5. Deliver in useful, testable increments.

   | Phase | Deliverable | Acceptance gate |
   |---|---|---|
   | Foundation | Worker routing, connection auth, Apple JWTs, transport, errors, cursors | Static site works; API errors are JSON; credentials stay private |
   | First usable reads | Apps, app infos, versions, localization lists and exact-locale reads | A client can find an app and read any selected locale using facade JSON |
   | First usable write | Version-localization PATCH, durable attempts, operation lookup | A description edit works; duplicate requests and lost responses do not blindly repeat writes |
   | Remaining first-release routes | Locale creation, privacy URL, category, version, review contact, content rights, age rating | Separate create/update allowlists and parent ownership checks pass |
   | Read parity and reports | Builds/icons, users, bundle IDs, metadata composition, daily/monthly sales | Pagination is complete and truthful; reports preserve precision |
   | Copy workflows | Stored preview, execution, progress, stale-plan checks, cancellation | Preview performs zero writes; partial success and unknown outcomes survive restarts |
   | Client migration | Studio and Disco adopt the facade behind explicit configuration | Representative reads and writes match existing behavior |

   The first implementation PR should deliver foundation plus the read flow, establishing the runtime before adding mutations.
6. Make contracts and documentation track implementation.

   The existing 457 GET designs are valuable specifications, but they are not executable adapters. Start with the routes above.

   Export reviewed contracts from the Python documentation tooling into a language-neutral manifest for runtime validation and documentation checks. Keep Apple-specific adapter behavior explicit, especially for ownership checks, locale resolution, reports, and legacy resources.

   Add implementation status separately from delivery stage, so documentation distinguishes “planned for first release” from “available.” Publish a facade OpenAPI document for implemented routes and validate examples against it.

   Reports need one Cloudflare-specific adjustment: rest.md allows 128 MiB expanded reports, while a Worker isolate has a 128 MB total memory limit. Stream decompression and parsing into bounded, immutable report snapshots, using R2 for larger artifacts rather than buffering the entire report and JSON rows. Verify monthly date encoding and concatenated gzip behavior with fixtures. Worker limits (https://developers.cloudflare.com/workers/platform/limits/)

Verification should combine mocked Apple contract tests with Workers-runtime tests for concurrent idempotency, restart recovery, ownership, cursor tampering, pagination, and ambiguous write outcomes. Authenticated checks should be opt-in and read-only by default; mutation checks need a designated test app.
