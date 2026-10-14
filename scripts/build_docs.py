#!/usr/bin/env python3
"""Build the static GET reference from reviewed Apple facts and facade contracts."""

import argparse
import json
import re
from html.parser import HTMLParser
from docs_render import ROOT, DIST, esc, code, link, page_url, write_page, header, section, endpoint_line, requests, code_block, table, resource_rows
from docs_contracts import make_contract

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
    "dataPoints": "Measurements for this dimension, including the start and end of each reporting interval.",
    "dimensions": "Plain identifiers for the entities represented by these measurements.",
    "productData": "Performance series grouped by product, version, and platform.",
    "insights": "Apple’s performance findings for this resource.",
    "appMetadata": "App and platform metadata associated with the performance overview.",
    "categories": "The performance categories included in this overview.",
    "signatures": "Diagnostic signatures used by the reported performance findings.",
    "telemetryIdentifier": "The identifier associated with this telemetry response.",
    "customerPrice": "The customer-facing price, preserved as a decimal string.",
    "proceeds": "Developer proceeds for this price point, preserved as a decimal string.",
    "proceedsYear2": "Developer proceeds after the first subscription year, preserved as a decimal string.",
    "productId": "The product identifier configured in App Store Connect.",
    "referenceName": "The internal reference name configured for this resource.",
    "familySharable": "Whether the purchase is available through Family Sharing.",
    "subscriptionPeriod": "The subscription’s renewal period, using Apple’s enum values.",
    "groupLevel": "The subscription’s position within its subscription group.",
    "reviewNote": "The review note attached to this purchase or subscription.",
    "rating": "The customer’s star rating.",
    "title": "The customer review’s title.",
    "body": "The customer review’s text.",
    "reviewerNickname": "The customer’s public review nickname.",
    "responseBody": "The developer’s published or pending response text.",
    "createdDate": "When Apple created this resource, as an RFC 3339 timestamp.",
    "lastModifiedDate": "When this resource was last changed, as an RFC 3339 timestamp.",
    "startDate": "The beginning of this resource’s scheduled period.",
    "endDate": "The end of this resource’s scheduled period.",
    "fileName": "The uploaded file’s name.",
    "fileSize": "The file size in bytes.",
    "imageAsset": "Apple’s image dimensions and URL template. The service does not download the image.",
    "iconAsset": "Apple’s icon dimensions and URL template. The service does not download the icon.",
    "assetDeliveryState": "The delivery state and any processing issues Apple reports for this asset.",
    "uploadOperations": "Apple’s recorded upload-operation metadata. A GET does not perform these operations.",
    "sourceFileChecksum": "The checksum Apple records for the uploaded source file.",
    "screenshotDisplayType": "The device display format used by this screenshot set.",
    "previewType": "The device format used by this preview set.",
    "agreementText": "The text of the configured license agreement.",
    "currency": "The currency associated with this territory or price.",
    "currencyCode": "The currency code associated with this amount.",
    "availableInNewTerritories": "Whether availability extends automatically to new territories.",
    "contentRightsDeclaration": "Whether the app uses third-party content.",
    "isOrEverWasMadeForKids": "Whether Apple records this app as having been made for kids.",
    "streamlinedPurchasingEnabled": "Whether the app has streamlined purchasing enabled.",
    "accessibilityUrl": "The app’s accessibility-information website.",
    "subscriptionStatusUrl": "The production server-notification URL configured for subscriptions.",
    "subscriptionStatusUrlForSandbox": "The sandbox server-notification URL configured for subscriptions.",
    "subscriptionStatusUrlVersion": "The notification payload version configured for production.",
    "subscriptionStatusUrlVersionForSandbox": "The notification payload version configured for the sandbox.",
    "computedMinMacOsVersion": "The macOS minimum version calculated by Apple for this build.",
    "computedMinVisionOsVersion": "The visionOS minimum version calculated by Apple for this build.",
    "lsMinimumSystemVersion": "The minimum macOS system version recorded in the build.",
    "iconType": "Apple’s icon format or usage classification.",
    "masked": "Whether the returned icon has a platform mask applied.",
    "downloadable": "Whether Apple marks this version as downloadable.",
    "releaseType": "The selected release policy: manual, after approval, or scheduled.",
    "earliestReleaseDate": "The earliest scheduled release time, when configured.",
    "reviewType": "Whether this version is reviewed for the App Store or notarization.",
}


