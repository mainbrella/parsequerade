"""Synchronous, idiomatic Python access to the shared Rust execution core."""
import json
import threading
from collections.abc import Mapping

from . import _native
from ._errors import ApiError, invalid_query
from ._generated import (
    AgeRating, AgeRatingUpdate, ListingImageUpload, ListingImageUploadCheckpoint,
    ListingImageUploadReceipt, Query, ResourceClient,
)

_checkpoint_lock = threading.Lock()
_active_checkpoints: set[int] = set()


def _json(value):
    try:
        return json.dumps(value, allow_nan=False, separators=(",", ":"))
    except (TypeError, ValueError, OverflowError):
        raise ApiError(400, "invalid_request", "Provide a JSON-compatible request.") from None


def _query(query):
    if query is None:
        return []
    pairs = query.items() if isinstance(query, Mapping) else query
    result = []
    try:
        for name, value in pairs:
            if not isinstance(name, str):
                raise invalid_query("query")
            if isinstance(value, (list, tuple)) and all(isinstance(item, str) for item in value):
                value = ",".join(value)
            elif type(value) is bool:
                value = str(value).lower()
            elif type(value) is int:
                value = str(value)
            if not isinstance(value, str):
                raise invalid_query(name)
            result.append((name, value))
    except (TypeError, ValueError):
        raise invalid_query("query") from None
    return result


def _result(encoded):
    result = json.loads(encoded)
    if "error" in result:
        raise ApiError.from_dict(result["error"])
    return result["value"]


def _invoke(method, *args):
    try:
        return method(*args)
    except _native.Error as error:
        raise ApiError.from_dict(json.loads(str(error))) from None


class Client(ResourceClient):
    def __init__(self, issuer_id: str, key_id: str, private_key: str, *, transport=None):
        """Load a P-256 PKCS#8 key locally. Custom transports receive signed requests."""
        if not all(isinstance(value, str) for value in (issuer_id, key_id, private_key)):
            raise ApiError(400, "invalid_credentials", "Provide an issuer ID, key ID and a P-256 PKCS#8 private key.")
        self._core = _invoke(_native.Client, issuer_id, key_id, private_key, transport)

    def get(self, path: str, query: Query | None = None) -> dict:
        """Read a facade path, preserving absent fields and explicit JSON nulls."""
        if not isinstance(path, str):
            raise ApiError(404, "route_not_found", "The requested resource was not found.")
        return _result(_invoke(self._core.get, path, _query(query)))

    def update_age_rating(self, app_id: str, app_info_id: str, attributes: AgeRatingUpdate) -> AgeRating:
        """Resolve ownership and send one PATCH; uncertain writes are never retried."""
        return _result(_invoke(self._core.update_age_rating, app_id, app_info_id, _json(attributes)))

    def update_metadata(self, path: str, attributes: dict) -> dict:
        """Update supported editable Store metadata on an app, version, or localization."""
        return _result(_invoke(self._core.update_metadata, path, _json(attributes)))

    def create_version(self, app_id: str, platform: str, version_string: str) -> dict:
        """Create one unsubmitted version. Read versions before retrying an uncertain write."""
        return _result(_invoke(self._core.create_version, app_id, platform, version_string))

    def attach_build(self, app_id: str, version_id: str, build_id: str) -> dict:
        """Attach an eligible build to its matching draft version and read back to verify."""
        return _result(_invoke(self._core.attach_build, app_id, version_id, build_id))

    def listing_image_reference_data(self) -> dict:
        """Read Apple's live asset specifications and placement groups without writes."""
        return _result(_invoke(self._core.listing_image_reference_data))

    def upload_listing_image(
        self, request: ListingImageUpload, png: bytes,
        checkpoint: ListingImageUploadCheckpoint, persist,
    ) -> ListingImageUploadReceipt:
        """Upload or resume one PNG, updating ``checkpoint`` even when the operation fails.

        ``persist(snapshot)`` must durably save each independent checkpoint snapshot
        and return None. Its exceptions stop the operation before the next write.
        Reuse the same checkpoint and PNG to reconcile an uncertain upload.
        """
        if not isinstance(checkpoint, dict) or not callable(persist):
            raise TypeError("checkpoint must be a dict and persist must be callable")
        if not isinstance(png, bytes):
            raise TypeError("png must be bytes")
        identity = id(checkpoint)
        with _checkpoint_lock:
            if identity in _active_checkpoints:
                raise ApiError(409, "checkpoint_in_use", "This checkpoint already has an upload in progress.")
            _active_checkpoints.add(identity)
        try:
            def save(encoded):
                snapshot = json.loads(encoded)
                checkpoint.clear()
                checkpoint.update(snapshot)
                if persist(json.loads(encoded)) is not None:
                    raise TypeError("persist must return None after durably saving the checkpoint")

            encoded, callback_error = _invoke(
                self._core.upload_listing_image, _json(request), png, _json(checkpoint), save,
            )
            result = json.loads(encoded)
            checkpoint.clear()
            checkpoint.update(result["checkpoint"])
            if callback_error is not None:
                raise callback_error
            return _result(encoded)
        finally:
            with _checkpoint_lock:
                _active_checkpoints.remove(identity)
