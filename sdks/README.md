# Parsequerade SDKs

All five SDKs execute the same Rust core in the caller's process and send requests directly to App Store Connect. Apple credentials and JWT signing stay local. They support 22 reads, age-rating and metadata updates, version creation, verified build attachment, and resumable listing-image uploads.

| SDK | Public API | Native integration |
| --- | --- | --- |
| [Rust](rust/README.md) | Async Rust with Tokio | Canonical execution core |
| [TypeScript](typescript/README.md) | Node.js 22+, promises and typed objects | Node native addon |
| [Python](python/README.md) | Python 3.11+, synchronous typed dictionaries | Bundled Python extension |
| [Go](go/README.md) | Go 1.22+, contexts and typed structs | cgo and a Rust static library |
| [Swift](swift/README.md) | Swift 5.9+, async actor and Codable models | C interface in an Apple XCFramework |

`api/sdk-catalog.json` declares contracts, models, and explicit implementation support. The hosted Worker retains its 12 baseline reads and scoped age-rating update. Hosted connections and facade grants are unnecessary for SDKs; Apple's key permissions govern access. Packages are not yet published to their registries.

The Python extension calls the Rust crate directly. TypeScript, Go, and Swift share the host bridge in `sdks/native`: operation dispatch and lifecycle, cancellation, transport events, and checkpoint acknowledgments. Host adapters supply their native transport hooks and durable persistence callbacks. They do not implement the facade's resource resolution, validation, retries, pagination, or upload workflow separately.

`npm run generate:sdks` regenerates public models, thin resource methods, contracts, licenses, and test fixtures. Operation scenarios in `tests/sdk/scenarios.json` and upload/recovery cases in `tests/sdk/uploads.json` run through the shared bridge and language adapters. The original Rust and Python suites retain additional coverage of the underlying workflows and binding boundaries.

`npm run build:sdks` generates artifacts and builds each SDK. Source builds require Node.js, Python with pip, Rust, Swift/Xcode on macOS, Go, and a C compiler. TypeScript's npm package bundles its host-specific `.node` addon. Python wheels bundle their extension and need no Python runtime dependencies. Go distributions must include the matching static archive and require cgo. Swift distributions must include an XCFramework with the appropriate architecture and platform slices; the default build produces a host macOS artifact for local testing. See each SDK README for cross-target and distribution instructions.

`npm run check:sdks` checks generated-file drift and formatting, runs Rust Clippy and Go vet, builds native bindings, and tests all SDKs. Go tests include race detection. Activate a Python environment and install development dependencies with `python3 -m pip install -e 'sdks/python[dev]'` first. Individual `build:sdk:*`, `test:sdk:*`, and `check:sdk:*` commands are available, including `native` for the common bridge. Swift builds require an Apple host. `npm run check` checks the hosted service and TypeScript SDK and now requires Rust for the Node addon.

Tests use generated keys and mocked Apple responses, never real Apple credentials. Packaging checks run consumers outside the checkout.
