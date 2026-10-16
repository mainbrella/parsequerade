# Parsequerade SDKs

SDKs execute the Parsequerade facade inside the caller's program and send requests directly to App Store Connect. This removes the hosted proxy hop. Apple credentials and JWT signing stay in the caller's process.

| SDK | Requirements | Runtime dependencies |
| --- | --- | --- |
| [Rust](rust/README.md) | Async Rust with Tokio | See Cargo.toml |
| [TypeScript](typescript/README.md) | Node.js 22+, ESM | None; bundles the Worker's facade |
| [Python](python/README.md) | Python 3.11+, synchronous; Rust to build from source | None in Python; wheel bundles the Rust core |
| [Swift](swift/README.md) | Swift 5.9+, Apple platforms | None; Foundation and CryptoKit |
| [Go](go/README.md) | Go 1.22+ | None; Go standard library |

Rust and Python share one execution core and expose 22 reads, scoped age-rating and metadata updates, version creation, verified build attachment, and resumable listing-image uploads. Python is the first language binding of the Rust core: operation fixes and new behavior in Rust apply directly to Python. TypeScript, Swift, and Go retain their 12 core reads and scoped age-rating update. `api/sdk-catalog.json` declares contract groups and explicit runtime support; generating types alone does not enable an operation. Hosted connection creation, revocation, and facade grants belong to the hosted service and are not needed by direct SDKs. Apple key permissions govern SDK access. Packages are not yet published to their registries.

`npm run generate:sdks` regenerates models, methods, bundled contracts, licenses, and shared native test fixtures. The generators share the language-independent catalog, contract loading, and model selection in `scripts/sdk_contracts.py`. The native tests reuse the Worker's core read fixtures through `scripts/export_sdk_fixtures.mjs`. Rust and Python also execute shared request/response operation scenarios from `tests/sdk/scenarios.json`, covering media and build reads, metadata writes, version creation, ownership failures, and uncertain build confirmation.

`npm run build:sdks` regenerates artifacts, builds TypeScript, creates a Python wheel, compiles Swift, and builds Go. It requires Node.js, Python with pip, Rust, Swift, and Go. Python wheels and source distributions include the Rust core; a source build needs Cargo, while a compatible wheel needs no Rust installation. Individual builds are available as `build:sdk:typescript`, `build:sdk:python`, `build:sdk:swift`, and `build:sdk:go`; Rust builds through Cargo.

`npm run check:sdks` checks generated-file drift, Rust formatting and Clippy, Go vet, and tests all five SDKs. Go tests include race detection. Install Rust, Swift, and Go, then install the Python SDK in your active environment with `python3 -m pip install -e 'sdks/python[dev]'`. Each SDK also has a `test:sdk:typescript`, `test:sdk:rust`, `test:sdk:python`, `test:sdk:swift`, or `test:sdk:go` command. Swift requires an Apple host.

`npm run check` checks every SDK for generated-file drift and tests TypeScript alongside the Worker, without requiring Rust, Swift, Go, or Python development dependencies. SDK tests use mocked Apple responses and generated keys, never real Apple credentials.
