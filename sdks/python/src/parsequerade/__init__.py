"""Run the Parsequerade facade locally with an Apple team API key."""
from ._generated import *
from ._generated import __all__ as _models
from ._client import Client
from ._errors import ApiError
from ._transport import Response

__all__ = ["Client", "ApiError", "Response", *_models]