def resolve(schema):
    if "$ref" in schema:
        return {**SCHEMAS[schema["$ref"].rsplit("/", 1)[1]], **{k: v for k, v in schema.items() if k != "$ref"}}
    return schema


def type_name(schema):
    schema = resolve(schema)
    for keyword in ("oneOf", "anyOf", "allOf"):
        if schema.get(keyword):
            return " | ".join(type_name(s) for s in schema[keyword])
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


def humanize(name):
    return re.sub(r"(?<!^)(?=[A-Z])", " ", name).replace("_", " ").lower()


def field_description(name, schema, optional_relation=False):
    if optional_relation:
        return "ID of the related " + humanize(name.removesuffix("Id")) + ". Present only when Apple supplies linkage; related objects are not loaded automatically."
    if name in FIELD_DESCRIPTIONS:
        return FIELD_DESCRIPTIONS[name]
    schema = resolve(schema)
    if name.endswith("Id"):
        return "The ID of the selected " + humanize(name.removesuffix("Id")) + "."
    if schema.get("format") == "date-time":
        return humanize(name).capitalize() + ", as an RFC 3339 timestamp."
    if schema.get("format") == "uri":
        return "The " + humanize(name) + " URL reported by Apple."
    if schema.get("type") == "boolean":
        return "Whether Apple marks this resource as " + humanize(name) + "."
    return "The " + humanize(name) + " value reported by Apple for this resource."


def nested_fields(schema, name, depth=0):
    if depth >= 4:
        return ""
    schema = resolve(schema)
    if schema.get("type") == "array":
        schema = resolve(schema.get("items", {}))
    properties = schema.get("properties", {})
    if not properties:
        return ""
    rows = [(n, type_name(s), esc(field_description(n, s)) + enum_note(s), "Required" if n in schema.get("required", []) else "Optional") for n, s in properties.items()]
    body = table(rows)
    for child, value in properties.items():
        body += nested_fields(value, name + "." + child, depth + 1)
    return '<details class="schema-details"><summary>' + esc(name) + " fields</summary>" + body + "</details>"


def query_rows(parameters):
    rows = []
    descriptions = {
        "bundleId": "Match a bundle identifier, such as com.example.notes.",
        "name": "Filter by the resource’s name using Apple’s matching behavior.",
        "sku": "Filter by the app’s SKU.", "id": "Restrict results to the specified resource IDs.",
        "locale": "Restrict results to these exact locale identifiers.",
        "processingState": "Filter by Apple’s build-processing state; this is separate from App Review approval.",
        "state": "Filter by the current resource state. Version requests map this to Apple’s appVersionState.",
        "versionState": "Require related App Store versions to match these current distribution states.",
        "versionPlatform": "Require related App Store versions to match these platforms.",
        "platform": "Restrict results to the specified Apple platforms.",
        "period": "The reporting interval used for these metrics.",
        "groupBy": "Group measurements by the supported dimension.",
        "betaTesters": "Select the beta testers whose metrics should be returned.",
        "territory": "Restrict results to the specified App Store territory IDs, such as USA.",
        "sort": "Comma-separated sort keys. Prefix a key with - for descending order. Only the values below are supported.",
        "fields": "Comma-separated attribute names to return. This selects read fields and does not expand relationships.",
        "cursor": "Opaque nextCursor from a previous response. Keep the route, connection, filters, and ordering unchanged.",
    }
    for parameter in parameters:
        name = parameter["name"]
        schema = resolve(parameter["schema"])
        desc = descriptions.get(name, "Filter by " + humanize(name) + ".")
        meta = "Required" if parameter["required"] else "Optional"
        if name == "limit":
            maximum = schema.get("maximum", 200)
            desc = f"Number of items per page, from 1 to {maximum}."
        if "default" in parameter:
            meta += " · Default " + str(parameter["default"])
        value_type = type_name(schema)
        if schema.get("type") == "array":
            value_type = "string"
            desc += " Multiple values are comma-separated."
        rows.append((name, value_type, esc(desc) + enum_note(schema), meta))
    return rows


