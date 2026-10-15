"""Bounded direct Apple transport. Redirects are never followed."""
import base64
import http.client
import json
import math
import threading
import time
from dataclasses import dataclass, field
from email.utils import parsedate_to_datetime
from urllib.parse import urlsplit

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.exceptions import UnsupportedAlgorithm
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature

from ._errors import ApiError, invalid_upstream, unknown_write

ORIGIN = "https://api.appstoreconnect.apple.com"
RESPONSE_BYTES = 4 * 1024 * 1024
TOTAL_BYTES = 16 * 1024 * 1024


def b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def unb64(value):
    return base64.b64decode(value + "=" * (-len(value) % 4), altchars=b"-_", validate=True)


class Auth:
    def __init__(self, issuer_id, key_id, pem):
        try:
            if not all(isinstance(v, str) and 0 < len(v) <= 128 for v in (issuer_id, key_id)):
                raise ValueError()
            if not isinstance(pem, str) or not pem.strip().startswith("-----BEGIN PRIVATE KEY-----"):
                raise ValueError()
            self.key = serialization.load_pem_private_key(pem.encode(), password=None)
            if not isinstance(self.key, ec.EllipticCurvePrivateKey) or not isinstance(self.key.curve, ec.SECP256R1):
                raise ValueError()
        except (ValueError, TypeError, UnicodeError, UnsupportedAlgorithm):
            raise ApiError(400, "invalid_credentials", "Provide an issuer ID, key ID and a P-256 PKCS#8 private key.") from None
        self.issuer_id, self.key_id = issuer_id, key_id
        self._cached = None
        self._lock = threading.Lock()

    def token(self):
        with self._lock:
            now = int(time.time())
            if self._cached and self._cached[1] > now + 60:
                return self._cached[0]
            header = b64(json.dumps({"alg": "ES256", "kid": self.key_id, "typ": "JWT"}).encode())
            payload = b64(json.dumps({"iss": self.issuer_id, "iat": now - 30, "exp": now + 600, "aud": "appstoreconnect-v1"}).encode())
            content = f"{header}.{payload}"
            r, s = decode_dss_signature(self.key.sign(content.encode(), ec.ECDSA(hashes.SHA256())))
            token = content + "." + b64(r.to_bytes(32, "big") + s.to_bytes(32, "big"))
            self._cached = token, now + 600
            return token

    def invalidate(self, rejected):
        with self._lock:
            if self._cached and self._cached[0] == rejected:
                self._cached = None


@dataclass
class Budget:
    deadline: float = field(default_factory=lambda: time.monotonic() + 25)
    attempts: int = 0
    bytes: int = 0


@dataclass(frozen=True)
class Response:
    """Response returned by a trusted custom transport; body must be bounded bytes."""
    status: int
    headers: dict[str, str]
    body: bytes


def https_transport(method, url, headers, body, timeout):
    """One request; standard library HTTPS verifies TLS and does not redirect."""
    parsed = urlsplit(url)
    connection = http.client.HTTPSConnection(parsed.hostname, timeout=timeout)
    deadline = time.monotonic() + timeout
    try:
        connection.request(method, parsed.path + ("?" + parsed.query if parsed.query else ""), body, headers)
        sock = connection.sock
        if sock:
            sock.settimeout(max(0.001, deadline - time.monotonic()))
        response = connection.getresponse()
        headers = {k.lower(): v for k, v in response.getheaders()}
        length = headers.get("content-length", "0")
        if not length.isascii() or not length.isdigit() or int(length) > RESPONSE_BYTES:
            raise invalid_upstream()
        chunks, size = [], 0
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError()
            if sock:
                sock.settimeout(remaining)
            chunk = response.read1(min(65536, RESPONSE_BYTES + 1 - size))
            if not chunk:
                break
            size += len(chunk)
            if size > RESPONSE_BYTES:
                raise invalid_upstream()
            chunks.append(chunk)
        return Response(response.status, headers, b"".join(chunks))
    finally:
        connection.close()


