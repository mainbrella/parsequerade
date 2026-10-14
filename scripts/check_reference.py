#!/usr/bin/env python3
"""Validate generated GET reference pages against the pinned source snapshot."""

import argparse
from datetime import date, datetime
from html.parser import HTMLParser
import json
import re
from pathlib import Path
from urllib.parse import urlsplit

import docs_contracts as contracts
from docs_render import DIST, page_url


class CheckFailure(Exception):
    pass


def need(ok, message):
    if not ok:
        raise CheckFailure(message)


def check_value(value, schema, where):
    schema = contracts.resolve(schema)
    for key in ("oneOf", "anyOf"):
        if key in schema:
            errors = []
            for option in schema[key]:
                try:
                    check_value(value, option, where)
                    break
                except CheckFailure as error:
                    errors.append(str(error))
            else:
                raise CheckFailure(f"{where}: no {key} branch accepts the value")
            return
    if value is None:
        need(schema.get("nullable", False), f"{where}: null is not allowed")
        return
    kind = schema.get("type", "object")
    valid = {
        "string": lambda v: isinstance(v, str),
        "integer": lambda v: isinstance(v, int) and not isinstance(v, bool),
        "number": lambda v: isinstance(v, (int, float)) and not isinstance(v, bool),
        "boolean": lambda v: isinstance(v, bool),
        "array": lambda v: isinstance(v, list),
        "object": lambda v: isinstance(v, dict),
    }.get(kind, lambda v: True)
    need(valid(value), f"{where}: expected {kind}, got {type(value).__name__}")
    if "enum" in schema:
        need(value in schema["enum"], f"{where}: {value!r} is outside the enum")
    if kind == "string":
        if schema.get("format") == "date":
            try:
                date.fromisoformat(value)
            except ValueError:
                raise CheckFailure(f"{where}: invalid ISO date {value!r}")
        elif schema.get("format") == "date-time":
            try:
                parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
                need(parsed.tzinfo is not None, f"{where}: timestamp lacks a timezone")
            except ValueError:
                raise CheckFailure(f"{where}: invalid RFC 3339 timestamp {value!r}")
        elif schema.get("format") in ("uri", "uri-reference"):
            parsed = urlsplit(value)
            need(bool(parsed.scheme and (parsed.netloc or schema.get("format") == "uri-reference")), f"{where}: invalid URI {value!r}")
        if "pattern" in schema:
            need(re.search(schema["pattern"], value) is not None, f"{where}: does not match source pattern")
    if kind in ("integer", "number"):
        if "minimum" in schema: need(value >= schema["minimum"], f"{where}: below minimum")
        if "maximum" in schema: need(value <= schema["maximum"], f"{where}: above maximum")
    if kind == "array":
        if "minItems" in schema: need(len(value) >= schema["minItems"], f"{where}: too few items")
        if "maxItems" in schema: need(len(value) <= schema["maxItems"], f"{where}: too many items")
        for index, item in enumerate(value):
            check_value(item, schema.get("items", {}), f"{where}[{index}]")
    if kind == "object":
        props = schema.get("properties", {})
        for key in schema.get("required", []):
            need(key in value, f"{where}: missing required property {key!r}")
        if schema.get("additionalProperties") is False:
            need(set(value) <= set(props), f"{where}: unexpected properties {sorted(set(value) - set(props))}")
        for key, item in value.items():
            if key in props:
                check_value(item, props[key], f"{where}.{key}")
            elif isinstance(schema.get("additionalProperties"), dict):
                check_value(item, schema["additionalProperties"], f"{where}.{key}")


