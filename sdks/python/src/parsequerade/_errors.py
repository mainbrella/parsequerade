"""Stable, sanitized facade errors."""


class ApiError(Exception):
    def __init__(self, status, code, message, *, retryable=False, details=None, retry_after=None):
        super().__init__(f"{code}: {message}")
        self.status = status
        self.code = code
        self.message = message
        self.retryable = retryable
        self.details = details or []
        self.retry_after = retry_after


def invalid_upstream():
    return ApiError(502, "upstream_invalid_response", "Apple returned an invalid response.")


def not_found(code="resource_not_found"):
    return ApiError(404, code, "The requested resource was not found.")


def incomplete():
    return ApiError(502, "upstream_incomplete_result", "Apple pagination exceeded its bound or repeated a page.")


def unknown_write():
    return ApiError(503, "write_outcome_unknown", "Apple did not confirm this update. Read the age rating before trying again.")


def invalid_query(field):
    return ApiError(400, "invalid_query", "The request query is invalid.", details=[{
        "code": "invalid_parameter", "message": "Provide a supported parameter and value.", "field": field,
    }])
