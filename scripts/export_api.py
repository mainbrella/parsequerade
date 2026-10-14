#!/usr/bin/env python3
"""Export implemented read contracts and facade OpenAPI, offline. --check detects drift."""

import argparse
import json
from docs_contracts import ENDPOINTS, SOURCE, make_contract, relationship_field, resolve
from docs_render import ROOT


def expanded(value, read=False):
    if isinstance(value, list):
        return [expanded(v, read) for v in value]
    if not isinstance(value, dict):
        return value
    value = resolve(value)
    # Read enum values can evolve independently of this versioned facade.
    return {k: expanded(v, read) for k, v in value.items() if k != "$ref" and not (read and k == "enum")}


def outputs():
    routes = []
    paths = {}
    for entry in json.loads((ROOT / "api/read-routes.json").read_text()):
        endpoint = ENDPOINTS[entry["slug"]]
        c = make_contract(endpoint)
        model = SOURCE["schemas"][c["shape"]["modelName"]]
        attributes = resolve(model["properties"].get("attributes", {})).get("properties", {})
        relationships = resolve(model["properties"].get("relationships", {})).get("properties", {})
        linkage = {}
        for name, schema in relationships.items():
            field = relationship_field(name)
            data = resolve(schema).get("properties", {}).get("data", {})
            if field in c["shape"]["optionalRelations"]:
                linkage[name] = {"field": field, "type": resolve(data)["properties"]["type"]["enum"][0]}
        route = {
            **entry, "route": c["route"], "applePath": endpoint["applePath"],
            "type": c["shape"]["modelName"][0].lower() + c["shape"]["modelName"][1:] + "s",
            "collection": c["shape"]["collection"],
            "attributes": {k: expanded(v, True) for k, v in attributes.items() if k in c["shape"]["fields"]},
            "relationships": linkage,
            "parameters": expanded(c["parameters"]),
            "responseSchema": expanded(c["shape"]["schema"], True),
        }
        routes.append(route)
        parameters = [{"name": "connectionId", "in": "path", "required": True, "schema": {"type": "string"}}]
        for name in [part[1:-1] for part in c["route"].split("/") if part.startswith("{")]:
            parameters.append({"name": name, "in": "path", "required": True, "schema": {"type": "string"}})
        for p in route["parameters"]:
            item = {"name": p["name"], "in": "query", "required": p["required"], "schema": p["schema"]}
            if p["schema"].get("type") == "array":
                item.update(style="form", explode=False)
            if "default" in p:
                item["schema"] = {**item["schema"], "default": p["default"]}
            parameters.append(item)
        paths["/v1/connections/{connectionId}" + c["route"]] = {"get": {
            "operationId": entry["handler"], "summary": c["title"],
            "description": c["summary"], "parameters": parameters,
            "responses": {"200": {"description": "Flat resource or bounded collection.", "content": {
                "application/json": {"schema": route["responseSchema"], "example": c["example"]}}},
                **{str(status): {"description": description, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}}
                   for status, description in [(400, "Invalid query or cursor."), (401, "Facade authentication failed."),
                       (403, "Facade or Apple permission denied."), (404, "Resource or locale not found."),
                       (405, "Only GET is implemented."), (409, "Ambiguous locale."), (422, "Apple validation failed."),
                       (429, "Rate limited; respect Retry-After."), (500, "Internal error."),
                       (502, "Invalid upstream response or credentials."), (503, "Unavailable or unconfigured."), (504, "Upstream timeout.")]}},
            "x-apple-path": endpoint["applePath"], "x-implementation-status": "implemented",
        }}
    paths["/v1/connections"] = {"post": {
        "operationId": "createHostedConnection",
        "summary": "Create a hosted connection",
        "description": "Accept at most 8 KiB of JSON, validate an App Store Connect team key, probe read access, and return a read-only facade key. The submitted private key is stored encrypted; provide it again only when creating a new connection.",
        "security": [],
        "requestBody": {"required": True, "description": "The complete JSON body must not exceed 8 KiB.", "content": {"application/json": {
            "schema": {"$ref": "#/components/schemas/HostedConnectionRequest"},
        }}},
        "responses": {
            "201": {"description": "Hosted connection created.", "headers": {
                "Location": {"required": True, "description": "The connection base URL.", "schema": {"type": "string", "format": "uri"}},
            }, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/HostedConnection"}}}},
            **{str(status): {"description": description, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}}
               for status, description in [(400, "Invalid connection request or credentials."), (408, "Connection request body timed out."),
                   (413, "Connection request body exceeded 8 KiB."), (415, "Content-Type must be application/json."),
                   (422, "Apple rejected the supplied credentials."), (429, "Connection setup rate limit exceeded."),
                   (502, "Invalid upstream response or credentials."), (503, "Hosted connection storage is unavailable or unconfigured."),
                   (504, "Apple request timed out.")]},
        },
        "x-implementation-status": "implemented",
    }}
    paths["/v1/connections/{connectionId}"] = {"delete": {
        "operationId": "deleteHostedConnection",
        "summary": "Delete a hosted connection",
        "description": "Remove the stored Apple credentials and revoke the facade key. Authenticate with the connection's facade API key.",
        "parameters": [{"name": "connectionId", "in": "path", "required": True,
            "schema": {"type": "string", "pattern": "^conn_live_[a-f0-9]{32}$"}}],
        "responses": {
            "204": {"description": "Connection credentials removed and facade key revoked."},
            "401": {"description": "Facade authentication failed.", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}},
            "503": {"description": "Hosted connection storage is unavailable or unconfigured.", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}},
        },
        "x-implementation-status": "implemented",
    }}
    manifest = {"appleSpecification": SOURCE["specification"], "routes": routes}
    openapi = {
        "openapi": "3.0.3", "info": {"title": "Parsequerade", "version": "0.1.0", "description": "Implemented read-only facade routes and hosted connection setup and revocation. Hosted connections require operator-configured encrypted storage; operator-managed connections remain supported."},
        "servers": [{"url": "/"}], "security": [{"facadeKey": []}], "paths": paths,
        "components": {"securitySchemes": {"facadeKey": {"type": "http", "scheme": "bearer"}}, "schemas": {
            "HostedConnectionRequest": {"type": "object", "additionalProperties": False,
                "required": ["issuerId", "keyId", "privateKey"], "properties": {
                    "issuerId": {"type": "string", "format": "uuid"},
                    "keyId": {"type": "string", "pattern": "^[A-Z0-9]{10}$"},
                    "privateKey": {"type": "string", "maxLength": 4096, "description": "P-256 PKCS#8 PEM contents."},
                }},
            "HostedConnection": {"type": "object", "additionalProperties": False,
                "required": ["connectionId", "baseUrl", "apiKey"], "properties": {
                    "connectionId": {"type": "string", "pattern": "^conn_live_[a-f0-9]{32}$"},
                    "baseUrl": {"type": "string", "format": "uri"},
                    "apiKey": {"type": "string", "pattern": "^pq_[A-Za-z0-9_-]{43}$", "description": "One-time bearer key; store it securely."},
                }},
            "Error": {"type": "object", "required": ["error"], "properties": {"error": {
                "type": "object", "required": ["code", "message", "requestId", "retryable", "details"],
                "properties": {"code": {"type": "string"}, "message": {"type": "string"}, "requestId": {"type": "string"},
                    "retryable": {"type": "boolean"}, "details": {"type": "array", "items": {"type": "object", "properties": {
                        "field": {"type": "string"}, "code": {"type": "string"}, "message": {"type": "string"}}}}}}}}}},
    }
    return {ROOT / "api/read-contracts.json": manifest, ROOT / "dist/docs/openapi.json": openapi}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    for path, value in outputs().items():
        content = json.dumps(value, indent=2, ensure_ascii=False) + "\n"
        if args.check:
            if not path.exists() or path.read_text() != content:
                raise SystemExit(f"{path.relative_to(ROOT)} is out of date; run npm run build:docs")
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
    print("Validated 12 implemented read contracts and 14 OpenAPI paths." if args.check else "Exported 12 implemented read contracts and 14 OpenAPI paths.")


if __name__ == "__main__":
    main()
