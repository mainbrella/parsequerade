"""Language-neutral contracts and explicit runtime support for SDK generation."""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"Duplicate SDK contract key: {key}")
        result[key] = value
    return result


def load_contract(path):
    return json.loads(path.read_text(), object_pairs_hook=unique_object)


CATALOG = load_contract(ROOT / "api/sdk-catalog.json")
RESOURCE_NAMES = CATALOG["resourceNames"]


def read_groups(sdk="worker"):
    """Select only contracts implemented by this runtime, never all known routes."""
    implementations = {sdk for group in CATALOG["readGroups"] for sdk in group["implementations"]}
    if sdk not in implementations:
        raise ValueError(f"Unknown SDK runtime: {sdk}")
    for group in CATALOG["readGroups"]:
        if sdk in group["implementations"]:
            yield load_contract(ROOT / "api" / group["source"])


def read_contracts(sdk="worker"):
    routes = [route for group in read_groups(sdk) for route in group["routes"]]
    write = load_contract(ROOT / "api" / CATALOG["ageRatingWrite"])
    return routes, write


def model_definitions(sdk="worker"):
    """Named schemas referenced by this runtime's read contracts."""
    return {name: schema for group in read_groups(sdk) for name, schema in group.get("models", {}).items()}


def operation_models(sdk="worker"):
    definitions = CATALOG["operationModels"]
    selected = set()

    def select(name):
        if name in selected or name not in definitions:
            return
        selected.add(name)
        refs(definitions[name])

    def refs(value):
        if isinstance(value, dict):
            if "$ref" in value:
                select(value["$ref"].rsplit("/", 1)[-1])
            for item in value.values():
                refs(item)
        elif isinstance(value, list):
            for item in value:
                refs(item)

    for op in CATALOG["operations"].values():
        if sdk in op["implementations"]:
            for key in ("request", "checkpoint", "result"):
                select(op.get(key))
    return {name: schema for name, schema in definitions.items() if name in selected}


def resolve_schema(value, models):
    """Resolve shared refs while retaining sibling constraints such as nullable."""
    if isinstance(value, list):
        return [resolve_schema(item, models) for item in value]
    if not isinstance(value, dict):
        return value
    if "$ref" in value:
        target = models[value["$ref"].rsplit("/", 1)[-1]]
        value = {**target, **{key: item for key, item in value.items() if key != "$ref"}}
    return {key: resolve_schema(item, models) for key, item in value.items()}


def resource_models(routes):
    seen = set()
    for route in routes:
        if route["type"] in seen:
            continue
        seen.add(route["type"])
        schema = route["responseSchema"]
        if route["collection"]:
            schema = schema["properties"]["items"]["items"]
        yield RESOURCE_NAMES[route["type"]], schema


def runtime_contracts(sdk="worker"):
    routes, write = read_contracts(sdk)
    models = model_definitions(sdk)
    contracts = {
        "routes": [
            {key: resolve_schema(r[key], models) for key in ("handler", "route", "type", "collection", "attributes", "relationships", "parameters")}
            for r in routes
        ],
        "ageRatingWrite": write["attributes"],
    }
    if sdk in CATALOG["operations"]["updateMetadata"]["implementations"]:
        contracts["metadataUpdates"] = CATALOG["metadataUpdates"]
    return contracts


def validate_catalog():
    """Fail generation on ambiguous IDs, unavailable contracts, or broken refs."""
    group_names = set()
    handlers = set()
    paths = set()
    known_models = set(CATALOG["resourceNames"].values()) | set(CATALOG["operationModels"])
    implementations = {sdk for group in CATALOG["readGroups"] for sdk in group["implementations"]}
    if len(set(RESOURCE_NAMES.values())) != len(RESOURCE_NAMES):
        raise ValueError("Duplicate SDK resource model name")
    for group in CATALOG["readGroups"]:
        name = group["name"]
        if name in group_names:
            raise ValueError(f"Duplicate SDK read group: {name}")
        group_names.add(name)
        if not group["implementations"] or len(set(group["implementations"])) != len(group["implementations"]):
            raise ValueError(f"Invalid SDK implementation support: {name}")
        manifest = load_contract(ROOT / "api" / group["source"])
        for route in manifest["routes"]:
            if route["handler"] in handlers or route["route"] in paths:
                raise ValueError(f"Duplicate SDK read handler or route: {route['handler']}")
            handlers.add(route["handler"])
            paths.add(route["route"])
            if route["type"] not in RESOURCE_NAMES:
                raise ValueError(f"Unnamed SDK resource: {route['type']}")
        for model in manifest.get("models", {}):
            if model in known_models:
                raise ValueError(f"Duplicate SDK model name: {model}")
            known_models.add(model)
    for name, operation in CATALOG["operations"].items():
        if not operation["implementations"] or len(set(operation["implementations"])) != len(operation["implementations"]):
            raise ValueError(f"Invalid SDK operation support: {name}")
        if set(operation["implementations"]) - implementations:
            raise ValueError(f"Unknown SDK operation implementation: {name}")
        for key in ("request", "checkpoint", "result"):
            if key in operation and operation[key] not in known_models:
                raise ValueError(f"Unknown SDK operation model: {name}.{key}")
    for sdk in implementations:
        models = {**model_definitions(sdk), **operation_models(sdk)}
        routes, _ = read_contracts(sdk)
        try:
            resolve_schema({"routes": routes, "models": models}, models)
        except KeyError as error:
            raise ValueError(f"Unknown model reference in {sdk}: {error}") from error


validate_catalog()
