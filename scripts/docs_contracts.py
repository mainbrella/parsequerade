"""Proposed facade GET contracts derived from reviewed resource schemas.

These are documentation contracts, not an implementation of an Apple adapter.
Existing app/version/localization routes come from rest.md. Broader resource
reads are explicitly labeled as extended designs. Upstream API versions do not
become facade API versions; distinct old resource shapes live under /legacy.
"""

import json
import re
from docs_render import ROOT, PATH_EXAMPLES, concrete_route

SOURCE = json.loads((ROOT / "docs/apple-get-reference.json").read_text())
SCHEMAS = SOURCE["schemas"]
ENDPOINTS = {endpoint["slug"]: endpoint for endpoint in SOURCE["endpoints"]}
OVERRIDES = json.loads((ROOT / "docs/facade-overrides.json").read_text())

NAMES = {
    "appInfos": "app-infos", "appInfo": "app-info",
    "appStoreVersions": "versions", "appStoreVersion": "version",
    "appInfoLocalizations": "localizations", "appStoreVersionLocalizations": "localizations",
    "appStoreReviewDetails": "review-contacts", "appStoreReviewDetail": "review-contact",
    "ageRatingDeclaration": "age-rating", "territoryAgeRatings": "territory-age-ratings",
    "appCategories": "categories", "appPricePoints": "price-points",
    "appPriceSchedule": "price-schedule", "appPriceSchedules": "price-schedules",
    "appAvailability": "availability", "appAvailabilities": "availabilities",
    "appCustomProductPages": "custom-product-pages",
    "appCustomProductPageVersions": "custom-product-page-versions",
    "appCustomProductPageLocalizations": "localizations",
    "appEvents": "events", "appEventLocalizations": "localizations",
    "appEventScreenshots": "event-screenshots", "appEventVideoClips": "event-video-clips",
    "appScreenshotSets": "screenshot-sets", "appScreenshots": "screenshots",
    "appPreviewSets": "preview-sets", "appPreviews": "previews",
    "appClips": "app-clips", "appClipDefaultExperiences": "default-experiences",
    "appClipAdvancedExperiences": "advanced-experiences",
    "appClipDefaultExperienceLocalizations": "localizations",
    "appClipDefaultExperience": "default-experience",
    "appClipAppStoreReviewDetail": "review-details",
    "appClipAppStoreReviewDetails": "app-clip-review-details",
    "appClipHeaderImages": "app-clip-header-images",
    "appClipAdvancedExperienceImages": "advanced-experience-images",
    "appStoreVersionExperiments": "experiments",
    "appStoreVersionExperimentTreatments": "treatments",
    "appStoreVersionExperimentTreatmentLocalizations": "localizations",
    "appStoreVersionPhasedRelease": "phased-release",
    "appStoreVersionSubmission": "submission",
    "appStoreReviewAttachments": "review-attachments",
    "appEncryptionDeclarations": "encryption-declarations",
    "appEncryptionDeclaration": "encryption-declaration",
    "appEncryptionDeclarationDocuments": "encryption-documents",
    "appEncryptionDeclarationDocument": "encryption-document",
    "appAssetLibraries": "asset-libraries", "assetLibrary": "asset-library",
    "appAssetLibraryImages": "library-images", "appAssetLibraryVideos": "library-videos",
    "appAssetLibraryPlacements": "library-placements", "appAssetLibraryRefData": "asset-reference-data",
    "appTags": "tags", "routingAppCoverage": "routing-coverage", "routingAppCoverages": "routing-coverages",
    "appStoreVersionPromotions": "version-promotions",
    "endUserLicenseAgreements": "license-agreements", "endUserLicenseAgreement": "license-agreement",
    "inAppPurchases": "in-app-purchases", "inAppPurchaseLocalizations": "localizations",
    "inAppPurchaseVersions": "purchase-versions", "inAppPurchaseVersion": "purchase-version",
    "inAppPurchaseImages": "purchase-images", "inAppPurchasePricePoints": "purchase-price-points",
    "inAppPurchasePriceSchedules": "purchase-price-schedules", "iapPriceSchedule": "price-schedule",
    "inAppPurchaseAvailability": "availability", "inAppPurchaseAvailabilities": "purchase-availabilities",
    "inAppPurchaseAppStoreReviewScreenshots": "purchase-review-screenshots",
    "appStoreReviewScreenshot": "review-screenshot",
    "inAppPurchaseContents": "purchase-contents",
    "inAppPurchaseOfferCodes": "purchase-offer-codes",
    "inAppPurchaseOfferCodeCustomCodes": "purchase-custom-codes",
    "inAppPurchaseOfferCodeOneTimeUseCodes": "purchase-code-batches",
    "subscriptionLocalizations": "localizations", "subscriptionGroupLocalizations": "localizations",
    "subscriptionImages": "subscription-images", "subscriptionAppStoreReviewScreenshots": "subscription-review-screenshots",
    "subscriptionOfferCodeOneTimeUseCodes": "subscription-code-batches",
    "subscriptionOfferCodeCustomCodes": "subscription-custom-codes",
    "subscriptionAvailability": "availability", "subscriptionAvailabilities": "subscription-availabilities",
    "subscriptionPlanAvailabilities": "plan-availabilities",
    "subscriptionGracePeriods": "grace-periods", "subscriptionGracePeriod": "grace-period",
    "preReleaseVersions": "prerelease-versions", "preReleaseVersion": "prerelease-version",
    "ciProduct": "xcode-cloud-product", "betaAppReviewDetail": "beta-review-details",
    "betaLicenseAgreement": "beta-license-agreement", "betaAppLocalizations": "beta-localizations",
    "perfPowerMetrics": "power-performance-metrics", "betaTesterUsages": "beta-tester-usages",
    "betaBuildUsages": "beta-build-usages", "nominations": "featuring-nominations",
}