def render_endpoint(endpoint, contract, entries, topics=None, domains=None):
    title, route = contract["title"], contract["route"]
    group = next(t["title"] for t in SOURCE["topics"] if t["slug"] == endpoint["topic"])
    body = header(title, contract["summary"], group) + endpoint_line(route)
    implemented = contract["implementationStatus"] == "implemented"
    body += '<p class="response-caption"><span class="status-label">' + ("Implemented" if implemented else "Proposed") + '</span> · ' + esc(contract["stage"]) + (' · Operator configuration and deployment required' if implemented else ' · Adapter not implemented') + '</p>'
    body += section("request", "Request", '<p>Routes are relative to your connection. Use a facade API key; the service manages Apple authentication.</p>' + requests(route, contract.get("queryExample")) + '<p class="response-caption">asc.example.com is a placeholder for your deployment. Examples use fictional IDs.</p>')
    path_rows = []
    path_descriptions = {"appId": "Apple’s app ID. Returned by List apps.", "versionId": "An explicit version ID belonging to this app. Select it from the version list.", "appInfoId": "An explicit app-info ID belonging to this app.", "locale": "The exact locale to read. No fallback to a different locale."}
    for name in re.findall(r"\{(\w+)\}", route):
        path_rows.append((name, "string", esc(path_descriptions.get(name, "The selected resource’s ID.")), "Required"))
    if path_rows:
        body += section("path-parameters", "Path parameters", table(path_rows))
    parameter_rows = query_rows(contract.get("parameters", []))
    if parameter_rows:
        body += section("query-parameters", "Query parameters", table(parameter_rows))
    shape = contract["shape"]
    collection, fields = shape["collection"], shape["fields"]
    response = contract["example"]
    if shape["kind"] in ("ids", "codes"):
        description = "A page of plain " + ("resource IDs" if shape["kind"] == "ids" else "redemption codes") + ", with an opaque continuation cursor."
    elif shape["kind"] == "id":
        description = "A plain ID object. An empty optional relationship returns id: null; an upstream error remains an error."
    elif shape["kind"] == "metrics":
        description = "Structured metric data, with series and dimension values preserved." + (" Pages use items and nextCursor." if collection else "")
    else:
        description = "A page of flat objects, with an opaque continuation cursor." if collection else "One flat object for the selected resource."
    body += section("response", "Response", "<p>" + description + "</p>" + code_block(response, "response-json", "200 · application/json", True) + '<p class="response-caption">Illustrative response. Optional read fields may be absent or null; unknown enum values are preserved.</p>')
    field_rows = []
    for name, schema in fields.items():
        desc = "The format version of this metrics response." if shape["kind"] == "metrics" and name == "version" else field_description(name, schema, name in shape["optionalRelations"])
        meta = "Legacy field" if schema.get("deprecated") else "Optional linkage" if name in shape["optionalRelations"] else ""
        field_rows.append((name, type_name(schema), esc(desc) + enum_note(schema), meta))
    envelope_rows = [("items", "string[]" if shape["kind"] in ("ids", "codes") else "object[]", "The current page of " + ("IDs." if shape["kind"] == "ids" else "redemption codes." if shape["kind"] == "codes" else "plain resource objects. An empty collection returns [].")), ("nextCursor", "string | null", FIELD_DESCRIPTIONS["nextCursor"])] if collection else []
    fields_body = table(envelope_rows) if collection else ""
    if field_rows:
        fields_body += ("<h3>Each item</h3>" if collection else "") + table(field_rows)
    for name, schema in fields.items():
        fields_body += nested_fields(schema, name)
    body += section("response-fields", "Response fields", fields_body)
    if contract.get("notes"):
        body += section("behavior", "Behavior", '<ul class="prose-list">' + "".join("<li>" + esc(note) + "</li>" for note in contract["notes"]) + "</ul>")
    if collection:
        body += section("pagination", "Pagination", '<p>Pass ' + code("nextCursor") + ' back as ' + code("cursor") + ' with the same filters and ordering. Stop when it is ' + code("null") + '. Each request returns one bounded page; the service never reports a capped or incomplete traversal as a complete collection.</p>')
    error_rows = [("400", "invalid_query", "Unsupported filter, sort, limit, or cursor."), ("401", "unauthorized", "Missing or invalid facade API key."), ("403", "forbidden / upstream_forbidden", "The caller or configured Apple key cannot read this resource."), ("404", "localization_not_found" if "{locale}" in route else "resource_not_found", "The requested resource, parent, or exact locale is absent or outside the caller’s scope."), ("429", "rate_limited", "A facade or Apple rate limit was reached. Respect Retry-After when returned."), ("502–504", "upstream error", "Apple authentication, response parsing, availability, or timeout failure; this is not an empty result.")]
    if "{locale}" in route:
        error_rows.insert(4, ("409", "ambiguous_selection", "More than one localization matches the selected parent and locale."))
    body += section("errors", "Errors", table(error_rows, "HTTP"))
    source_body = '<p>The adapter calls ' + code("GET " + contract.get("runtimeApplePath", endpoint["applePath"])) + '. ' + link(endpoint["source"], "Read Apple’s original endpoint ↗") + "</p>"
    if implemented and contract.get("runtimeApplePath") != endpoint["applePath"]:
        source_body += '<p>The implemented facade resolves the review contact through the selected version’s related endpoint. Clients do not supply a review-detail ID.</p>'
    if contract.get("queryFrom"):
        other = ENDPOINTS[contract["queryFrom"]]
        source_body += '<p>Filtered requests use ' + code("GET " + other["applePath"]) + ' with ' + code("filter[app]") + ' supplied from the path. ' + link(other["source"], "Apple’s filter reference ↗") + "</p>"
    if "{locale}" in route:
        source_body += '<p>Resolve the parent’s localization collection using the exact locale, validate ownership, and then supply the resolved ID to this Apple endpoint.</p>'
    if shape["kind"] == "codes":
        source_body += '<p>This endpoint returns ' + code("text/csv") + '. The proposed adapter parses complete, validated records into JSON strings and pages an immutable decoded snapshot.</p>'
    elif shape["kind"] == "metrics":
        source_body += '<p>Apple returns ' + code(shape["upstreamMime"]) + '. The adapter retains the metric-series structure, removes transport envelopes where present, and exposes dimension linkage as IDs.</p>'
    elif shape["kind"] in ("ids", "id"):
        source_body += '<p>Keep the related ID values and remove Apple’s resource-type objects. Collections use the facade cursor rather than an upstream pagination URL.</p>'
    else:
        source_body += '<p>Flatten Apple’s ' + code("attributes") + ' into each resource object. Expose supplied singleton linkage as IDs, with no automatic relationship traversal. Read access does not make these fields writable.</p>'
    if contract["parameters"]:
        mapping_rows = [(p["name"], "query", code(p["appleName"]) if p["appleName"] else "Facade-owned pagination") for p in contract["parameters"]]
        source_body += table(mapping_rows, "Facade parameter")
    if shape["modelName"] == "AppStoreReviewDetail":
        source_body += '<p>The review-contact view returns only the four contact fields. Review notes and demo-account credentials remain outside this view.</p>'
    body += section("apple-mapping", "Apple mapping", '<details class="source-details"><summary>Upstream route and translation</summary>' + source_body + '<p class="response-caption">Reviewed ' + esc(SOURCE["reviewedAt"]) + ' · OpenAPI ' + esc(SOURCE["specification"]["version"]) + "</p></details>")
    related = [e for e in entries if e["url"] != page_url(endpoint["slug"]) and e["topic"] == endpoint["topic"]][:3]
    if related:
        body += section("related", "Related reads", resource_rows(related))
    toc = [("overview", title), ("request", "Request")]
    if path_rows: toc.append(("path-parameters", "Path parameters"))
    if parameter_rows: toc.append(("query-parameters", "Query parameters"))
    toc.extend([("response", "Response"), ("response-fields", "Response fields"), ("behavior", "Behavior")])
    if collection: toc.append(("pagination", "Pagination"))
    toc.extend([("errors", "Errors"), ("apple-mapping", "Apple mapping")])
    write_page(page_url(endpoint["slug"]), title, contract["summary"], body, entries, toc, group, topics, domains)


