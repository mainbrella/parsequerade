# Shared native host bridge

This crate exposes the Rust execution core to Node, Go, and Swift. It builds as a
Rust library for the Node addon and a static library with a C interface for Go and
Swift. Python calls the same core directly through its existing extension.

The host owns opaque integer handles and UTF-8 JSON messages. No callbacks or
pointers into a host runtime cross the interface. Every C string returned by the
bridge must be released using `pq_string_free`. The C API is declared in
`include/parsequerade.h`; bindings check `pq_abi_version()` before use.

Create a client with `client_new_json` / `pq_client_new`, supplying `issuerId`,
`keyId`, `privateKey`, and `customTransport`. The result contains a client handle
or a sanitized core error. Construction validates credentials without networking.

Start an operation with `start_json` / `pq_start` and a request containing
`operation`, `args`, and, for uploads, base64 `png`. Supported operation names are
`get`, `update_age_rating`, `update_metadata`, `create_version`, `attach_build`,
`listing_image_reference_data`, and `upload_listing_image`. Reads accept a facade
path and query pairs. Upload arguments contain the request and current checkpoint.

Poll the operation handle for these events:

| Event | Host action |
| --- | --- |
| `http` | Send one bounded HTTP request and acknowledge the status, headers, and base64 body, or a sanitized transport failure. Only emitted with `customTransport: true`. |
| `checkpoint` | Copy the checkpoint to the caller and persist an independent snapshot durably before acknowledging success. |
| `result` | Copy any final checkpoint, then return the value or sanitized core error. |
| `null` | No event arrived before the polling timeout. |

Responses are correlated using the event ID. HTTP events include a remaining timeout and absolute `deadlineMs`; hosts recheck
expiry immediately before dispatch. The bridge suppresses expired or abandoned
events and rejects late acknowledgments. The host must never retry requests or
follow redirects. Rust owns authentication, URL and response validation, time and
size bounds, retries, ownership and locale resolution, and all write/upload logic.
Blob transfers contain Apple's validated storage headers without the API JWT.
Persistence errors stop execution before the next write; adapters preserve the
original host exception or error after receiving the final checkpoint.

Cancellation releases pending acknowledgments and stops the operation. A cancelled
read returns `operation_cancelled`; a cancelled mutation conservatively returns
`write_outcome_unknown`. Adapters must not dispatch queued requests after cancellation.
Release every operation handle in all paths using `operation_free` /
`pq_operation_free`. Client release prevents new operations; already-started
operations own their client state. Language adapters implement their public `close`
behavior by cancelling active operations before releasing the client.

Node polls on a native blocking worker and services events on its event loop. Swift
polls on Dispatch workers and awaits its async transport/persistence closures. Go
polls through cgo and services events with request contexts. These adapters do not
need foreign function pointers or callback registrations.

From the repository root:

```sh
npm run test:sdk:native
npm run check:sdk:native
```

Tests execute the shared operation and upload scenarios and cover cancellation,
released clients, late responses, and invalid requests. Build helpers package the
static archive or Node addon for each target; no hosted service is involved.
