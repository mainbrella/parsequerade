import hashlib
import hmac
import json
import secrets
import time
from urllib.parse import parse_qsl, urlencode, urljoin, urlsplit

from ._errors import ApiError, invalid_upstream
from ._transport import ORIGIN, b64, unb64


def apple_url(path, query):
    return ORIGIN + path + ("?" + urlencode(sorted(query.items())) if query else "")


def digest(value):
    return b64(hashlib.sha256(value.encode()).digest()[:16])


def next_page(payload, initial):
    link = payload.get("links", {}).get("next")
    if link is None:
        return None
    if not isinstance(link, str) or len(link) > 8192:
        raise invalid_upstream()
    try:
        url = urlsplit(urljoin(initial, link))
        start = urlsplit(initial)
        if url.scheme != start.scheme or url.netloc != start.netloc or url.path != start.path or url.fragment or url.username or url.password:
            raise invalid_upstream()
        original = dict(parse_qsl(start.query, keep_blank_values=True))
        seen, cursor = set(), None
        for name, value in parse_qsl(url.query, keep_blank_values=True):
            if name in seen or (name != "cursor" and original.get(name) != value):
                raise invalid_upstream()
            seen.add(name)
            if name == "cursor":
                cursor = value
        if not cursor or len(cursor) > 2048:
            raise invalid_upstream()
        return apple_url(start.path, {**original, "cursor": cursor})
    except ValueError:
        raise invalid_upstream() from None


class Cursors:
    def __init__(self):
        self.key = secrets.token_bytes(32)

    def encode(self, value):
        payload = b64(json.dumps(value, separators=(",", ":")).encode())
        token = payload + "." + b64(hmac.digest(self.key, payload.encode(), "sha256"))
        if len(token) > 8192:
            raise invalid_upstream()
        return token

    def decode(self, token, binding):
        try:
            if len(token) > 8192:
                raise ValueError()
            payload, signature = token.split(".")
            if not hmac.compare_digest(hmac.digest(self.key, payload.encode(), "sha256"), unb64(signature)):
                raise ValueError()
            value = json.loads(unb64(payload))
            if (value["binding"] != binding or not time.time() < value["expires"] <= time.time() + 900
                    or not isinstance(value["next"], str) or not 0 < len(value["next"]) <= 2048
                    or not isinstance(value["seen"], list) or not 0 < len(value["seen"]) <= 100
                    or any(not isinstance(v, str) for v in value["seen"]) or len(set(value["seen"])) != len(value["seen"])):
                raise ValueError()
            return value
        except (ValueError, KeyError, TypeError):
            raise ApiError(400, "invalid_cursor", "The cursor is invalid, expired, or belongs to another client or query.") from None