def build(examples_only=False):
    contracts = {slug: make_contract(e) for slug, e in ENDPOINTS.items() if not examples_only or slug in OVERRIDES}
    topic_lookup = {t["slug"]: t for t in SOURCE["topics"]}
    entries = [{"title": c["title"], "route": c["route"], "url": page_url(slug), "source": ENDPOINTS[slug]["source"], "applePath": ENDPOINTS[slug]["applePath"], "group": topic_lookup[ENDPOINTS[slug]["topic"]]["title"], "topic": ENDPOINTS[slug]["topic"], "keywords": " ".join([slug, c["stage"], *c["shape"]["fields"], *(p["name"] for p in c["parameters"])])} for slug, c in contracts.items()]
    preferred_topics = ["apps", "app-infos", "app-info-localizations", "app-store-versions", "app-store-version-localizations", "app-categories", "age-ratings", "builds", "build-bundles", "build-uploads", "in-app-purchases", "in-app-purchase-versions", "subscription-groups", "subscriptions", "subscription-versions", "customer-reviews"]
    priority = {slug: index for index, slug in enumerate(preferred_topics)}
    topics = sorted(SOURCE["topics"], key=lambda t: (int(t["domain"].split("-")[-1]), priority.get(t["slug"], len(priority)), t["title"].lower())) if not examples_only else None
    domains = SOURCE["domains"] if not examples_only else None
    if not examples_only:
        build_guides(entries, topics, domains)
    for slug, contract in contracts.items():
        render_endpoint(ENDPOINTS[slug], contract, entries, topics, domains)
    (DIST / "docs/reference-index.json").write_text(json.dumps(entries, indent=2) + "\n")
    if examples_only:
        body = header("API reference", "Plain JSON, readable routes, and explicit resource targets. Start with these example GET contracts.") + section("endpoints", "GET endpoints", resource_rows(entries))
        write_page("/docs/reference/", "API reference", "Parsequerade proposed GET API reference.", body, entries, [("overview", "API reference"), ("endpoints", "GET endpoints")])
    else:
        build_reference(entries, contracts, topics, domains)
    print(f"Generated {len(contracts)} GET pages" + ("." if examples_only else f", {len(topics)} topic pages, three guides, and search/source indexes."))