ROOT_NAMES = {
    "appInfoLocalizations": "app-info-localizations",
    "appStoreVersionLocalizations": "version-localizations",
    "appCustomProductPageLocalizations": "product-page-localizations",
    "appEventLocalizations": "event-localizations",
    "appClipDefaultExperienceLocalizations": "app-clip-localizations",
    "appClipDefaultExperiences": "app-clip-default-experiences",
    "appClipAdvancedExperiences": "app-clip-advanced-experiences",
    "appStoreVersionExperimentTreatments": "experiment-treatments",
    "appStoreVersionExperimentTreatmentLocalizations": "treatment-localizations",
    "inAppPurchaseLocalizations": "purchase-localizations",
    "subscriptionLocalizations": "subscription-localizations",
    "subscriptionGroupLocalizations": "group-localizations",
}

IDS = {
    "apps": "appId", "appInfos": "appInfoId", "appStoreVersions": "versionId",
    "builds": "buildId", "appCategories": "categoryId", "territories": "territoryId",
    "appCustomProductPages": "pageId", "appCustomProductPageVersions": "pageVersionId",
    "appEvents": "eventId", "appClips": "appClipId",
    "appClipDefaultExperiences": "experienceId", "appClipAdvancedExperiences": "experienceId",
    "appStoreVersionExperiments": "experimentId", "appStoreVersionExperimentTreatments": "treatmentId",
    "inAppPurchases": "purchaseId", "inAppPurchaseVersions": "purchaseVersionId",
    "subscriptions": "subscriptionId", "subscriptionVersions": "subscriptionVersionId",
    "subscriptionGroups": "groupId", "subscriptionGroupVersions": "groupVersionId",
    "appPricePoints": "pricePointId", "inAppPurchasePricePoints": "pricePointId", "subscriptionPricePoints": "pricePointId",
    "appPriceSchedules": "scheduleId", "inAppPurchasePriceSchedules": "scheduleId",
    "appAvailabilities": "availabilityId", "inAppPurchaseAvailabilities": "availabilityId", "subscriptionAvailabilities": "availabilityId",
    "customerReviews": "reviewId", "customerReviewResponses": "responseId",
}

