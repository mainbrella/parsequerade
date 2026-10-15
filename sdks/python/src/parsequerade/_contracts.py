"""Validation and flattening driven by bundled, reviewed facade contracts."""
import json
import re
from importlib.resources import files
from urllib.parse import urlsplit

from ._errors import ApiError, invalid_upstream, invalid_query, not_found

CONTRACTS = json.loads(files(__package__).joinpath("contracts.json").read_text())
ROUTES = {r["handler"]: r for r in CONTRACTS["routes"]}
ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]{0,127}\Z")
LOCALE = re.compile(r"[A-Za-z0-9][A-Za-z0-9-]{1,34}\Z")


def validate_segment(value, locale=False):
    if not isinstance(value, str) or not (LOCALE if locale else ID).fullmatch(value):
        raise ApiError(400, "invalid_path", "The resource path is invalid.")


def match_route(path):
    if not isinstance(path, str):
        raise not_found("route_not_found")
    for route in ROUTES.values():
        pattern = re.sub(r"\{(\w+)\}", r"(?P<\1>[^/]+)", route["route"])
        match = re.fullmatch(pattern, path)
        if match:
            params = match.groupdict()
            for key, value in params.items():
                validate_segment(value, key == "locale")
            return route, params
    raise not_found("route_not_found")


def parse_query(route, query):
    parameters = {p["name"]: p for p in route["parameters"]}
    pairs = query.items() if hasattr(query, "items") else (query or [])
    values, upstream = {}, {}
    for name, raw in pairs:
        if name not in parameters or name in values:
            raise invalid_query(name)
        schema = parameters[name]["schema"]
        if isinstance(raw, (list, tuple)) and all(isinstance(v, str) for v in raw):
            raw = ",".join(raw)
        elif type(raw) is bool:
            raw = str(raw).lower()
        elif type(raw) is int:
            raw = str(raw)
        if not isinstance(raw, str) or not raw or len(raw) > (8192 if name == "cursor" else 4096):
            raise invalid_query(name)
        kind = schema["type"]
        if kind == "integer" and (not re.fullmatch(r"[1-9][0-9]{0,2}", raw) or int(raw) > schema.get("maximum", 200)):
            raise invalid_query(name)
        if kind == "boolean" and raw not in ("true", "false"):
            raise invalid_query(name)
        if kind == "array":
            parts = raw.split(",")
            choices = schema["items"].get("enum")
            if len(parts) > 200 or any(not p or len(p) > 256 or (choices and p not in choices) for p in parts):
                raise invalid_query(name)
        values[name] = raw
        if parameters[name]["appleName"]:
            upstream[parameters[name]["appleName"]] = raw
    for p in parameters.values():
        if p["name"] not in values and "default" in p:
            upstream[p["appleName"]] = str(p["default"])
    return upstream, values.get("fields", "").split(",") if "fields" in values else None, values.get("cursor"), int(values.get("limit", 50))


def resource(value, kind):
    if (not isinstance(value, dict) or value.get("type") != kind or not isinstance(value.get("id"), str)
            or not ID.fullmatch(value["id"]) or any(k in value and not isinstance(value[k], dict) for k in ("attributes", "relationships"))):
        raise invalid_upstream()
    return value


def check_parent(value, relation, identifier, kind):
    rel = value.get("relationships", {}).get(relation, {})
    if not isinstance(rel, dict):
        raise invalid_upstream()
    if "data" in rel:
        data = rel["data"]
        if not isinstance(data, dict) or data.get("id") != identifier or data.get("type") != kind:
            raise invalid_upstream()


def flatten(value, route, context, fields=None):
    resource(value, route["type"])
    result = {**context, "id": value["id"]}
    for name, schema in route["attributes"].items():
        if fields is not None and name not in fields:
            continue
        if name in value.get("attributes", {}):
            item = value["attributes"][name]
            if item is not None and type(item) is not {"string": str, "boolean": bool}[schema["type"]]:
                raise invalid_upstream()
            result[name] = item
    for name, relation in route["relationships"].items():
        rel = value.get("relationships", {}).get(name, {})
        if not isinstance(rel, dict):
            raise invalid_upstream()
        if "data" in rel:
            data = rel["data"]
            if data is not None:
                resource(data, relation["type"])
            result[relation["field"]] = data["id"] if data else None
    return result


def validate_update(value):
    if not isinstance(value, dict) or not value:
        raise ApiError(400, "invalid_request", "Provide a nonempty object of supported age rating attributes.")
    try:
        encoded = json.dumps(value, separators=(",", ":"), ensure_ascii=False, allow_nan=False).encode()
    except (TypeError, ValueError, UnicodeError):
        raise ApiError(400, "invalid_request", "Provide valid JSON attributes.") from None
    if len(encoded) > 8192:
        raise ApiError(413, "request_too_large", "Age rating attributes must fit within 8 KiB.")
    for name, item in value.items():
        schema = CONTRACTS["ageRatingWrite"].get(name)
        valid = schema is not None
        if valid and item is None:
            valid = schema.get("nullable", False)
        elif valid:
            valid = type(item) is {"boolean": bool, "string": str}[schema["type"]]
            if valid and isinstance(item, str):
                valid = len(item.encode("utf-16-le")) // 2 <= 2048 and ("enum" not in schema or item in schema["enum"])
                if valid and schema.get("format") == "uri":
                    try:
                        url = urlsplit(item)
                        valid = url.scheme in ("http", "https") and bool(url.hostname) and url.username is None and url.password is None
                    except ValueError:
                        valid = False
        if not valid:
            raise ApiError(400, "invalid_request", "Unsupported age rating attribute or value.", details=[{
                "code": "invalid_attribute", "message": "This age rating attribute or value is unsupported.", "field": name,
            }])
    return json.loads(encoded)
