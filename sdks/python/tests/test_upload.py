"""Exercise upload recovery through the Python API and the shared Rust runtime."""
import hashlib
import json
import unittest
from urllib.parse import parse_qs, urlsplit

from parsequerade import ApiError
from test_client import Mock, ROOT, envelope, resource, response

PNG = (ROOT / "sdks/rust/src/tests/fixtures/header.png").read_bytes()
REQUEST = {"appId": "app1", "versionId": "version1", "locale": "en-US",
           "placementType": "PRODUCT_PAGE_HEADER_ASSET", "fileName": "listing.png"}
BINDING = "app1:version1:en-US:PRODUCT_PAGE_HEADER_ASSET:listing.png:" + hashlib.sha256(PNG).hexdigest()
BLOB_ORIGIN = "https://store-030.blobstore.apple.com"
RECEIPT = {"appId": "app1", "versionId": "version1", "locale": "en-US",
           "placementType": "PRODUCT_PAGE_HEADER_ASSET", "imageId": "image1", "placementId": "placement1"}


def step(path, payload, *, query=None, method="GET", body=None, status=200, headers=None, error=None):
    return {"path": path, "payload": payload, "query": query or {}, "method": method,
            "body": body, "status": status, "headers": headers or {}, "error": error}


def scope():
    return [
        step("/v1/apps/app1/appStoreVersions", envelope([resource("appStoreVersions", "version1")]),
             query={"limit": "200", "filter[id]": "version1"}),
        step("/v1/appStoreVersions/version1/appStoreVersionLocalizations",
             envelope([resource("appStoreVersionLocalizations", "loc1", locale="en-US")]),
             query={"limit": "200", "filter[locale]": "en-US", "fields[appStoreVersionLocalizations]": "locale"}),
        step("/v1/appStoreVersionLocalizations/loc1",
             envelope(resource("appStoreVersionLocalizations", "loc1", locale="en-US")),
             query={"fields[appStoreVersionLocalizations]": "locale"}),
    ]


def placement(image_id="image1"):
    item = resource("appAssetLibraryPlacements", "placement1", mediaType="IMAGE",
                    placementType="PRODUCT_PAGE_HEADER_ASSET", placementGroup="CREATIVE_IOS", state="ASSET_PROCESSING")
    item["relationships"] = {
        "appStoreVersionLocalization": {"data": {"type": "appStoreVersionLocalizations", "id": "loc1"}},
        "image": {"data": {"type": "appAssetLibraryImages", "id": image_id}},
    }
    return item


def placements(*items):
    return scope()[:2] + [
        step("/v1/appStoreVersionLocalizations/loc1/placements", envelope(list(items)),
             query={"limit": "200", "filter[placementType]": "PRODUCT_PAGE_HEADER_ASSET", "include": "image,video"})
    ]


def catalog():
    return envelope([resource("appAssetLibraryRefData", "reference1",
        features=[{"featureId": "APP_STORE_VERSIONS", "placementPolicies": [
            {"placementType": "PRODUCT_PAGE_HEADER_ASSET", "groupLimits": [{"groupIds": ["CREATIVE_IOS"], "maxCount": 1}]}
        ]}],
        placementProfileGroups=[{"placementProfileGroupId": "CREATIVE_IOS", "platform": "IOS"}],
        placementTypes=[{"placementTypeId": "PRODUCT_PAGE_HEADER_ASSET", "acceptsAssetCategories": ["CREATIVE_ASSETS"],
                         "specMappings": [{"placementGroupId": "CREATIVE_IOS", "specs": ["header-spec"]}]}],
        imageSpecs=[{"specId": "header-spec", "dimensions": {"minWidth": 3840, "maxWidth": 3840, "minHeight": 1646, "maxHeight": 1646},
                     "compatiblePlacementTypes": ["PRODUCT_PAGE_HEADER_ASSET"], "alphaAllowed": False,
                     "fileExtensions": [".png"], "mimeTypes": ["image/png"], "maxFileSize": 33554432}],
    )])


