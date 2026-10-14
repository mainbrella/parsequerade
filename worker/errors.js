export class ApiError extends Error {
  constructor(status, code, message, { retryable = false, details = [], headers = {} } = {}) {
    super(message);
    Object.assign(this, { status, code, retryable, details, headers });
  }
}

export function invalidQuery(field, message = "This query parameter is not supported.") {
  return new ApiError(400, "invalid_query", "The request query is invalid.", {
    details: [{ field, code: "invalid_parameter", message }],
  });
}

export function invalidUpstream() {
  return new ApiError(502, "upstream_invalid_response", "Apple returned an invalid response.");
}

export function notFound(code = "resource_not_found") {
  return new ApiError(404, code, code === "localization_not_found" ? "The exact locale was not found." : "The requested resource was not found.");
}

export function configurationError() {
  return new ApiError(503, "service_not_configured", "The operator must configure this connection before it can be used.");
}

export function jsonResponse(body, status, requestId, extraHeaders = {}) {
  return new Response(JSON.stringify(body), { status, headers: {
    "Content-Type": "application/json; charset=utf-8", "Cache-Control": "private, no-store",
    "X-Content-Type-Options": "nosniff", "X-Request-Id": requestId, ...extraHeaders,
  } });
}
