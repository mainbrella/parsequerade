#!/usr/bin/env python3
"""Snapshot GET operations reached through Apple's App Store documentation tree.

Only topic links are followed: see-also links would escape into unrelated APIs.
The normalized snapshot contains API facts, not Apple's rendered documentation.
Run explicitly when updating sources; building the site never needs the network.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import io
import json
from pathlib import Path
import time
from urllib.request import Request, urlopen
import zipfile

ROOT = Path(__file__).resolve().parents[1]
BASE = "https://developer.apple.com"
DOC_ROOT = "/documentation/appstoreconnectapi/"
SPEC_URL = BASE + "/sample-code/app-store-connect/app-store-connect-openapi-specification.zip"


def download(url):
    for attempt in range(4):
        try:
            with urlopen(Request(url, headers={"User-Agent": "Parsequerade documentation source review"}), timeout=60) as response:
                return response.read()
        except Exception:
            if attempt == 3:
                raise
            time.sleep(attempt + 1)


def run(cache, refresh):
    cache.mkdir(parents=True, exist_ok=True)

    def read_doc(slug):
        path = cache / (slug + ".json")
        if refresh or not path.exists():
            payload = download(BASE + "/tutorials/data" + DOC_ROOT + slug + ".json")
            path.write_bytes(payload)
        else:
            payload = path.read_bytes()
        return slug, json.loads(payload), hashlib.sha256(payload).hexdigest()

    zip_path = cache / "openapi.zip"
    if refresh or not zip_path.exists():
        zip_path.write_bytes(download(SPEC_URL))
    spec_payload = zip_path.read_bytes()
    with zipfile.ZipFile(io.BytesIO(spec_payload)) as archive:
        name = next(name for name in archive.namelist() if name.endswith(".json") and "__MACOSX" not in name)
        spec = json.loads(archive.read(name))
    schemas = spec["components"]["schemas"]

    domains = []
    topics = {}
    endpoint_topics = {}
    category_sources = {}
    endpoint_sources = {}
    pending = {"app-store": (None, 0)}
    seen = set()
    with ThreadPoolExecutor(max_workers=8) as pool:
        while pending:
            batch = pending
            pending = {}
            for slug, document, digest in pool.map(read_doc, batch):
                seen.add(slug)
                domain, depth = batch[slug]
                refs = document.get("references", {})
                title = document.get("metadata", {}).get("title", slug)
                is_endpoint = slug.startswith("get-")
                if is_endpoint:
                    endpoint_sources[slug] = (document, digest)
                    continue
                category_sources[slug] = {"title": title, "sha256": digest, "url": BASE + DOC_ROOT + slug}
                if slug != "app-store":
                    topics[slug] = {"slug": slug, "title": title, "domain": domain, "depth": depth, "source": BASE + DOC_ROOT + slug, "endpoints": []}
                for section in document.get("topicSections", []):
                    section_domain = domain
                    if slug == "app-store":
                        section_domain = "domain-" + str(len(domains) + 1)
                        domains.append({"id": section_domain, "title": section["title"]})
                    for identifier in section.get("identifiers", []):
                        ref = refs.get(identifier, {})
                        url = ref.get("url", "")
                        if not url.startswith(DOC_ROOT):
                            continue
                        child = url.removeprefix(DOC_ROOT)
                        if child.startswith("get-"):
                            membership = {"topic": slug, "section": section["title"], "depth": depth}
                            memberships = endpoint_topics.setdefault(child, [])
                            if membership not in memberships:
                                memberships.append(membership)
                            if child not in seen:
                                pending.setdefault(child, (section_domain, depth + 1))
                        elif ref.get("kind") == "article" and child not in seen:
                            pending.setdefault(child, (section_domain, depth + 1))
            print(f"Read {len(category_sources)} topic pages and {len(endpoint_sources)} GET pages; {len(pending)} queued.", flush=True)

    endpoints = []
    for slug, (doc, digest) in sorted(endpoint_sources.items()):
        sections = doc.get("primaryContentSections", [])
        endpoint_section = next((s for s in sections if s.get("kind") == "restEndpoint"), None)
        if endpoint_section is None:
            raise ValueError(f"GET page has no endpoint: {slug}")
        path = "".join(t["text"] for t in endpoint_section["tokens"] if t["kind"] in ("path", "parameter"))
        path = "/" + path.lstrip("/")
        operation = spec["paths"].get(path, {}).get("get")
        if operation is None:
            raise ValueError(f"Published GET missing from the pinned OpenAPI specification: {path}")
        documented_params = {
            p["name"]: p for section in sections if section.get("kind") == "restParameters"
            for p in section.get("items", [])
        }
        params = []
        for param in operation.get("parameters", []):
            doc_param = documented_params.get(param["name"], {})
            item = {key: param[key] for key in ("name", "in", "required", "schema", "deprecated") if key in param}
            if doc_param.get("deprecated"):
                item["deprecated"] = True
            # Descriptions are deliberately rewritten by the facade renderer.
            params.append(item)
        response = operation["responses"].get("200", {})
        content = response.get("content", {})
        memberships = sorted(endpoint_topics[slug], key=lambda item: -item["depth"])
        topic = memberships[0]["topic"]
        topics[topic]["endpoints"].append(slug)
        endpoints.append({
            "slug": slug,
            "title": doc.get("metadata", {}).get("title", slug),
            "source": BASE + DOC_ROOT + slug,
            "sourceSha256": digest,
            "applePath": path,
            "operationId": operation["operationId"],
            "deprecated": operation.get("deprecated", False),
            "topic": topic,
            "memberships": [{"topic": m["topic"], "section": m["section"]} for m in memberships],
            "parameters": params,
            "response": content,
            "responseCodes": sorted(operation["responses"]),
        })

    # Response envelopes contain recursive includes across the API. Keep only
    # schemas needed to describe the actual resource, not every included type.
    retained = {}
    queue = []
    for endpoint in endpoints:
        for media in endpoint["response"].values():
            raw = media.get("schema", {})
            response_schema = schemas.get(raw.get("$ref", "").rsplit("/", 1)[-1], raw)
            resource_data = response_schema.get("properties", {}).get("data")
            if resource_data is not None:
                resource_data = resource_data.get("items", resource_data)
                name = resource_data.get("$ref", "").rsplit("/", 1)[-1]
                if name in schemas:
                    model = schemas[name]
                    attributes = model.get("properties", {}).get("attributes", {})
                    relationships = model.get("properties", {}).get("relationships", {})
                    retained[name] = {"type": "object", "title": model.get("title", name), "properties": {
                        "id": {"type": "string"}, "attributes": attributes, "relationships": relationships,
                    }, "required": model.get("required", [])}
                    queue.append(retained[name])
            # Preserve exact envelopes to distinguish metrics from resources.
            ref = raw.get("$ref", "").rsplit("/", 1)[-1]
            if ref in schemas:
                retained[ref] = {key: val for key, val in response_schema.items() if key != "properties"}
                retained[ref]["properties"] = {key: val for key, val in response_schema.get("properties", {}).items() if key not in ("included", "links", "meta")}
                queue.append(retained[ref])
        queue.extend(param.get("schema", {}) for param in endpoint["parameters"])

    def retain_refs(value):
        if isinstance(value, dict):
            ref = value.get("$ref", "").rsplit("/", 1)[-1]
            if ref in schemas and ref not in retained:
                retained[ref] = schemas[ref]
                retain_refs(schemas[ref])
            for key, child in value.items():
                if key != "$ref":
                    retain_refs(child)
        elif isinstance(value, list):
            for child in value:
                retain_refs(child)

    for value in queue:
        retain_refs(value)
    active_topics = [t for t in topics.values() if t["endpoints"]]
    active_topics.sort(key=lambda t: (int(t["domain"].split("-")[-1]), t["title"].lower()))
    result = {
        "source": BASE + DOC_ROOT + "app-store",
        "reviewedAt": datetime.now(timezone.utc).date().isoformat(),
        "specification": {"url": SPEC_URL, "version": spec["info"]["version"], "sha256": hashlib.sha256(spec_payload).hexdigest()},
        "counts": {"topicPagesRead": len(category_sources), "getPagesRead": len(endpoints), "topicsWithGets": len(active_topics)},
        "domains": [d for d in domains if any(t["domain"] == d["id"] for t in active_topics)],
        "topics": active_topics,
        "sources": category_sources,
        "endpoints": endpoints,
        "schemas": dict(sorted(retained.items())),
    }
    output = ROOT / "docs" / "apple-get-reference.json"
    output.parent.mkdir(exist_ok=True)
    output.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    print(f"Saved {len(endpoints)} GET endpoints in {len(active_topics)} topics, OpenAPI {spec['info']['version']}, to {output.relative_to(ROOT)}.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, default=Path("/tmp/parsequerade-apple-docs"))
    parser.add_argument("--refresh", action="store_true", help="Fetch current Apple docs and specification, replacing the cached source files.")
    args = parser.parse_args()
    run(args.cache, args.refresh)