CORE_BASES = {
    "appInfos": "/apps/{appId}/app-infos/{appInfoId}",
    "appStoreVersions": "/apps/{appId}/versions/{versionId}",
    "builds": "/apps/{appId}/builds/{buildId}",
    "appInfoLocalizations": "/apps/{appId}/app-infos/{appInfoId}/localizations/{locale}",
    "appStoreVersionLocalizations": "/apps/{appId}/versions/{versionId}/localizations/{locale}",
    "appStoreReviewDetails": "/apps/{appId}/versions/{versionId}/review-contact",
}

LOCALE_BASES = {
    "appCustomProductPageLocalizations": "/custom-product-page-versions/{pageVersionId}/localizations/{locale}",
    "appClipDefaultExperienceLocalizations": "/app-clip-default-experiences/{experienceId}/localizations/{locale}",
    "appStoreVersionExperimentTreatmentLocalizations": "/experiment-treatments/{treatmentId}/localizations/{locale}",
    "appEventLocalizations": "/events/{eventId}/localizations/{locale}",
    "inAppPurchaseLocalizations": "/purchase-versions/{purchaseVersionId}/localizations/{locale}",
    "subscriptionLocalizations": "/subscription-versions/{subscriptionVersionId}/localizations/{locale}",
    "subscriptionGroupLocalizations": "/subscription-group-versions/{groupVersionId}/localizations/{locale}",
}

MAX_VERSIONS = {}
for endpoint in ENDPOINTS.values():
    version, resource = endpoint["applePath"].split("/")[1:3]
    MAX_VERSIONS[resource] = max(MAX_VERSIONS.get(resource, 0), int(version[1:]))


def resolve(schema):
    if "$ref" in schema:
        return {**SCHEMAS[schema["$ref"].rsplit("/", 1)[1]], **{k: v for k, v in schema.items() if k != "$ref"}}
    return schema


def normalize_name(value, root=False):
    value = re.sub(r"V\d+$", "", value)
    if root and value in ROOT_NAMES:
        return ROOT_NAMES[value]
    if value in NAMES:
        return NAMES[value]
    return re.sub(r"(?<!^)(?=[A-Z])", "-", value).lower()


def identifier(resource):
    if resource in IDS:
        return IDS[resource]
    name = normalize_name(resource, True)
    name = name[:-3] + "y" if name.endswith("ies") else name.removesuffix("s")
    words = name.split("-")
    return words[0] + "".join(word.title() for word in words[1:]) + "Id"


def response_model(endpoint):
    media = endpoint["response"]
    mime = "application/json" if "application/json" in media else next((m for m in media if "+json" in m), next(iter(media)))
    envelope = resolve(media[mime]["schema"])
    wrapped = "data" in envelope.get("properties", {})
    data = resolve(envelope["properties"]["data"]) if wrapped else envelope
    many = data.get("type") == "array"
    raw_model = data.get("items", data)
    model_name = raw_model.get("$ref", "").rsplit("/", 1)[-1]
    return mime, many, resolve(raw_model), model_name, wrapped


def route_for(endpoint):
    path = endpoint["applePath"]
    version, resource, *tail = path.lstrip("/").split("/")
    _, many, _, _, _ = response_model(endpoint)
    legacy = endpoint["deprecated"] or int(version[1:]) < MAX_VERSIONS[resource]
    root = normalize_name(resource, True)
    if tail and tail[0] == "{id}":
        if resource in CORE_BASES:
            route = CORE_BASES[resource]
        elif resource in LOCALE_BASES:
            route = LOCALE_BASES[resource]
            if int(version[1:]) == 1 and resource in ("inAppPurchaseLocalizations", "subscriptionLocalizations", "subscriptionGroupLocalizations"):
                parent, parent_id = {"inAppPurchaseLocalizations": ("in-app-purchases", "purchaseId"), "subscriptionLocalizations": ("subscriptions", "subscriptionId"), "subscriptionGroupLocalizations": ("subscription-groups", "groupId")}[resource]
                route = f"/{parent}/{{{parent_id}}}/localizations/{{locale}}"
        else:
            route = f"/{root}/{{{identifier(resource)}}}"
        remaining = tail[1:]
        relationship = "relationships" in remaining
        route += "".join("/" + normalize_name(part) for part in remaining if part != "relationships")
        if relationship:
            route += "/ids" if many else "/id"
    elif resource == "builds":
        route = "/apps/{appId}/builds"
    elif resource in CORE_BASES:
        route = CORE_BASES[resource].rsplit("/", 1)[0]
    else:
        route = "/" + root + "".join("/" + normalize_name(part) for part in tail)
    if legacy:
        route = "/legacy" + route
    return route, legacy