def check_contracts():
    source = contracts.SOURCE
    endpoints = source["endpoints"]
    need(len(endpoints) == source["counts"]["getPagesRead"], "source endpoint count does not match pinned counts")
    need(len(source["topics"]) == source["counts"]["topicsWithGets"], "source topic count does not match pinned counts")
    example_errors = []
    for endpoint in endpoints:
        try:
            c = contracts.make_contract(endpoint)
            check_value(c["example"], c["shape"]["schema"], endpoint["slug"] + " response example")
        except CheckFailure as error:
            example_errors.append(str(error))
    need(not example_errors, "Invalid response examples:\n- " + "\n- ".join(example_errors))
    for endpoint in endpoints:
        c = contracts.make_contract(endpoint)
        where = endpoint["slug"]
        check_value(c["example"], c["shape"]["schema"], where + " response example")
        names = [p["name"] for p in c["parameters"]]
        need(len(names) == len(set(names)), f"{where}: duplicate public query parameter alias")
        required = {p["name"] for p in c["parameters"] if p["required"]}
        example = c["queryExample"]
        need(required <= set(example), f"{where}: query example omits required parameters {sorted(required - set(example))}")
        if where == "get-v1-apps-_id_-metrics-betatesterusages":
            need(bool({"groupBy", "betaTesters"} & set(example)), f"{where}: conditional groupBy/betaTesters requirement is not demonstrated")
        for parameter in c["parameters"]:
            name = parameter["name"]
            schema = contracts.resolve(parameter["schema"])
            if name == "limit":
                need(schema.get("minimum", 1) > 0, f"{where}: limit must be positive")
                source_limit = next((contracts.resolve(p["schema"]).get("maximum", 200) for p in endpoint["parameters"] if p["name"] == parameter.get("appleName")), 200)
                need(schema.get("maximum", 200) <= source_limit, f"{where}: public limit exceeds Apple's maximum {source_limit}")
                need(1 <= parameter.get("default", 50) <= schema.get("maximum", 200), f"{where}: invalid limit default")
            if name not in example:
                continue
            raw = example[name]
            if schema.get("type") == "array":
                items = raw.split(",") if isinstance(raw, str) else raw
                check_value(items, schema, where + " query " + name)
            else:
                check_value(raw, schema, where + " query " + name)
        if c["shape"]["kind"] == "resource" and "id" in c["shape"]["fields"]:
            need(c["shape"]["fields"]["id"].get("type") == "string", f"{where}: resource IDs must stay strings")
    special = {
        "get-v1-subscriptions-_id_-pricepoints": {"territory"},
    }
    for slug, required in special.items():
        c = contracts.make_contract(contracts.ENDPOINTS[slug])
        need(required <= {p["name"] for p in c["parameters"] if p["required"]}, f"{slug}: missing documented conditional requirement")
    core_routes = {
        "get-v1-apps": "/apps",
        "get-v1-apps-_id_": "/apps/{appId}",
        "get-v1-apps-_id_-appinfos": "/apps/{appId}/app-infos",
        "get-v1-appinfos-_id_": "/apps/{appId}/app-infos/{appInfoId}",
        "get-v1-appinfolocalizations-_id_": "/apps/{appId}/app-infos/{appInfoId}/localizations/{locale}",
        "get-v1-appstoreversions-_id_": "/apps/{appId}/versions/{versionId}",
        "get-v1-appstoreversionlocalizations-_id_": "/apps/{appId}/versions/{versionId}/localizations/{locale}",
        "get-v1-appstoreversions-_id_-appstorereviewdetail": "/apps/{appId}/versions/{versionId}/review-contact",
    }
    for slug, route in core_routes.items():
        need(contracts.make_contract(contracts.ENDPOINTS[slug])["route"] == route, f"{slug}: core route no longer matches rest.md")
    for slug in ("get-v1-appstoreversions-_id_-appstorereviewdetail",):
        c = contracts.make_contract(contracts.ENDPOINTS[slug])
        need(set(c["shape"]["fields"]) == {"id", "appId", "versionId", "contactFirstName", "contactLastName", "contactPhone", "contactEmail"}, f"{slug}: review contact field allowlist changed")
        need("demoAccountPassword" not in c["shape"]["fields"], f"{slug}: private demo password exposed")
    # These upstream paths intentionally converge on the same public route.
    shared = [
        ("get-v1-builds", "get-v1-apps-_id_-builds"),
        ("get-v1-appstoreversions-_id_-appstorereviewdetail", "get-v1-appstorereviewdetails-_id_"),
    ]
    for left, right in shared:
        a, b = (contracts.make_contract(contracts.ENDPOINTS[s]) for s in (left, right))
        need(a["route"] == b["route"], f"shared route mismatch: {left}, {right}")
        need(a["shape"]["schema"] == b["shape"]["schema"], f"shared route response mismatch: {left}, {right}")
        params = lambda c: [(p["name"], p["schema"], p["required"]) for p in c["parameters"]]
        need(params(a) == params(b), f"shared route query mismatch: {left}, {right}")
    csv_contracts = [contracts.make_contract(e) for e in endpoints if "text/csv" in e["response"] and not any("json" in mime for mime in e["response"])]
    need(len(csv_contracts) == 2, f"expected two CSV code collections, got {len(csv_contracts)}")
    need(all(c["shape"]["kind"] == "codes" and c["shape"]["collection"] for c in csv_contracts), "CSV endpoints must be string code collections")
    return len(endpoints)


class ResponseExampleParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.active = False
        self.depth = 0
        self.parts = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "pre" and attrs.get("id") == "response-json":
            self.active, self.depth = True, 1
        elif self.active:
            self.depth += 1

    def handle_endtag(self, tag):
        if self.active:
            self.depth -= 1
            if self.depth == 0:
                self.active = False

    def handle_data(self, data):
        if self.active: self.parts.append(data)


def check_generated():
    source = contracts.SOURCE
    runtime = json.loads((contracts.ROOT / "api/read-contracts.json").read_text())
    implemented = {r["route"] for r in runtime["routes"]}
    coverage = {e["slug"]: e for e in json.loads((DIST / "docs/coverage.json").read_text())["endpoints"]}
    entries = json.loads((DIST / "docs/reference-index.json").read_text())
    by_url = {entry.get("url"): entry for entry in entries}
    need(len(by_url) == len(entries), "search index contains duplicate URLs")
    need(len(entries) == len(source["endpoints"]), f"search index has {len(entries)} entries for {len(source['endpoints'])} endpoints")
    for endpoint in source["endpoints"]:
        url = page_url(endpoint["slug"])
        need(url in by_url, f"search index missing {url}")
        entry = by_url[url]
        need(entry.get("applePath") == endpoint["applePath"], f"search index Apple path mismatch for {url}")
        need(entry.get("source", entry.get("sourceUrl")) == endpoint["source"], f"search index source URL mismatch for {url}")
        path = DIST / url.lstrip("/") / "index.html"
        need(path.is_file(), f"missing endpoint page {path}")
        parser = ResponseExampleParser()
        html = path.read_text()
        parser.feed(html)
        contract = contracts.make_contract(endpoint)
        status = "implemented" if contract["route"] in implemented else "planned"
        need(contract["implementationStatus"] == status, f"{path}: implementation status differs from runtime")
        need(coverage[endpoint["slug"]]["implementationStatus"] == status, f"{path}: coverage status differs from runtime")
        need(('>Implemented</span>' if status == "implemented" else '>Proposed</span>') in html,
             f"{path}: rendered implementation label differs from runtime")
        need(bool(parser.parts), f"{path}: missing response-json example")
        try:
            rendered = json.loads("".join(parser.parts))
        except json.JSONDecodeError as error:
            raise CheckFailure(f"{path}: invalid response example JSON: {error}")
        expected = contracts.make_contract(endpoint)["example"]
        need(rendered == expected, f"{path}: rendered response example differs from contract")
    for topic in source["topics"]:
        path = DIST / "docs/reference/topics" / topic["slug"] / "index.html"
        need(path.is_file(), f"missing topic page {path}")
    return len(entries), len(source["topics"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--contracts-only", action="store_true", help="validate contracts without requiring generated HTML")
    args = parser.parse_args()
    try:
        endpoints = check_contracts()
        if args.contracts_only:
            print(f"Validated {endpoints} GET contracts against the pinned OpenAPI snapshot.")
        else:
            indexed, topics = check_generated()
            print(f"Validated {endpoints} GET contracts, {indexed} generated endpoint pages, and {topics} topic pages.")
    except (CheckFailure, KeyError, ValueError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
