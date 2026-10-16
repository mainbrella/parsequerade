"""Validate generated method arguments before inserting them into resource paths."""
import re

from ._errors import ApiError

_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]{0,127}\Z")
_LOCALE = re.compile(r"[A-Za-z0-9][A-Za-z0-9-]{1,34}\Z")


def validate_segment(value, locale=False):
    if not isinstance(value, str) or not (_LOCALE if locale else _ID).fullmatch(value):
        raise ApiError(400, "invalid_path", "The resource path is invalid.")