def relationship_field(name):
    if name == "appStoreVersion":
        return "versionId"
    if name == "inAppPurchaseV2":
        return "purchaseId"
    if name == "appCustomProductPageVersion":
        return "pageVersionId"
    if name == "appClipDefaultExperience":
        return "experienceId"
    return re.sub(r"V\d+$", "", name) + "Id"


def response_shape(endpoint, contract):
    mime, collection, model, model_name, wrapped = response_model(endpoint)
    is_linkage = "/relationships/" in endpoint["applePath"]
    fields = {}
    optional_relations = set()
    if mime == "text/csv":
        kind = "codes"
        item_schema = {"type": "string"}
        collection = True
    elif is_linkage:
        kind = "ids" if collection else "id"
        item_schema = {"type": "string"} if collection else {"type": "object", "properties": {"id": {"type": "string", "nullable": True}}, "required": ["id"]}
        if not collection:
            fields = item_schema["properties"]
    elif "attributes" in model.get("properties", {}):
        kind = "resource"
        fields = {"id": {"type": "string"}}
        contexts = {}
        own = {"App": "appId", "AppInfo": "appInfoId", "AppStoreVersion": "versionId"}.get(model_name)
        for name in ("appId", "appInfoId", "versionId"):
            if "{" + name + "}" in contract["route"] and name != own:
                contexts[name] = PATH_EXAMPLES[name]
        contexts.update(contract.get("context", {}))
        contract["context"] = contexts
        fields.update({name: {"type": "string"} for name in contexts})
        attributes = resolve(model["properties"]["attributes"]).get("properties", {})
        fields.update({name: schema for name, schema in attributes.items() if not schema.get("deprecated") or contract["legacy"]})
        relationships = resolve(model["properties"].get("relationships", {})).get("properties", {})
        for name, relationship in relationships.items():
            data = resolve(relationship).get("properties", {}).get("data", {})
            if data and resolve(data).get("type") == "object":
                field = relationship_field(name)
                if field not in fields:
                    fields[field] = {"type": "string", "nullable": True}
                    optional_relations.add(field)
        # The initial contact view deliberately preserves private review fields.
        if model_name == "AppStoreReviewDetail":
            allowlist = {"id", "appId", "versionId", "contactFirstName", "contactLastName", "contactPhone", "contactEmail"}
            fields = {name: schema for name, schema in fields.items() if name in allowlist}
            optional_relations.intersection_update(fields)
        fields.pop("demoAccountPassword", None)
        item_schema = {"type": "object", "properties": fields, "required": ["id", *contexts]}
    else:
        kind = "metrics"
        fields = dict(model.get("properties", {}))
        # Metric dimensions are identifiers, not API URLs or resource envelopes.
        if "dimensions" in fields:
            dimensions = resolve(fields["dimensions"])
            plain_dimensions = {}
            for name, dimension in dimensions.get("properties", {}).items():
                data = resolve(dimension).get("properties", {}).get("data", {"type": "string"})
                plain_dimensions[name] = {"type": "object", "properties": {"id": data}}
            fields["dimensions"] = {"type": "object", "properties": plain_dimensions}
        item_schema = {**model, "properties": fields}
    if collection:
        schema = {"type": "object", "properties": {"items": {"type": "array", "items": item_schema}, "nextCursor": {"type": "string", "nullable": True}}, "required": ["items", "nextCursor"]}
    else:
        schema = item_schema
    return {"kind": kind, "collection": collection, "fields": fields, "optionalRelations": sorted(optional_relations), "schema": schema, "itemSchema": item_schema, "upstreamMime": mime, "modelName": model_name, "wrapped": wrapped}


