#!/usr/bin/env python3
"""Build the static GET reference from reviewed Apple facts and facade contracts."""

import argparse
import json
import re
from docs_render import ROOT, DIST, esc, code, link, page_url, write_page, header, section, endpoint_line, requests, code_block, table, resource_rows

SOURCE = json.loads((ROOT / "docs/apple-get-reference.json").read_text())
SCHEMAS = SOURCE["schemas"]
ENDPOINTS = {e["slug"]: e for e in SOURCE["endpoints"]}
OVERRIDES = json.loads((ROOT / "docs/facade-overrides.json").read_text())

FIELD_DESCRIPTIONS = {
    "id": "Apple’s resource ID. Keep this string unchanged when addressing a resource.",
    "appId": "The app that owns this resource.",
    "appInfoId": "The selected app-info resource.",
    "versionId": "The selected App Store version.",
    "name": "The resource’s name. An app’s localized name is read from its app-info localization.",
    "bundleId": "The app’s bundle identifier, such as com.example.notes.",
    "sku": "The identifier you assigned to the app in App Store Connect.",
    "primaryLocale": "The app’s primary locale. It does not cause automatic locale fallback.",
    "contentRightsDeclaration": "Whether the app uses third-party content.",
    "locale": "The exact Apple locale identifier, such as en-US.",
    "description": "The localized store description.",
    "keywords": "The localized keywords, returned as Apple’s comma-separated string.",
    "marketingUrl": "The localized marketing website. Null remains null.",
    "supportUrl": "The localized support website.",
    "promotionalText": "The localized promotional text shown on the product page.",
    "whatsNew": "The localized release notes for this version.",
    "version": "The uploaded build’s version string; this is separate from the store versionString.",
    "versionString": "The App Store version string selected for this resource.",
    "uploadedDate": "When Apple received the build, as an RFC 3339 timestamp.",
    "expirationDate": "When this build expires for testing, as an RFC 3339 timestamp.",
    "expired": "Whether the build has expired.",
    "minOsVersion": "The minimum supported OS version reported by the build.",
    "processingState": "Apple’s build-processing state. This does not indicate App Review approval.",
    "usesNonExemptEncryption": "Whether the build uses encryption that is not exempt from export compliance.",
    "buildAudienceType": "Whether a build is internal-only or eligible for App Store distribution.",
    "iconAssetToken": "Apple’s icon asset token, preserved as returned.",
    "nextCursor": "Opaque continuation token, or null when the collection is complete. Pass it back as cursor.",
    "items": "The current page of plain resource objects. An empty collection returns an empty array.",
    "copyright": "The copyright notice attached to the App Store version.",
    "platform": "Apple’s platform value, preserved without renaming.",
    "appVersionState": "The version’s current App Store distribution state.",
    "state": "The current state reported by Apple for this resource.",
    "contactFirstName": "The review contact’s first name.",
    "contactLastName": "The review contact’s last name.",
    "contactPhone": "The review contact’s telephone number.",
    "contactEmail": "The review contact’s email address.",
}


def resolve(schema):
    if "$ref" in schema:
        return {**SCHEMAS[schema["$ref"].rsplit("/", 1)[1]], **{k: v for k, v in schema.items() if k != "$ref"}}
    return schema


def type_name(schema):
    schema = resolve(schema)
    value = schema.get("type", "object")
    if value == "array":
        value = type_name(schema["items"]) + "[]"
    if schema.get("format"):
        value += " · " + schema["format"]
    if schema.get("nullable"):
        value += " | null"
    return value


def enum_note(schema):
    schema = resolve(schema)
    if schema.get("type") == "array":
        schema = resolve(schema.get("items", {}))
    values = schema.get("enum", [])
    return '<details class="enum-values"><summary>Values</summary>' + ", ".join(code(v) for v in values) + "</details>" if values else ""


def response_fields(endpoint, override):
    media = endpoint["response"].get("application/json", next(iter(endpoint["response"].values())))
    envelope = resolve(media["schema"])
    data = resolve(envelope.get("properties", {}).get("data", envelope))
    collection = data.get("type") == "array"
    model = resolve(data.get("items", data))
    attrs = resolve(model.get("properties", {}).get("attributes", {})).get("properties", {})
    fields = {"id": {"type": "string"}, **{k: {"type": "string"} for k in override.get("context", {})}, **attrs}
    return collection, fields


