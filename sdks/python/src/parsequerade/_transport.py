"""Custom transport response. HTTP policy and execution live in the Rust core."""
from dataclasses import dataclass


@dataclass(frozen=True)
class Response:
    """Response from a trusted custom transport; body must contain bounded bytes.

    A transport receives (method, url, headers, body, timeout_seconds). It must
    verify TLS, avoid redirects, and honor the timeout. It receives credentials
    and, for uploads, Apple-issued storage URLs and headers.
    """
    status: int
    headers: dict[str, str]
    body: bytes