FILTER_NAMES = {
    "app": "appId", "appStoreVersion": "versionId", "appStoreVersions": "versionId",
    "appInfo": "appInfoId", "subscriptionGroup": "groupId", "inAppPurchase": "purchaseId",
    "inAppPurchaseV2": "purchaseId", "appVersionState": "state",
    "appStoreVersions.appVersionState": "versionState", "appStoreVersions.platform": "versionPlatform",
    "reviewSubmissions.state": "reviewState", "reviewSubmissions.platform": "reviewPlatform",
    "preReleaseVersion.platform": "platform", "preReleaseVersion.version": "prereleaseVersion",
    "betaAppReviewSubmission.betaReviewState": "betaReviewState",
    "preReleaseVersion": "prereleaseVersionId", "betaGroups": "betaGroupId",
    "territory": "territory", "subscription": "subscriptionId", "build": "buildId",
}


def query_parameters(endpoint, contract, shape):
    source = ENDPOINTS.get(contract.get("queryFrom"), endpoint)
    result = []
    seen = set()
    fields_added = False
    for parameter in source["parameters"]:
        raw_name = parameter["name"]
        if parameter["in"] != "query" or parameter.get("deprecated"):
            continue
        if raw_name == "include" or raw_name.startswith("limit["):
            continue
        if raw_name.startswith("fields["):
            # Only fields on the resource being returned are client-selectable.
            values = resolve(parameter["schema"]).get("items", {}).get("enum", [])
            attributes = resolve(response_model(endpoint)[2].get("properties", {}).get("attributes", {})).get("properties", {})
            allowed = [v for v in values if v in attributes and v in shape["fields"]]
            if not allowed or fields_added:
                continue
            name = "fields"
            schema = {"type": "array", "items": {"type": "string", "enum": allowed}}
            fields_added = True
        else:
            name = raw_name
            schema = parameter["schema"]
            if raw_name.startswith("filter["):
                filter_name = raw_name[7:-1]
                if filter_name in ("appStoreState", "appStoreVersions.appStoreState"):
                    continue
                name = FILTER_NAMES.get(filter_name, filter_name)
                if "{" + name + "}" in contract["route"]:
                    continue
            elif raw_name.startswith("exists["):
                suffix = raw_name[7:-1]
                name = "has" + suffix[0].upper() + suffix[1:]
        if name in seen:
            raise ValueError(f"Query alias collision for {contract['route']}: {name}")
        seen.add(name)
        item = {"name": name, "schema": schema, "required": parameter.get("required", False), "appleName": raw_name}
        if name == "limit":
            item["default"] = min(50, resolve(schema).get("maximum", 200))
        result.append(item)
    if shape["collection"]:
        if "limit" not in seen:
            result.append({"name": "limit", "schema": {"type": "integer", "minimum": 1, "maximum": 200}, "required": False, "default": 50, "appleName": None})
        result.append({"name": "cursor", "schema": {"type": "string"}, "required": False, "appleName": None})
    # Apple's discussion establishes conditional requirements beyond OpenAPI.
    if endpoint["slug"] == "get-v1-subscriptions-_id_-pricepoints":
        for parameter in result:
            if parameter["name"] == "territory":
                parameter["required"] = True
    return result


EXAMPLE_VALUES = {
    **PATH_EXAMPLES, "id": "resource_123", "name": "Example Notes", "bundleId": "com.example.notes",
    "sku": "example-ios", "primaryLocale": "en-US", "locale": "en-US", "version": "42", "versionString": "1.2.3",
    "description": "Keep your notes organized.", "subtitle": "Notes that stay with you",
    "keywords": "notes,productivity,writing", "supportUrl": "https://example.com/support",
    "marketingUrl": "https://example.com", "privacyPolicyUrl": "https://example.com/privacy",
    "privacyChoicesUrl": "https://example.com/privacy/choices", "url": "https://example.com",
    "promotionalText": "A clearer place for your ideas.", "whatsNew": "Faster search and improved offline editing.",
    "copyright": "2026 Example Company", "referenceName": "Example subscription", "productId": "com.example.notes.pro",
    "contactFirstName": "Alex", "contactLastName": "Example", "contactEmail": "review@example.com", "contactPhone": "+1 415 555 0100",
    "territory": "USA", "territoryId": "USA", "primaryCategoryId": "UTILITIES", "secondaryCategoryId": "PRODUCTIVITY",
    "minOsVersion": "18.0", "lsMinimumSystemVersion": "15.0", "rating": 5, "reviewerNickname": "notes_reader",
    "title": "Helpful every day", "body": "Search makes it easy to find my notes.", "responseBody": "Thank you for your feedback.",
    "fileName": "screenshot.png", "fileSize": 184320, "width": 1290, "height": 2796,
    "customerPrice": "1.99", "proceeds": "1.40", "proceedsYear2": "1.40", "currency": "USD", "currencyCode": "USD",
    "uploadedDate": "2026-10-13T18:00:00Z", "expirationDate": "2027-01-11T18:00:00Z",
    "startDate": "2026-10-14", "endDate": "2026-10-21", "createdDate": "2026-10-14T12:00:00Z",
    "code": "EXAMPLE-OFFER-CODE", "identifier": "com.example.notes", "nextCursor": None,
}


