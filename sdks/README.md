# Parsequerade SDKs

SDKs execute the Parsequerade facade inside the caller's program and send requests directly to App Store Connect. This removes the hosted proxy hop. Apple credentials and JWT signing stay in the caller's process.

- [Rust](rust/README.md): the first SDK, covering all 12 implemented reads and the scoped age-rating update.
- [TypeScript](typescript/README.md): a Node.js 22+ ESM package with typed methods for the same resources and no runtime dependencies. It bundles the Worker's existing facade implementation.

SDK support follows the implemented runtime contracts in `api/read-contracts.json` and `api/age-rating-write-contract.json`, rather than the broader proposed documentation. Hosted connection creation, revocation, and facade grants belong to the hosted service and are not needed by direct SDKs. Apple key permissions govern SDK access.

Run `npm run build:sdks` to regenerate both SDKs' artifacts and build the TypeScript package. `npm run generate:sdks` only regenerates the artifacts. Both generators share contract loading and model selection in `scripts/sdk_contracts.py`.

`npm run check:sdks` validates and tests both SDKs. `npm run test:sdk:typescript` and `npm run test:sdk:rust` run each SDK's tests separately. `npm run check` checks both SDKs for generated-file drift and tests TypeScript without requiring Rust.