class Outline(HTMLParser):
    def __init__(self):
        super().__init__()
        self.last_id = "overview"
        self.heading = None
        self.items = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if attrs.get("id"):
            self.last_id = attrs["id"]
        if tag in ("h1", "h2"):
            self.heading = [self.last_id, ""]

    def handle_data(self, data):
        if self.heading is not None:
            self.heading[1] += data

    def handle_endtag(self, tag):
        if tag in ("h1", "h2") and self.heading is not None:
            self.items.append(tuple(self.heading))
            self.heading = None


def build_guides(entries, topics, domains):
    guides = [("conventions", "/docs/conventions/", "API conventions"), ("introduction", "/docs/", "Introduction"), ("quickstart", "/docs/quickstart/", "Quickstart")]
    # Make every guide destination available before rendering sidebar links.
    for _, url, _ in guides:
        target = DIST / url.lstrip("/") / "index.html"
        target.parent.mkdir(parents=True, exist_ok=True)
        if not target.exists(): target.write_text("")
    for slug, url, title in guides:
        article = (ROOT / "docs/guides" / (slug + ".html")).read_text()
        outline = Outline()
        outline.feed(article)
        write_page(url, title, "Parsequerade’s App Store Connect facade: " + title.lower() + ".", article, entries, outline.items, "Get started", topics, domains)


