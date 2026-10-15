import base64
import copy
import json
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import encode_dss_signature

from parsequerade import ApiError, Client
from parsequerade._client import page_data
from parsequerade._contracts import ROUTES
from parsequerade._pagination import apple_url
from parsequerade._transport import Budget, Response, b64, unb64

KEY = ec.generate_private_key(ec.SECP256R1())
PEM = KEY.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()).decode()
FIXTURES = json.loads((Path(__file__).resolve().parents[2] / 'swift/Tests/ParsequeradeTests/Fixtures/reads.json').read_text())


def envelope(data, next=None):
    return {"data": data, "links": {"next": next}}


def resource(kind="apps", identifier="123", **attrs):
    return {"id": identifier, "type": kind, "attributes": attrs}


def response(payload, status=200, **headers):
    return Response(status, {"content-type": "application/json", **headers}, json.dumps(payload).encode())


class Mock:
    def __init__(self, responses):
        self.responses = list(responses)
        self.calls = []
        self.client = Client("issuer", "key", PEM, transport=self.send)

    def send(self, method, url, headers, body, timeout):
        assert urlsplit(url).netloc == "api.appstoreconnect.apple.com"
        assert 0 < timeout <= 8
        token = headers["Authorization"].removeprefix("Bearer ")
        header, payload, sig = token.split(".")
        assert json.loads(unb64(header))["alg"] == "ES256"
        claims = json.loads(unb64(payload))
        assert claims["aud"] == "appstoreconnect-v1"
        assert claims["iss"] == "issuer"
        assert 0 < claims["exp"] - time.time() <= 600
        signature = unb64(sig)
        KEY.public_key().verify(encode_dss_signature(int.from_bytes(signature[:32], "big"), int.from_bytes(signature[32:], "big")), f"{header}.{payload}".encode(), ec.ECDSA(hashes.SHA256()))
        self.calls.append((method, url, headers, body))
        assert self.responses, f"Unexpected request {url}"
        result = self.responses.pop(0)
        if callable(result):
            result = result(self.calls[-1])
        if isinstance(result, Exception):
            raise result
        return result if isinstance(result, Response) else response(result)


