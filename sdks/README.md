# Parsequerade SDKs

SDKs execute the Parsequerade facade inside the caller's program and send requests directly to App Store Connect. This removes the hosted proxy hop. Apple credentials and JWT signing stay in the caller's process.

| SDK | Requirements | Runtime dependencies |
| --- | --- | --- |
| [Rust](rust/README.md) | Async Rust with Tokio | See Cargo.toml |
| [TypeScript](typescript/README.md) | Node.js 22+, ESM | None; bundles the Worker's facade |
| [Python](python/README.md) | Python 3.11+, synchronous | `cryptography` for ES256 signing |
| [Swift](swift/README.md) | Swift 5.9+, Apple platforms | None; Foundation and CryptoKit |

All four SDKs cover the 12 implemented reads and the scoped age-rating update. SDK support follows the implemented runtime contracts in `api/read-contracts.json` and `api/age-rating-write-contract.json`, rather than the broader proposed documentation. Hosted connection creation, revocation, and facade grants belong to the hosted service and are not needed by direct SDKs. Apple key permissions govern SDK access. Packages are not yet published to their registries.

`npm run generate:sdks` regenerates models, methods, bundled contracts, licenses, and shared native test fixtures. The generators share contract loading and model selection in `scripts/sdk_contracts.py`. The native tests reuse the Worker's fixtures through `scripts/export_sdk_fixtures.mjs`.

`npm run build:sdks` regenerates artifacts, builds TypeScript, creates a Python wheel, and compiles Swift. It requires Node.js, Python with pip, and Swift. Individual builds are available as `build:sdk:typescript`, `build:sdk:python`, and `build:sdk:swift`; Rust builds through Cargo.

`npm run check:sdks` checks generated-file drift, Rust formatting and Clippy, and tests all four SDKs. Install Rust and Swift, then install the Python SDK in your active environment with `python3 -m pip install -e sdks/python`. Each SDK also has a `test:sdk:typescript`, `test:sdk:rust`, `test:sdk:python`, or `test:sdk:swift` command. Swift requires an Apple host.

`npm run check` checks every SDK for generated-file drift and tests TypeScript alongside the Worker, without requiring Rust, Swift, or Python signing dependencies. SDK tests use mocked Apple responses and generated keys, never real Apple credentials.