def preflight():
    return scope() + placements() + [
        step("/v1/apps/app1/assetLibrary", envelope(resource("appAssetLibraries", "library1"))),
        step("/v1/appAssetLibraryRefData", catalog()),
    ]


def upload_operation(path="/upload", offset=0, length=None):
    return {"method": "PUT", "url": BLOB_ORIGIN + path, "offset": offset,
            "length": len(PNG) if length is None else length,
            "requestHeaders": [{"name": "Content-Type", "value": "image/png"},
                               {"name": "X-Upload-Token", "value": "signed-fixture"}]}


def reservation(operations=None, error=None):
    return step("/v1/appAssetLibraryImages",
        envelope(resource("appAssetLibraryImages", "image1", state="AWAITING_UPLOAD",
                          uploadOperations=[upload_operation()] if operations is None else operations)),
        method="POST", error=error,
        body={"data": {"type": "appAssetLibraryImages", "attributes": {
            "fileName": "listing.png", "fileSize": len(PNG), "category": "CREATIVE_ASSETS",
            "referenceName": "parsequerade-" + hashlib.sha256(BINDING.encode()).hexdigest()},
            "relationships": {"assetLibrary": {"data": {"type": "appAssetLibraries", "id": "library1"}}}}})


def transfer(path="/upload", body=PNG, status=200):
    return step(path, {}, method="PUT", body=body, status=status,
                headers={"content-type": "image/png", "x-upload-token": "signed-fixture"})


def commit(error=None):
    return step("/v1/appAssetLibraryImages/image1",
                envelope(resource("appAssetLibraryImages", "image1", state="UPLOAD_COMPLETE")),
                method="PATCH", error=error,
                body={"data": {"type": "appAssetLibraryImages", "id": "image1", "attributes": {"uploaded": True}}})


def create_placement():
    return step("/v1/appAssetLibraryPlacements",
                envelope(resource("appAssetLibraryPlacements", "placement1", state="ASSET_PROCESSING")),
                method="POST", body={"data": {"type": "appAssetLibraryPlacements",
                    "attributes": {"placementType": "PRODUCT_PAGE_HEADER_ASSET", "placementGroup": "CREATIVE_IOS"},
                    "relationships": {"image": {"data": {"type": "appAssetLibraryImages", "id": "image1"}},
                                      "appStoreVersionLocalization": {"data": {"type": "appStoreVersionLocalizations", "id": "loc1"}}}}})


def image_read(state="UPLOAD_COMPLETE"):
    return step("/v1/appAssetLibraryImages/image1",
                envelope(resource("appAssetLibraryImages", "image1", state=state)))


def checkpoint(phase):
    result = {"binding": BINDING, "imageId": "image1", "phase": phase}
    if phase.startswith("placement_"):
        result["placementGroup"] = "CREATIVE_IOS"
    return result


class UploadMock(Mock):
    def __init__(self, case, steps):
        self.case = case
        super().__init__([self.reply(value) for value in steps])

    def reply(self, expected):
        def send(call):
            method, url, headers, body = call
            self.case.assertEqual(method, expected["method"])
            self.case.assertEqual(urlsplit(url).path, expected["path"])
            self.case.assertEqual(parse_qs(urlsplit(url).query), {k: [v] for k, v in expected["query"].items()})
            for name, value in expected["headers"].items():
                self.case.assertEqual(headers.get(name), value)
            actual = body if isinstance(expected["body"], bytes) else json.loads(body) if body else None
            self.case.assertEqual(actual, expected["body"])
            if expected["error"]:
                raise expected["error"]
            return response(expected["payload"], expected["status"])
        return send

    def send(self, method, url, headers, body, timeout):
        if urlsplit(url).netloc == "api.appstoreconnect.apple.com":
            return super().send(method, url, headers, body, timeout)
        self.case.assertEqual(urlsplit(url).scheme, "https")
        self.case.assertEqual(urlsplit(url).netloc, "store-030.blobstore.apple.com")
        self.case.assertGreater(timeout, 0)
        self.case.assertLessEqual(timeout, 60)
        headers = {k.lower(): v for k, v in headers.items()}
        self.case.assertNotIn("authorization", headers)
        self.calls.append((method, url, headers, body))
        self.case.assertTrue(self.responses, f"Unexpected upload request: {url}")
        return self.responses.pop(0)(self.calls[-1])