class ClientTests(unittest.TestCase):
    def assert_error(self, code, operation):
        with self.assertRaises(ApiError) as caught:
            operation()
        self.assertEqual(caught.exception.code, code)
        return caught.exception

    def test_all_twelve_methods_use_shared_worker_fixtures(self):
        import re
        for fixture in FIXTURES:
            with self.subTest(handler=fixture["handler"]):
                mock = Mock(fixture["responses"])
                self.assertEqual(mock.calls, [])
                method = re.sub(r"(?<!^)(?=[A-Z])", "_", fixture["handler"]).lower()
                params = {"appId": "123", "appInfoId": "info_1", "versionId": "version_1", "locale": "en-US"}
                args = [params[p] for p in re.findall(r"\{(\w+)\}", ROUTES[fixture["handler"]]["route"])]
                value = getattr(mock.client, method)(*args)
                item = value["items"][0] if fixture["collection"] else value
                self.assertEqual(item["id"], fixture["item"]["id"])
                for excluded in ("attributes", "relationships", "notes", "demoAccountPassword"):
                    self.assertNotIn(excluded, item)
                self.assertEqual(len(mock.calls), len(fixture["responses"]))
                self.assertFalse(mock.responses)
                if fixture["handler"] == "ageRating":
                    self.assertIs(item["gambling"], False)
                    self.assertEqual(item["ageRatingOverrideV2"], "FUTURE_READ_VALUE")

    def test_queries_projection_null_false_relationships(self):
        item = resource(name="Notes", isOrEverWasMadeForKids=False, subscriptionStatusUrl=None)
        mock = Mock([envelope([item])])
        page = mock.client.apps({"bundleId": ["one", "two"], "fields": ["isOrEverWasMadeForKids", "subscriptionStatusUrl"], "limit": 2})
        self.assertEqual(page["items"][0], {"id": "123", "isOrEverWasMadeForKids": False, "subscriptionStatusUrl": None})
        query = parse_qs(urlsplit(mock.calls[0][1]).query)
        self.assertEqual(query["filter[bundleId]"], ["one,two"])
        self.assertEqual(query["limit"], ["2"])
        info = resource("appInfos", "info_1")
        info["relationships"] = {"primaryCategory": {"data": {"id": "GAMES", "type": "appCategories"}}}
        mock = Mock([envelope([info])])
        self.assertEqual(mock.client.app_infos("123")["items"][0]["primaryCategoryId"], "GAMES")

    def test_invalid_queries_and_paths_make_no_requests(self):
        mock = Mock([])
        for query in ({"limit": 0}, {"limit": True}, {"fields": "secret"}, {"unknown": "yes"}, [("limit", 1), ("limit", 2)], {"limit": "01"}, {"fields": []}):
            self.assert_error("invalid_query", lambda: mock.client.apps(query))
        for value in ("../123", "a/b", "a?b", "a#b", "", "é", "x\n"):
            self.assert_error("invalid_path", lambda: mock.client.app(value))
        self.assert_error("route_not_found", lambda: mock.client.get("https://evil.example/apps"))
        self.assertFalse(mock.calls)

    def test_cursors_bind_client_and_query_preserve_filters_and_reject_loops(self):
        first = envelope([resource()], "/v1/apps?cursor=page2")
        mock = Mock([first, envelope([resource(identifier="456")])])
        page = mock.client.apps({"bundleId": "notes"})
        cursor = page["nextCursor"]
        self.assertIsInstance(cursor, str)
        for client, query in ((Mock([]).client, {"bundleId": "notes", "cursor": cursor}), (mock.client, {"cursor": cursor}), (mock.client, {"bundleId": "notes", "cursor": cursor + "x"})):
            self.assert_error("invalid_cursor", lambda: client.apps(query))
        second = mock.client.apps({"bundleId": "notes", "cursor": cursor})
        self.assertIsNone(second["nextCursor"])
        self.assertEqual(parse_qs(urlsplit(mock.calls[-1][1]).query)["filter[bundleId]"], ["notes"])
        mock = Mock([first, first])
        page = mock.client.apps()
        self.assert_error("upstream_incomplete_result", lambda: mock.client.apps({"cursor": page["nextCursor"]}))

    def test_untrusted_continuations_never_dispatch(self):
        for link in ("https://evil.example/v1/apps?cursor=x", "/v1/other?cursor=x", "/v1/apps?cursor=x&cursor=y", "/v1/apps?limit=3&cursor=x", "/v1/apps?cursor=x#fragment", "/v1/apps"):
            with self.subTest(link=link):
                mock = Mock([envelope([], link)])
                self.assert_error("upstream_invalid_response", mock.client.apps)
                self.assertEqual(len(mock.calls), 1)

    def test_parent_ownership_and_locale_uniqueness(self):
        mock = Mock([envelope([])])
        self.assert_error("resource_not_found", lambda: mock.client.version("123", "foreign"))
        fixture = next(f for f in FIXTURES if f["handler"] == "versionLocalization")
        replies = copy.deepcopy(fixture["responses"])
        duplicate = copy.deepcopy(replies[1]["data"][0]); duplicate["id"] = "locale_2"
        replies[1]["data"].append(duplicate)
        mock = Mock(replies)
        self.assert_error("ambiguous_selection", lambda: mock.client.version_localization("123", "version_1", "en-US"))
        self.assertEqual(len(mock.calls), 2)
        replies = copy.deepcopy(fixture["responses"])
        replies[-1]["data"]["attributes"]["locale"] = "fr-FR"
        mock = Mock(replies)
        self.assert_error("upstream_invalid_response", lambda: mock.client.version_localization("123", "version_1", "en-US"))

    def test_reads_retry_refresh_and_sanitize_errors(self):
        with patch("parsequerade._transport.time.sleep"):
            mock = Mock([response({}, 401), response({}, 503), envelope([])])
            self.assertEqual(mock.client.apps()["items"], [])
        self.assertNotEqual(mock.calls[0][2]["Authorization"], mock.calls[1][2]["Authorization"])
        self.assertEqual(mock.calls[1][2]["Authorization"], mock.calls[2][2]["Authorization"])
        mock = Mock([response({"errors": [{"code": "BAD.PARAM", "detail": "secret", "source": {"parameter": "limit"}}]}, 422)])
        error = self.assert_error("validation_failed", mock.client.apps)
        self.assertEqual(error.status, 422)
        self.assertEqual(error.details[0]["field"], "limit")
        self.assertNotIn("secret", str(error.details))
        mock = Mock([response({}, 429, **{"retry-after": "10"})])
        error = self.assert_error("rate_limited", mock.client.apps)
        self.assertEqual(error.retry_after, 10)
        self.assertEqual(len(mock.calls), 1)

    def test_write_validation_snapshot_and_uncertain_outcomes(self):
        fixture = next(f for f in FIXTURES if f["handler"] == "ageRating")
        for attrs in ({}, {"unknown": True}, {"gambling": None}, {"gambling": 0}, {"kidsAgeBand": "bad"}, {"developerAgeRatingInfoUrl": "https://user:pass@example.com"}):
            mock = Mock([])
            self.assert_error("invalid_request", lambda: mock.client.update_age_rating("123", "info_1", attrs))
            self.assertFalse(mock.calls)
        attrs = {"gambling": False, "kidsAgeBand": None}
        def mutate(call):
            attrs["gambling"] = True
            return fixture["responses"][0]
        mock = Mock([mutate, fixture["responses"][1], fixture["responses"][1]])
        rating = mock.client.update_age_rating("123", "info_1", attrs)
        self.assertFalse(rating["gambling"])
        self.assertEqual(json.loads(mock.calls[-1][3])["data"]["attributes"], {"gambling": False, "kidsAgeBand": None})
        self.assertEqual(mock.calls[-1][0], "PATCH")
        for failed, code in ((OSError("secret"), "write_outcome_unknown"), (Response(200, {"content-type": "text/html"}, b"secret"), "write_outcome_unknown"), (response({}, 401), "upstream_authentication_failed"), (envelope(resource("ageRatingDeclarations", "wrong")), "write_outcome_unknown")):
            mock = Mock([*fixture["responses"], failed])
            error = self.assert_error(code, lambda: mock.client.update_age_rating("123", "info_1", {"gambling": False}))
            self.assertFalse(error.retryable)
            self.assertEqual(len(mock.calls), 3)

    def test_malformed_and_bounded_responses(self):
        for reply in (Response(200, {"content-type": "application/json"}, b"invalid"), response({}, 302), envelope([resource(), resource()]), envelope([resource(name=42)]), Response(200, {"content-type": "application/json"}, b"x" * (4 * 1024 * 1024 + 1))):
            self.assert_error("upstream_invalid_response", Mock([reply]).client.apps)
        mock = Mock([])
        for budget in (Budget(attempts=32), Budget(deadline=0)):
            self.assert_error("upstream_budget_exceeded", lambda: mock.client._http.apple(apple_url("/v1/apps", {}), ROUTES["apps"], budget))
        self.assertFalse(mock.calls)
        mock = Mock([envelope([])])
        self.assert_error("upstream_invalid_response", lambda: mock.client._http.apple(apple_url("/v1/apps", {}), ROUTES["apps"], Budget(bytes=16 * 1024 * 1024)))

    def test_four_request_concurrency_bound(self):
        entered = threading.Barrier(5)
        release = threading.Event()
        def send(*args):
            entered.wait(timeout=5)
            release.wait(timeout=5)
            return response(envelope([]))
        client = Client("issuer", "key", PEM, transport=send)
        errors = []
        def run():
            try: client.apps()
            except Exception as error: errors.append(error)
        threads = [threading.Thread(target=run) for _ in range(4)]
        for thread in threads: thread.start()
        entered.wait(timeout=5)
        error = self.assert_error("rate_limited", client.apps)
        self.assertEqual(error.retry_after, 1)
        release.set()
        for thread in threads: thread.join(timeout=5)
        self.assertFalse(errors)

    def test_resolvers_are_bounded(self):
        replies = [envelope([], f"/v1/apps/123/appInfos?cursor={n}") for n in range(20)]
        mock = Mock(replies)
        self.assert_error("upstream_incomplete_result", lambda: mock.client.app_info("123", "info_1"))
        self.assertEqual(len(mock.calls), 20)

    def test_credentials_are_validated_locally(self):
        for pem in ("invalid", KEY.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.TraditionalOpenSSL, serialization.NoEncryption()).decode(), ec.generate_private_key(ec.SECP384R1()).private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()).decode()):
            self.assert_error("invalid_credentials", lambda: Client("issuer", "key", pem))
        self.assertNotIn("PRIVATE KEY", repr(Mock([]).client))


if __name__ == "__main__":
    unittest.main()
