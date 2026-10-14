# App Store GET reference progress

The reference is being expanded to cover every GET endpoint reached through the topic tree of [Apple’s App Store API documentation](https://developer.apple.com/documentation/appstoreconnectapi/app-store). The site describes Parsequerade’s proposed JSON REST facade; the service itself remains unimplemented.

## Saved work

- Read `README.md`, `rest.md`, the homepage, and the three existing documentation pages.
- Read 134 Apple topic pages and 457 distinct GET endpoint pages. The endpoints are organized into 84 resource topics.
- Saved endpoint URLs, source checksums, parameters, response schemas, topic memberships, and deprecations in `docs/apple-get-reference.json`.
- Added `scripts/sync_apple_docs.py` to repeat the topic-tree traversal. It follows topic links rather than unrelated see-also links and verifies every GET path against Apple’s OpenAPI specification.
- Pinned Apple’s specification version 4.5.1 and ZIP checksum `7b826ae9ca3e45b0e7df2643cbb811c9930632640944db11a8af4586fab9db46` in the snapshot.
- Updated `dist/app.js` and `dist/styles.css` with endpoint search, request-language tabs, copy controls, a keyboard-accessible mobile drawer, and compact purple-accent reference styles. JavaScript syntax validation passes.

## Reference design

Every Apple GET page will have a corresponding static page under `/docs/reference/{apple-documentation-slug}/`. Topic pages and an index will make the full collection browsable, with search by title, facade route, Apple route, and resource group.

Pages will show a plain-language purpose, proposed facade route, cURL/JavaScript/Python requests, path and query parameters, a flat JSON response, complete read-field definitions, relevant selection and pagination behavior, errors, and the original Apple mapping. Apple’s resource-envelope and relationship-query details belong in a disclosure rather than the primary request flow.

Existing contracts in `rest.md` take precedence: connection-scoped authentication, plain single objects, `items` and `nextCursor` for collections, exact locales, explicit app-info and version targets, ownership checks, the four-field review-contact view, and no implied writes. First-release reads and read-parity routes must remain distinguishable from broader proposed read designs. Deprecated Apple variants must remain visible and must not silently collide with modern resource contracts.

The homepage’s existing layout and Wrangler hosting remain in place. Documentation generation will be offline and reproducible from the saved source snapshot, without new application dependencies.

## Remaining work

1. Implement the facade route mappings, response normalization, shared page layout, and offline page generator.
2. Generate all 457 endpoint pages, topic indexes, search data, and useful introduction/quickstart/conventions pages.
3. Account for conditional query requirements, CSV-only offer-code downloads, structured metrics, deprecated fields, and legacy endpoint versions.
4. Check endpoint coverage, route collisions, sample response types, internal links, and anchors.
5. Use the installed Playwright package at `/Users/aa/mainbrella/repo-rodeo/node_modules/playwright` for browser verification at 390×844, 768×1024, 1280×800, and 1440×900; check navigation, search, tabs, copy behavior, and overflow.
6. Update the README with the generation/source-review commands and report the final verification results.

No authenticated Apple calls, writes to App Store Connect, or deployment have been made.