def example_value(schema, name="", depth=0):
    schema = resolve(schema)
    for keyword in ("oneOf", "anyOf", "allOf"):
        if schema.get(keyword):
            return example_value(schema[keyword][0], name, depth + 1)
    kind = schema.get("type", "object")
    values = schema.get("enum", [])
    if values:
        preferred = ["IOS", "PREPARE_FOR_SUBMISSION", "VALID", "APP_STORE_ELIGIBLE", "DOES_NOT_USE_THIRD_PARTY_CONTENT", "NONE", "NOT_APPLICABLE", "MANUAL", "MONTHLY"]
        return next((value for value in preferred if value in values), values[0])
    if name in EXAMPLE_VALUES:
        value = EXAMPLE_VALUES[name]
        valid_type = (kind == "string" and isinstance(value, str)) or (kind == "integer" and isinstance(value, int) and not isinstance(value, bool)) or (kind == "boolean" and isinstance(value, bool)) or (value is None and schema.get("nullable"))
        if valid_type and (not schema.get("format") or schema.get("format") not in ("date", "date-time")):
            return value
    if kind == "string":
        if schema.get("format") == "date-time":
            return EXAMPLE_VALUES.get(name, "2026-10-14T12:00:00Z")
        if schema.get("format") == "date":
            return "2026-10-14"
        if schema.get("format") in ("uri", "uri-reference"):
            return "https://example.com/asset"
        if name.endswith("Id"):
            return concrete_route("{" + name + "}")
        if "price" in name.lower() or "proceeds" in name.lower():
            return "1.99"
        return "example" if not name else re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower() + "_example"
    if kind == "boolean":
        return False
    if kind in ("integer", "number"):
        value = max(1, schema.get("minimum", 0))
        return min(value, schema.get("maximum", value))
    if kind == "array":
        return [] if depth > 4 else [example_value(schema.get("items", {}), name.removesuffix("s"), depth + 1)]
    properties = schema.get("properties", {})
    names = list(dict.fromkeys([*schema.get("required", []), *list(properties)[:6]]))
    return {key: example_value(properties[key], key, depth + 1) for key in names if key in properties and key not in ("links", "demoAccountPassword")}


def response_example(endpoint, contract, shape):
    if "example" in contract:
        return contract["example"]
    if shape["kind"] in ("ids", "codes"):
        child = endpoint["applePath"].rsplit("/", 1)[-1]
        item = "EXAMPLE-OFFER-CODE" if shape["kind"] == "codes" else concrete_route("{" + identifier(child) + "}")
    elif shape["kind"] == "id":
        child = endpoint["applePath"].rsplit("/", 1)[-1]
        item = {"id": concrete_route("{" + child.removesuffix("V2") + "Id}")}
    else:
        item = example_value(shape["itemSchema"])
        if shape["kind"] == "resource":
            resource = endpoint["applePath"].split("/")[2]
            item["id"] = PATH_EXAMPLES.get(identifier(resource), re.sub(r"(?<!^)(?=[A-Z])", "_", shape["modelName"]).lower().removesuffix("_v2") + "_123")
            item.update(contract.get("context", {}))
            if "locale" in shape["fields"]:
                item["locale"] = "en-US"
    return {"items": [item], "nextCursor": None} if shape["collection"] else item