def upstream_error(status, payload, headers, route):
    mapped, code, message = {
        401: (502, "upstream_authentication_failed", "Apple rejected the configured credentials."),
        403: (403, "upstream_forbidden", "The Apple key cannot access this resource."),
        404: (404, "resource_not_found", "The requested resource was not found."),
        409: (409, "upstream_conflict", "Apple reported a resource conflict."),
        400: (422, "validation_failed", "Apple rejected one or more request parameters."),
        422: (422, "validation_failed", "Apple rejected one or more request parameters."),
        429: (429, "rate_limited", "Apple is throttling this connection."),
    }.get(status, (503 if status >= 500 else 502, "upstream_unavailable", "Apple could not complete this request."))
    after = None
    try:
        raw = headers.get("retry-after", "")
        after = int(raw) if raw.isascii() and raw.isdigit() else max(0, math.ceil(parsedate_to_datetime(raw).timestamp() - time.time()))
        after = min(after, 86400)
    except (ValueError, TypeError, OverflowError):
        pass
    details = []
    import re
    errors = payload.get("errors", []) if isinstance(payload, dict) else []
    for error in errors[:32] if isinstance(errors, list) else []:
        if not isinstance(error, dict):
            continue
        safe_code = error.get("code")
        detail = {"code": safe_code if isinstance(safe_code, str) and re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", safe_code) else "upstream_error", "message": message}
        source = error.get("source")
        for p in route["parameters"]:
            if p["appleName"] and isinstance(source, dict) and source.get("parameter") == p["appleName"]:
                detail["field"] = p["name"]
        details.append(detail)
    return ApiError(mapped, code, message, retryable=status == 429 or status >= 500, details=details, retry_after=after)


class Transport:
    def __init__(self, auth, transport=None):
        self.auth = auth
        self._send = transport or https_transport
        self._permits = threading.BoundedSemaphore(4)

    def apple(self, url, route, budget, body=None):
        parsed = urlsplit(url)
        if (parsed.scheme + "://" + parsed.netloc != ORIGIN or parsed.fragment
                or not parsed.path.startswith("/v1/") or any(not (c.isascii() and (c.isalnum() or c in "/_-")) for c in parsed.path)):
            raise invalid_upstream()
        write = body is not None
        if not self._permits.acquire(blocking=False):
            raise ApiError(429, "rate_limited", "This client has too many concurrent Apple requests.", retryable=not write, retry_after=1)
        try:
            refreshed = False
            for attempt in range(1 if write else 3):
                if budget.attempts >= 32 or time.monotonic() >= budget.deadline:
                    raise ApiError(504, "upstream_budget_exceeded", "The bounded Apple operation could not finish.", retryable=not write)
                budget.attempts += 1
                jwt = self.auth.token()
                headers = {"Authorization": "Bearer " + jwt, "Accept": "application/json", "User-Agent": "parsequerade-python/0.1.0"}
                if write:
                    headers["Content-Type"] = "application/json"
                retry_auth = False
                try:
                    response = self._send("PATCH" if write else "GET", url, headers, json.dumps(body).encode() if write else None, min(8, budget.deadline - time.monotonic()))
                    if time.monotonic() >= budget.deadline:
                        raise TimeoutError()
                    headers = {k.lower(): v for k, v in response.headers.items()}
                    budget.bytes += len(response.body)
                    if len(response.body) > RESPONSE_BYTES or budget.bytes > TOTAL_BYTES or 300 <= response.status < 400:
                        raise invalid_upstream()
                    try:
                        payload = json.loads(response.body)
                    except (ValueError, UnicodeError):
                        payload = None
                    if 200 <= response.status < 300:
                        mime = headers.get("content-type", "").split(";")[0].strip().lower()
                        if (not (mime == "application/json" or (mime.startswith("application/") and mime.endswith("+json")))
                                or not isinstance(payload, dict) or "data" not in payload):
                            raise invalid_upstream()
                        return payload
                    retry_auth = response.status == 401 and not write and not refreshed
                    if retry_auth:
                        self.auth.invalidate(jwt)
                        refreshed = True
                    error = upstream_error(response.status, payload, headers, route)
                    if write:
                        error.retryable = False
                except ApiError as caught:
                    error = unknown_write() if write and caught.code == "upstream_invalid_response" else caught
                except (OSError, http.client.HTTPException) as caught:
                    error = unknown_write() if write else ApiError(504 if isinstance(caught, TimeoutError) else 503,
                        "upstream_timeout" if isinstance(caught, TimeoutError) else "upstream_unavailable", "The Apple read could not complete.", retryable=True)
                if write or attempt == 2 or (not error.retryable and not retry_auth):
                    raise error
                delay = 0 if retry_auth else (error.retry_after if error.retry_after is not None else 0.2 * 2 ** attempt)
                if delay > 2 or time.monotonic() + delay >= budget.deadline:
                    raise error
                time.sleep(delay)
        finally:
            self._permits.release()
