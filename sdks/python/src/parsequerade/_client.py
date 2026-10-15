"""Native, synchronous facade client. No hosted service is involved."""
import time
from urllib.parse import parse_qs, urlsplit

from ._contracts import ROUTES, check_parent, flatten, match_route, parse_query, resource, validate_segment, validate_update
from ._errors import ApiError, incomplete, invalid_upstream, not_found, unknown_write
from ._generated import AgeRating, AgeRatingUpdate, Query, ResourceClient
from ._pagination import Cursors, apple_url, digest, next_page
from ._transport import Auth, Budget, Transport


def page_data(payload, kind, limit):
    items = payload.get("data")
    if not isinstance(items, list) or len(items) > limit or not isinstance(payload.get("links"), dict):
        raise invalid_upstream()
    seen = set()
    for item in items:
        resource(item, kind)
        if item["id"] in seen:
            raise invalid_upstream()
        seen.add(item["id"])
    return items


class Client(ResourceClient):
    def __init__(self, issuer_id: str, key_id: str, private_key: str, *, transport=None):
        """Validate a P-256 PKCS#8 PEM locally. Custom transport is trusted with signed requests."""
        self._http = Transport(Auth(issuer_id, key_id, private_key), transport)
        self._cursors = Cursors()

    def get(self, path: str, query: Query | None = None) -> dict:
        """Read an implemented facade path and preserve exact flat JSON, including nulls."""
        return self._read(path, query, Budget())

    def _traverse(self, path, query, route, budget, visit):
        initial = apple_url(path, query)
        url, seen = initial, set()
        for _ in range(20):
            if digest(url) in seen:
                raise incomplete()
            seen.add(digest(url))
            payload = self._http.apple(url, route, budget)
            if visit(page_data(payload, route["type"], 200)):
                return
            url = next_page(payload, initial)
            if url is None:
                return
        raise incomplete()

    def _read(self, path, query, budget):
        route, p = match_route(path)
        upstream, fields, cursor, limit = parse_query(route, query)
        handler = route["handler"]
        info = handler.startswith("appInfo") or handler == "ageRating"
        app_id = p.get("appId", "")
        parent_id = p.get("appInfoId" if info else "versionId")
        parent_type = "appInfos" if info else "appStoreVersions"
        context = {}
        if handler not in ("apps", "app"):
            context["appId"] = app_id
            if parent_id and handler not in ("appInfo", "version"):
                context["appInfoId" if info else "versionId"] = parent_id
        if parent_id:
            found = False
            def find_parent(items):
                nonlocal found
                for item in items:
                    check_parent(item, "app", app_id, "apps")
                found = any(item["id"] == parent_id for item in items)
                return found
            self._traverse(f"/v1/apps/{app_id}/{parent_type}", {"limit": "200", **({} if info else {"filter[id]": parent_id})},
                           ROUTES["appInfos" if info else "versions"], budget, find_parent)
            if not found:
                raise not_found()
        identifier = parent_id or ""
        paths = {
            "apps": "/v1/apps", "app": f"/v1/apps/{app_id}",
            "appInfos": f"/v1/apps/{app_id}/appInfos", "appInfo": f"/v1/appInfos/{identifier}",
            "versions": f"/v1/apps/{app_id}/appStoreVersions", "version": f"/v1/appStoreVersions/{identifier}",
            "appInfoLocalizations": f"/v1/appInfos/{identifier}/appInfoLocalizations",
            "versionLocalizations": f"/v1/appStoreVersions/{identifier}/appStoreVersionLocalizations",
            "ageRating": f"/v1/appInfos/{identifier}/ageRatingDeclaration",
            "reviewContact": f"/v1/appStoreVersions/{identifier}/appStoreReviewDetail",
        }
        selected = None
        if "locale" in p:
            localizations = "appInfoLocalizations" if info else "appStoreVersionLocalizations"
            def select_locale(items):
                nonlocal selected
                for item in items:
                    check_parent(item, "appInfo" if info else "appStoreVersion", identifier, parent_type)
                    locale = item.get("attributes", {}).get("locale")
                    if not isinstance(locale, str):
                        raise invalid_upstream()
                    if locale == p["locale"]:
                        if selected is not None:
                            raise ApiError(409, "ambiguous_selection", "More than one localization matches this locale.")
                        selected = item["id"]
                return False
            self._traverse(f"/v1/{parent_type}/{identifier}/{localizations}", {
                "limit": "200", "filter[locale]": p["locale"], f"fields[{localizations}]": "locale",
            }, ROUTES["appInfoLocalizations" if info else "versionLocalizations"], budget, select_locale)
            if selected is None:
                raise not_found("localization_not_found")
            paths[handler] = f"/v1/{localizations}/{selected}"
            if fields is not None and "locale" not in fields:
                upstream[f"fields[{localizations}]"] = ",".join([*fields, "locale"])
        if handler in ("appInfos", "appInfo"):
            upstream["include"] = "primaryCategory,secondaryCategory"
            if fields is not None:
                upstream["fields[appInfos]"] = ",".join([*fields, "primaryCategory", "secondaryCategory"])
        initial = apple_url(paths[handler], upstream)
        if not route["collection"]:
            payload = self._http.apple(initial, route, budget)
            data = payload["data"]
            if data is None:
                raise not_found("review_contact_not_found" if handler == "reviewContact" else "resource_not_found")
            resource(data, route["type"])
            expected = app_id if handler == "app" else parent_id if handler in ("appInfo", "version") else selected
            if expected and data["id"] != expected:
                raise invalid_upstream()
            if handler in ("appInfo", "version"):
                check_parent(data, "app", app_id, "apps")
            if "locale" in p:
                if data.get("attributes", {}).get("locale") != p["locale"]:
                    raise invalid_upstream()
                check_parent(data, "appInfo" if info else "appStoreVersion", identifier, parent_type)
            if handler == "reviewContact":
                check_parent(data, "appStoreVersion", identifier, parent_type)
            return flatten(data, route, context, fields)
        binding = digest(path + "\n" + initial)
        prior = self._cursors.decode(cursor, binding) if cursor else None
        expires = prior["expires"] if prior else int(time.time()) + 900
        seen = list(prior["seen"]) if prior else []
        url = apple_url(paths[handler], {**upstream, "cursor": prior["next"]}) if prior else initial
        if len(seen) >= 100 or digest(url) in seen:
            raise incomplete()
        seen.append(digest(url))
        payload = self._http.apple(url, route, budget)
        items = []
        for item in page_data(payload, route["type"], limit):
            if handler in ("appInfos", "versions"):
                check_parent(item, "app", app_id, "apps")
            if handler in ("appInfoLocalizations", "versionLocalizations"):
                check_parent(item, "appInfo" if info else "appStoreVersion", identifier, parent_type)
            items.append(flatten(item, route, context, fields))
        next_url = next_page(payload, initial)
        next_cursor = None
        if next_url:
            if digest(next_url) in seen:
                raise incomplete()
            next_cursor = self._cursors.encode({"binding": binding, "next": parse_qs(urlsplit(next_url).query)["cursor"][0], "seen": seen, "expires": expires})
        return {"items": items, "nextCursor": next_cursor}

    def update_age_rating(self, app_id: str, app_info_id: str, attributes: AgeRatingUpdate) -> AgeRating:
        """Validate and snapshot attributes, resolve ownership, then send one PATCH. Never retries writes."""
        validate_segment(app_id)
        validate_segment(app_info_id)
        attributes = validate_update(attributes)
        budget = Budget()
        current = self._read(f"/apps/{app_id}/app-infos/{app_info_id}/age-rating", None, budget)
        identifier = current["id"]
        route = ROUTES["ageRating"]
        payload = self._http.apple(apple_url(f"/v1/ageRatingDeclarations/{identifier}", {}), route, budget,
                                   {"data": {"type": "ageRatingDeclarations", "id": identifier, "attributes": attributes}})
        try:
            if not isinstance(payload["data"], dict) or payload["data"].get("id") != identifier:
                raise invalid_upstream()
            return flatten(payload["data"], route, {"appId": app_id, "appInfoId": app_info_id})
        except ApiError:
            raise unknown_write() from None