def topic_url(topic):
    return "/docs/reference/topics/" + topic["slug"] + "/"


def build_reference(entries, contracts, topics, domains):
    body = header("API reference", f"{len(entries)} GET reference pages for App Store Connect, with readable routes, flat JSON, and connection-scoped access.")
    body += '<p class="contract-note">Core reads are implemented; other adapters remain proposed. Each page identifies implementation status and delivery stage. ' + link("/docs/openapi.json", "Download OpenAPI for implemented routes") + '.</p>'
    toc = [("overview", "API reference")]
    for domain in domains:
        members = [t for t in topics if t["domain"] == domain["id"]]
        rows = '<div class="topic-list">' + "".join(f'<a class="topic-row" href="{topic_url(t)}"><span><strong>{esc(t["title"])}</strong></span><span class="nav-count">{len(t["endpoints"])} GET</span></a>' for t in members) + "</div>"
        body += section(domain["id"], domain["title"], rows)
        toc.append((domain["id"], domain["title"]))
    source = '<p>Reviewed ' + esc(SOURCE["reviewedAt"]) + ' from ' + link(SOURCE["source"], "Apple’s App Store documentation") + ' and OpenAPI ' + esc(SOURCE["specification"]["version"]) + '. All GET topic links are included, including related-ID and legacy endpoints.</p>'
    source += '<p>' + link("/docs/coverage.json", "Download the source coverage manifest") + ' · ' + link("/docs/conventions/", "Read API conventions") + "</p>"
    body += section("sources", "Sources", source)
    toc.append(("sources", "Sources"))
    write_page("/docs/reference/", "API reference", "Parsequerade GET reference with implemented and proposed routes for Apple’s App Store documentation.", body, entries, toc, "Reference", topics, domains)
    for topic in topics:
        members = [e for e in entries if e["topic"] == topic["slug"]]
        body = header(topic["title"], f"{len(members)} GET contracts for this resource, including related data and IDs. Endpoint pages identify implemented and proposed adapters.", "Reference")
        toc = [("overview", topic["title"])]
        groups = [("resources", "Resource reads", lambda e: not contracts[e["url"].strip("/").split("/")[-1]]["legacy"] and contracts[e["url"].strip("/").split("/")[-1]]["shape"]["kind"] not in ("ids", "id")), ("ids", "Related IDs", lambda e: not contracts[e["url"].strip("/").split("/")[-1]]["legacy"] and contracts[e["url"].strip("/").split("/")[-1]]["shape"]["kind"] in ("ids", "id")), ("legacy", "Legacy reads", lambda e: contracts[e["url"].strip("/").split("/")[-1]]["legacy"])]
        for identifier, title, predicate in groups:
            group_entries = [e for e in members if predicate(e)]
            if group_entries:
                body += section(identifier, title, resource_rows(group_entries))
                toc.append((identifier, title))
        body += section("source", "Source", '<p>' + link(topic["source"], "Apple’s " + topic["title"] + " documentation ↗") + ' · ' + link("/docs/reference/", "All GET topics") + "</p>")
        toc.append(("source", "Source"))
        write_page(topic_url(topic), topic["title"], "GET contracts for " + topic["title"] + ".", body, entries, toc, "Reference", topics, domains)
    coverage = {"source": SOURCE["source"], "reviewedAt": SOURCE["reviewedAt"], "specification": SOURCE["specification"], "counts": SOURCE["counts"], "endpoints": [{"slug": slug, "source": ENDPOINTS[slug]["source"], "sourceSha256": ENDPOINTS[slug]["sourceSha256"], "applePath": ENDPOINTS[slug]["applePath"], "facadeRoute": c["route"], "stage": c["stage"], "implementationStatus": c["implementationStatus"], "legacy": c["legacy"], "page": page_url(slug)} for slug, c in contracts.items()]}
    (DIST / "docs/coverage.json").write_text(json.dumps(coverage, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--examples", action="store_true", help="Build only the three reviewed example pages.")
    args = parser.parse_args()
    build(args.examples)