def render_endpoint(endpoint, contract, entries, topics=None, domains=None):
    title, route = contract["title"], contract["route"]
    group = next(t["title"] for t in SOURCE["topics"] if t["slug"] == endpoint["topic"])
    body = header(title, contract["summary"], group) + endpoint_line(route)
    body += '<p class="response-caption"><span class="status-label">Proposed</span> ' + esc(contract["stage"]) + ' · Service not implemented</p>'
    body += section("request", "Request", '<p>Routes are relative to your connection. Use a facade API key; the service manages Apple authentication.</p>' + requests(route, contract.get("queryExample")) + '<p class="response-caption">asc.example.com is a placeholder for your deployment. Examples use fictional IDs.</p>')
    path_rows = []
    path_descriptions = {"appId": "Apple’s app ID. Returned by List apps.", "versionId": "An explicit version ID belonging to this app. Select it from the version list.", "appInfoId": "An explicit app-info ID belonging to this app.", "locale": "The exact locale to read. No fallback to a different locale."}
    for name in re.findall(r"\{(\w+)\}", route):
        path_rows.append((name, "string", esc(path_descriptions.get(name, "The selected resource’s ID.")), "Required"))
    if path_rows:
        body += section("path-parameters", "Path parameters", table(path_rows))
    query_rows = contract.get("queryRows", [])
    if query_rows:
        body += section("query-parameters", "Query parameters", table(query_rows))
    collection, fields = response_fields(endpoint, contract)
    response = contract["example"]
    description = "A page of flat objects, with an opaque continuation cursor." if collection else "One flat object for the selected resource."
    body += section("response", "Response", "<p>" + description + "</p>" + code_block(response, "response-json", "200 · application/json", True) + '<p class="response-caption">Illustrative response. Optional read fields may be absent or null; unknown enum values are preserved.</p>')
    field_rows = []
    for name, schema in fields.items():
        if schema.get("deprecated"):
            continue
        desc = FIELD_DESCRIPTIONS.get(name, "Apple’s " + re.sub(r"(?<!^)(?=[A-Z])", " ", name).lower() + " value, preserved in the read response.")
        field_rows.append((name, type_name(schema), esc(desc) + enum_note(schema)))
    envelope_rows = [("items", "object[]", FIELD_DESCRIPTIONS["items"]), ("nextCursor", "string | null", FIELD_DESCRIPTIONS["nextCursor"])] if collection else []
    body += section("response-fields", "Response fields", (table(envelope_rows) + "<h3>Each item</h3>" if collection else "") + table(field_rows))
    if contract.get("notes"):
        body += section("behavior", "Behavior", '<ul class="prose-list">' + "".join("<li>" + esc(note) + "</li>" for note in contract["notes"]) + "</ul>")
    if collection:
        body += section("pagination", "Pagination", '<p>Pass ' + code("nextCursor") + ' back as ' + code("cursor") + ' with the same filters and ordering. Stop when it is ' + code("null") + '. Each request returns one bounded page; the service never reports a capped or incomplete traversal as a complete collection.</p>')
    error_rows = [("400", "invalid_query", "Unsupported filter, sort, limit, or cursor."), ("401", "unauthorized", "Missing or invalid facade API key."), ("403", "forbidden / upstream_forbidden", "The caller or configured Apple key cannot read this resource."), ("404", "localization_not_found" if "{locale}" in route else "resource_not_found", "The requested resource or exact locale is absent or outside the caller’s scope."), ("429", "rate_limited", "A facade or Apple rate limit was reached. Respect Retry-After when returned."), ("502–504", "upstream error", "Apple authentication, response parsing, availability, or timeout failure; this is not an empty result.")]
    body += section("errors", "Errors", table(error_rows, "HTTP"))
    source_body = '<p>The adapter calls ' + code("GET " + endpoint["applePath"]) + '. ' + link(endpoint["source"], "Read Apple’s original endpoint ↗") + "</p>"
    if contract.get("queryFrom"):
        other = ENDPOINTS[contract["queryFrom"]]
        source_body += '<p>Filtered requests use ' + code("GET " + other["applePath"]) + ' with ' + code("filter[app]") + ' supplied from the path. ' + link(other["source"], "Apple’s filter reference ↗") + "</p>"
    if "{locale}" in route:
        source_body += '<p>Resolve the parent’s localization collection using the exact locale, validate ownership, and then supply the resolved ID to this Apple endpoint.</p>'
    source_body += '<p>Apple’s ' + code("data") + ', ' + code("attributes") + ', and ' + code("relationships") + ' envelopes are translated inside the service. Read access does not make these fields writable.</p>'
    body += section("apple-mapping", "Apple mapping", '<details class="source-details"><summary>Upstream route and translation</summary>' + source_body + '<p class="response-caption">Reviewed ' + esc(SOURCE["reviewedAt"]) + ' · OpenAPI ' + esc(SOURCE["specification"]["version"]) + "</p></details>")
    related = [e for e in entries if e["url"] != page_url(endpoint["slug"])][:4]
    if related:
        body += section("related", "Related reads", resource_rows(related))
    toc = [("overview", title), ("request", "Request")]
    if path_rows: toc.append(("path-parameters", "Path parameters"))
    if query_rows: toc.append(("query-parameters", "Query parameters"))
    toc.extend([("response", "Response"), ("response-fields", "Response fields"), ("behavior", "Behavior")])
    if collection: toc.append(("pagination", "Pagination"))
    toc.extend([("errors", "Errors"), ("apple-mapping", "Apple mapping")])
    write_page(page_url(endpoint["slug"]), title, contract["summary"], body, entries, toc, group, topics, domains)


