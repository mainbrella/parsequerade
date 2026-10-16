import base64
import copy
import gc
import json
from pathlib import Path
import threading
import time
import unittest
import weakref
from urllib.parse import parse_qs, urlsplit

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import encode_dss_signature

from parsequerade import ApiError, Client, Response

KEY = ec.generate_private_key(ec.SECP256R1())
PEM = KEY.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()).decode()
ROOT = Path(__file__).resolve().parents[3]
SCENARIOS = json.loads((ROOT / "tests/sdk/scenarios.json").read_text())
FIXTURES = json.loads((Path(__file__).resolve().parents[2] / 'swift/Tests/ParsequeradeTests/Fixtures/reads.json').read_text())


def unb64(value):
    return base64.urlsafe_b64decode(value + "=" * (-len(value) % 4))


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
        headers = {name.lower(): value for name, value in headers.items()}
        token = headers["authorization"].removeprefix("Bearer ")
        header, payload, sig = token.split(".")
        assert json.loads(unb64(header))["alg"] == "ES256"
        claims = json.loads(unb64(payload))
        assert claims["aud"] == "appstoreconnect-v1"
        assert claims["iss"] == "issuer"
        assert 0 < claims["exp"] - time.time() <= 1200
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
                args = [part for part in fixture["path"].split("/") if part in ("123", "info_1", "version_1", "en-US")]
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

    def assert_subset(self, actual, expected):
        if isinstance(expected, dict):
            self.assertIsInstance(actual, dict)
            for key, value in expected.items():
                self.assertIn(key, actual)
                self.assert_subset(actual[key], value)
        elif isinstance(expected, list):
            self.assertIsInstance(actual, list)
            self.assertEqual(len(actual), len(expected))
            for item, value in zip(actual, expected):
                self.assert_subset(item, value)
        else:
            self.assertEqual(actual, expected)

    def test_rust_and_python_share_operation_scenarios(self):
        for scenario in SCENARIOS:
            with self.subTest(scenario=scenario["name"]):
                def reply(step):
                    def send(call):
                        method, url, _, body = call
                        self.assertEqual(method, step["method"])
                        self.assertEqual(urlsplit(url).path, step["path"])
                        self.assertEqual(parse_qs(urlsplit(url).query), {
                            key: [value] for key, value in step.get("query", {}).items()
                        })
                        self.assertEqual(json.loads(body) if body else None, step.get("body"))
                        return response(step["response"], step.get("status", 200))
                    return send
                mock = Mock([reply(step) for step in scenario["steps"]])
                method = getattr(mock.client, scenario.get("pythonMethod", scenario["operation"]))
                args = scenario.get("pythonArgs", scenario["args"])
                if "error" in scenario:
                    error = self.assert_error(scenario["error"]["code"], lambda: method(*args))
                    for name, value in scenario["error"].items():
                        self.assertEqual(getattr(error, name), value)
                else:
                    self.assert_subset(method(*args), scenario["expect"])
                self.assertFalse(mock.responses)
                self.assertEqual(len(mock.calls), len(scenario["steps"]))

    def test_new_write_and_media_inputs_validate_before_dispatch(self):
        mock = Mock([])
        for attributes in ({}, {"name": "Not writable"}, {"contentRightsDeclaration": False}):
            self.assert_error("invalid_update", lambda: mock.client.update_metadata("/apps/app1", attributes))
        for app, platform, number in (("../app1", "IOS", "1.0"), ("app1", "ANDROID", "1.0"),
                                      ("app1", "IOS", "1..2"), ("app1", "IOS", "1.0-beta")):
            with self.assertRaises(ApiError):
                mock.client.create_version(app, platform, number)
        self.assert_error("invalid_path", lambda: mock.client.attach_build("../app", "version1", "build1"))
        self.assert_error("invalid_path", lambda: mock.client.preview_sets("app1", "version1", "en_US"))
        self.assert_error("invalid_query", lambda: mock.client.builds("app1", {"platform": "ANDROID"}))
        self.assert_error("invalid_query", lambda: mock.client.library_image("image1", {"fields": "secret"}))
        self.assertFalse(mock.calls)

    def test_unrepresentable_write_values_use_public_api_errors(self):
        mock = Mock([])
        self.assert_error("invalid_request", lambda: mock.client.update_metadata(
            "/apps/app1", {"contentRightsDeclaration": 10 ** 500}))
        for attributes in ({"contentRightsDeclaration": float("nan")},
                           {"contentRightsDeclaration": object()}):
            self.assert_error("invalid_request", lambda: mock.client.update_metadata("/apps/app1", attributes))
        self.assertFalse(mock.calls)

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

    def test_custom_transport_reference_cycles_are_collected(self):
        mock = Mock([])
        reference = weakref.ref(mock)
        del mock
        gc.collect()
        self.assertIsNone(reference())

    def test_callback_reentry_fails_cleanly(self):
        observed = []
        def send(*_):
            with self.assertRaises(RuntimeError) as caught:
                client.apps()
            observed.append(str(caught.exception))
            return response(envelope([]))
        client = Client("issuer", "key", PEM, transport=send)
        self.assertEqual(client.apps()["items"], [])
        self.assertEqual(len(observed), 1)

    def test_exported_types_describe_nested_arrays_and_checkpoint_optionality(self):
        from typing import get_type_hints
        from parsequerade import HttpHeader, ListingImageUpload, ListingImageUploadCheckpoint, UploadOperation
        self.assertEqual(get_type_hints(UploadOperation)["requestHeaders"], list[HttpHeader] | None)
        self.assertEqual(ListingImageUpload.__required_keys__, {
            "appId", "versionId", "locale", "placementType", "fileName",
        })
        self.assertEqual(ListingImageUploadCheckpoint.__required_keys__, frozenset())

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
        mock = Mock([response({}, 401), response({}, 503), envelope([])])
        self.assertEqual(mock.client.apps()["items"], [])
        self.assertNotEqual(mock.calls[0][2]["authorization"], mock.calls[1][2]["authorization"])
        self.assertEqual(mock.calls[1][2]["authorization"], mock.calls[2][2]["authorization"])
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

    def test_total_response_budget_is_shared_across_parent_lookup_pages(self):
        replies = []
        for number in range(6):
            payload = envelope([], f"?cursor={number}")
            payload["meta"] = {"padding": "x" * (3 * 1024 * 1024)}
            replies.append(response(payload))
        mock = Mock(replies)
        self.assert_error("upstream_invalid_response", lambda: mock.client.app_info("123", "info_1"))
        self.assertEqual(len(mock.calls), 6)

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
        try:
            entered.wait(timeout=5)
            error = self.assert_error("rate_limited", client.apps)
            self.assertEqual(error.retry_after, 1)
        finally:
            release.set()
            for thread in threads: thread.join(timeout=5)
        self.assertFalse(any(thread.is_alive() for thread in threads))
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