def sentence_title(title):
    if title.startswith("GET "):
        return "List in-app purchase IDs"
    title = title[:1].upper() + title[1:].lower()
    for text in ("App Store", "App Clip", "Game Center", "Xcode Cloud", "Android", "iOS", "TestFlight"):
        title = re.sub(re.escape(text), text, title, flags=re.IGNORECASE)
    title = re.sub(r"\bids\b", "IDs", title)
    title = re.sub(r"\bid\b", "ID", title)
    return title


def make_contract(endpoint):
    override = dict(OVERRIDES.get(endpoint["slug"], {}))
    route, legacy = route_for(endpoint)
    contract = {"route": override.get("route", route), "legacy": legacy, "title": override.get("title", sentence_title(endpoint["title"])), "stage": override.get("stage", "Legacy read design" if legacy else "Extended read design"), "notes": list(override.get("notes", [])), **override}
    if endpoint["slug"] in ("get-v1-builds", "get-v1-apps-_id_-builds"):
        contract["queryFrom"] = "get-v1-builds"
        contract["stage"] = "Read parity"
    shape = response_shape(endpoint, contract)
    contract["shape"] = shape
    contract["parameters"] = query_parameters(endpoint, contract, shape)
    contract["example"] = response_example(endpoint, contract, shape)
    if "summary" not in contract:
        if shape["kind"] in ("ids", "id"):
            contract["summary"] = "Read related IDs directly, without Apple’s resource-type envelope." + (" Results are paginated." if shape["collection"] else " The response contains only the related ID.")
        elif shape["kind"] == "codes":
            contract["summary"] = "Read one-time-use redemption codes as JSON strings. The service decodes Apple’s CSV download into a paginated collection."
        elif shape["kind"] == "metrics":
            contract["summary"] = "Read the selected performance data as JSON. Metric series and dimensions keep their domain structure." + (" Results use items and nextCursor." if shape["collection"] else "")
        else:
            contract["summary"] = ("Read a page of " if shape["collection"] else "Read ") + re.sub(r"(?<!^)(?=[A-Z])", " ", shape["modelName"]).lower().removesuffix(" v2") + (" resources as flat JSON objects." if shape["collection"] else " information as a flat JSON object.")
            if "{locale}" in contract["route"]:
                contract["summary"] += " Address an exact locale; the service resolves its localization ID."
    if legacy:
        contract["notes"].append("This documents an older Apple resource contract. Its /legacy route keeps that shape separate from the current resource design; do not use it as an automatic fallback after a failed request.")
    if shape["kind"] == "codes":
        contract["notes"].append("Apple returns a CSV file for this endpoint. This JSON adapter is an extended proposal: parsing, header validation, download limits, and immutable snapshot pagination must be implemented before it can be used.")
    if "{locale}" in contract["route"] and not contract["notes"]:
        contract["notes"].append("Resolve the exact locale within the selected parent, validate ownership, and read the resolved localization. No match returns localization_not_found; multiple matches return ambiguous_selection. GET does not create a locale or substitute another language.")
    if endpoint["slug"] == "get-v1-apps-_id_-metrics-betatesterusages":
        contract["notes"].append("Provide either groupBy or betaTesters. Apple requires at least one of these parameters.")
        contract["queryExample"] = {"groupBy": "betaTesters", "period": "P30D"}
    if endpoint["slug"] == "get-v1-subscriptions-_id_-pricepoints":
        contract["notes"].append("Provide territory on every request. Apple’s endpoint discussion requires it for forward compatibility even though the pinned specification marks it optional.")
    if "queryExample" not in contract:
        contract["queryExample"] = {}
        for parameter in contract["parameters"]:
            if parameter["required"]:
                schema = resolve(parameter["schema"])
                if schema.get("type") == "array": schema = schema.get("items", {})
                contract["queryExample"][parameter["name"]] = str(example_value(schema, parameter["name"]))
    if not contract["notes"]:
        contract["notes"].append("A nested resource must belong to the selected parent and the configured connection. The adapter validates ownership before returning data.")
    return contract