def build_examples():
    contracts = {slug: dict(value) for slug, value in OVERRIDES.items()}
    app = contracts["get-v1-apps"]
    app["queryRows"] = [("bundleId", "string", "Match an exact bundle identifier, such as " + code("com.example.notes") + "."), ("name", "string", "Filter by the app’s name."), ("sku", "string", "Filter by the app’s SKU."), ("limit", "integer", "Number of apps per page. Maximum 200.", "Default 50"), ("cursor", "string", "Opaque cursor from the preceding response."), ("sort", "string", "Comma-separated sort keys: " + ", ".join(code(s) for s in ["name", "-name", "bundleId", "-bundleId", "sku", "-sku"]) + ".")]
    builds = contracts["get-v1-apps-_id_-builds"]
    builds["queryRows"] = [("processingState", "string", "One or more comma-separated states: " + ", ".join(code(s) for s in ["PROCESSING", "FAILED", "INVALID", "VALID"]) + "."), ("version", "string", "Filter by the uploaded build’s version string."), ("expired", "string", "Filter by " + code("true") + " or " + code("false") + "."), ("limit", "integer", "Number of builds per page. Maximum 200.", "Default 50"), ("cursor", "string", "Opaque cursor from the preceding response."), ("sort", "string", "Use " + code("-uploadedDate") + " for newest uploads first. Also supports version and preReleaseVersion, with a leading minus for descending order.")]
    entries = [{"title": c["title"], "route": c["route"], "url": page_url(slug), "applePath": ENDPOINTS[slug]["applePath"], "group": "Example pages", "keywords": slug} for slug, c in contracts.items()]
    for slug, contract in contracts.items():
        render_endpoint(ENDPOINTS[slug], contract, entries)
    (DIST / "docs/reference-index.json").write_text(json.dumps(entries, indent=2) + "\n")
    body = header("API reference", "Plain JSON, readable routes, and explicit resource targets. Start with these three example GET contracts.")
    body += '<p class="contract-note">Proposed API. Parsequerade’s service is not implemented yet.</p>'
    body += section("endpoints", "GET endpoints", resource_rows(entries))
    body += section("coverage", "Coverage", '<p>The source review covers 457 GET pages in 84 topics from Apple’s App Store documentation. The remaining pages are being generated from the same reviewed snapshot.</p>')
    write_page("/docs/reference/", "API reference", "Parsequerade proposed GET API reference.", body, entries, [("overview", "API reference"), ("endpoints", "GET endpoints"), ("coverage", "Coverage")])
    print("Generated three example endpoint pages and the reference index.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--examples", action="store_true", help="Build only the three reviewed example pages.")
    args = parser.parse_args()
    build_examples()
