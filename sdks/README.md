# Parsequerade SDKs

SDKs execute the Parsequerade facade inside the caller's program and send requests directly to App Store Connect. This removes the hosted proxy hop. Apple credentials and JWT signing stay in the caller's process.

- [Rust](rust/README.md): the first SDK, covering all 12 implemented reads and the scoped age-rating update.

SDK support follows the implemented runtime contracts in `api/read-contracts.json` and `api/age-rating-write-contract.json`, rather than the broader proposed documentation. Hosted connection creation, revocation, and facade grants belong to the hosted service and are not needed by direct SDKs. Apple key permissions govern SDK access.

Run `npm run build:sdks` to regenerate bundled Rust contracts and response models, and `npm run check:sdks` to validate and test the Rust crate. `npm run check` checks generated SDK files for drift without requiring Rust.
