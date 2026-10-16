"""Stable, sanitized facade errors from the shared execution core."""


class ApiError(Exception):
    def __init__(self, status, code, message, *, retryable=False, details=None, retry_after=None):
        super().__init__(f"{code}: {message}")
        self.status = status
        self.code = code
        self.message = message
        self.retryable = retryable
        self.details = details or []
        self.retry_after = retry_after

    @classmethod
    def from_dict(cls, error):
        return cls(error["status"], error["code"], error["message"],
                   retryable=error["retryable"], details=error["details"],
                   retry_after=error.get("retryAfter"))


def invalid_query(field):
    return ApiError(400, "invalid_query", "The request query is invalid.", details=[{
        "code": "invalid_parameter", "message": "Provide a supported parameter and value.", "field": field,
    }])
