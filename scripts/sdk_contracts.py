"""Shared inputs and model selection for SDK generation."""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RESOURCE_NAMES = {
    "apps": "App", "appInfos": "AppInfo", "appInfoLocalizations": "AppInfoLocalization",
    "ageRatingDeclarations": "AgeRating", "appStoreVersions": "Version",
    "appStoreVersionLocalizations": "VersionLocalization", "appStoreReviewDetails": "ReviewContact",
}


def read_contracts():
    reads = json.loads((ROOT / "api/read-contracts.json").read_text())
    write = json.loads((ROOT / "api/age-rating-write-contract.json").read_text())
    return reads["routes"], write


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