class UploadTests(unittest.TestCase):
    def assert_error(self, code, operation):
        with self.assertRaises(ApiError) as caught:
            operation()
        self.assertEqual(caught.exception.code, code)
        self.assertFalse(caught.exception.retryable)
        return caught.exception

    def test_reads_live_reference_catalog(self):
        mock = UploadMock(self, [step("/v1/appAssetLibraryRefData", catalog())])
        self.assertEqual(mock.client.listing_image_reference_data(), catalog())
        self.assertFalse(mock.responses)

    def test_upload_exact_chunks_checkpoint_snapshots_and_verified_receipt(self):
        half = len(PNG) // 2
        steps = preflight() + [reservation([
            upload_operation("/upload/second", half, len(PNG) - half),
            upload_operation("/upload/first", 0, half),
        ]), transfer("/upload/second", PNG[half:]), transfer("/upload/first", PNG[:half]), commit()]
        steps += placements() + [create_placement()] + placements(placement())
        mock = UploadMock(self, steps)
        state, saves = {}, []
        def persist(snapshot):
            saves.append(snapshot)
        receipt = mock.client.upload_listing_image(REQUEST, PNG, state, persist)
        self.assertEqual(receipt, RECEIPT)
        self.assertEqual(state["phase"], "placement_created")
        self.assertEqual(state["placementId"], "placement1")
        self.assertEqual([value["phase"] for value in saves], [
            "reservation_started", "reserved", "upload_started", "commit_started",
            "committed", "placement_started", "placement_created",
        ])
        self.assertTrue(all(snapshot is not state for snapshot in saves))
        self.assertEqual(len({id(snapshot) for snapshot in saves}), len(saves))
        self.assertNotIn("imageId", saves[0])
        self.assertEqual(saves[1]["imageId"], "image1")
        self.assertNotIn("signed-fixture", json.dumps(state))
        self.assertNotIn(BLOB_ORIGIN, json.dumps(state))
        self.assertFalse(mock.responses)

    def test_uncertain_reservation_updates_checkpoint_and_never_replays(self):
        mock = UploadMock(self, preflight() + [reservation(error=OSError("private transport failure"))])
        state, saves = {}, []
        self.assert_error("write_outcome_unknown", lambda: mock.client.upload_listing_image(REQUEST, PNG, state, saves.append))
        self.assertEqual(state["phase"], "reservation_started")
        self.assertNotIn("imageId", state)
        self.assertEqual(sum(call[0] == "POST" for call in mock.calls), 1)
        before = len(mock.calls)
        self.assert_error("write_outcome_unknown", lambda: mock.client.upload_listing_image(REQUEST, PNG, state, saves.append))
        self.assertEqual(len(mock.calls), before)
        self.assertFalse(mock.responses)

    def test_resume_uncertain_placement_uses_only_reads(self):
        mock = UploadMock(self, scope() + placements(placement()) + [image_read()])
        state, saves = checkpoint("placement_started"), []
        self.assertEqual(mock.client.upload_listing_image(REQUEST, PNG, state, saves.append), RECEIPT)
        self.assertEqual(state["placementId"], "placement1")
        self.assertEqual(state["phase"], "placement_created")
        self.assertTrue(all(call[0] == "GET" for call in mock.calls))
        self.assertFalse(mock.responses)

    def test_uncertain_placement_without_matching_readback_never_posts(self):
        mock = UploadMock(self, scope() + placements())
        self.assert_error("write_outcome_unknown", lambda: mock.client.upload_listing_image(
            REQUEST, PNG, checkpoint("placement_started"), lambda _: None))
        self.assertTrue(all(call[0] == "GET" for call in mock.calls))
        self.assertFalse(mock.responses)

    def test_uncertain_commit_updates_checkpoint_and_resume_never_repeats_patch(self):
        mock = UploadMock(self, preflight() + [reservation(), transfer(), commit(error=OSError("lost response"))])
        state = {}
        self.assert_error("write_outcome_unknown", lambda: mock.client.upload_listing_image(REQUEST, PNG, state, lambda _: None))
        self.assertEqual(state["phase"], "commit_started")
        resumed = UploadMock(self, preflight() + [image_read("AWAITING_UPLOAD")])
        self.assert_error("write_outcome_unknown", lambda: resumed.client.upload_listing_image(REQUEST, PNG, state, lambda _: None))
        self.assertTrue(all(call[0] == "GET" for call in resumed.calls))
        self.assertFalse(mock.responses)
        self.assertFalse(resumed.responses)

    def test_persistence_failure_preserves_original_exception_and_prevents_reservation(self):
        mock = UploadMock(self, preflight())
        state = {}
        failure = RuntimeError("checkpoint disk unavailable")
        def persist(snapshot):
            self.assertEqual(snapshot["phase"], "reservation_started")
            raise failure
        with self.assertRaises(RuntimeError) as caught:
            mock.client.upload_listing_image(REQUEST, PNG, state, persist)
        self.assertIs(caught.exception, failure)
        self.assertEqual(state["phase"], "reservation_started")
        self.assertTrue(all(call[0] == "GET" for call in mock.calls))
        self.assertFalse(mock.responses)

    def test_mutating_persist_snapshot_does_not_change_native_checkpoint(self):
        mock = UploadMock(self, preflight() + [reservation(error=OSError("lost response"))])
        state = {}
        def persist(snapshot):
            snapshot.clear()
            snapshot["phase"] = "forged"
        self.assert_error("write_outcome_unknown", lambda: mock.client.upload_listing_image(REQUEST, PNG, state, persist))
        self.assertEqual(state["phase"], "reservation_started")
        self.assertEqual(state["binding"], BINDING)
        self.assertFalse(mock.responses)

    def test_same_checkpoint_cannot_be_reentered_from_persist(self):
        mock = UploadMock(self, preflight() + [reservation(error=OSError("lost response"))])
        state = {}
        def persist(_):
            self.assert_error("checkpoint_in_use", lambda: mock.client.upload_listing_image(REQUEST, PNG, state, lambda _: None))
        self.assert_error("write_outcome_unknown", lambda: mock.client.upload_listing_image(REQUEST, PNG, state, persist))
        self.assertFalse(mock.responses)

    def test_invalid_png_and_changed_request_do_not_dispatch(self):
        mock = UploadMock(self, [])
        for png in (b"not PNG", PNG[:24]):
            self.assert_error("invalid_png", lambda: mock.client.upload_listing_image(REQUEST, png, {}, lambda _: None))
        changed = {**REQUEST, "fileName": "different.png"}
        self.assert_error("checkpoint_mismatch", lambda: mock.client.upload_listing_image(changed, PNG, checkpoint("committed"), lambda _: None))
        self.assertFalse(mock.calls)

    def test_untrusted_upload_operations_never_dispatch_chunks_or_commit(self):
        invalid = upload_operation()
        invalid["requestHeaders"] = [{"name": "Authorization", "value": "Bearer private"}]
        for operations in ([invalid], [upload_operation(offset=1, length=len(PNG) - 1)],
                           [{**upload_operation(), "url": "https://blobstore.apple.com.evil.invalid/upload"}]):
            with self.subTest(operations=operations):
                mock = UploadMock(self, preflight() + [reservation(operations)])
                state = {}
                self.assert_error("upstream_invalid_response", lambda: mock.client.upload_listing_image(REQUEST, PNG, state, lambda _: None))
                self.assertEqual(state["imageId"], "image1")
                self.assertFalse(any(call[0] in ("PUT", "PATCH") for call in mock.calls))
                self.assertFalse(mock.responses)


if __name__ == "__main__":
    unittest.main()
