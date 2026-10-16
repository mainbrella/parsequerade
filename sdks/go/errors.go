package parsequerade

import "fmt"

type ErrorDetail struct {
	Code    string `json:"code"`
	Message string `json:"message"`
	Field   string `json:"field,omitempty"`
}

// APIError is a stable facade error. Raw Apple messages and credentials are omitted.
type APIError struct {
	Status     int           `json:"status"`
	Code       string        `json:"code"`
	Message    string        `json:"message"`
	Retryable  bool          `json:"retryable"`
	RetryAfter *int          `json:"retryAfter"` // seconds; nil when Apple supplied no usable Retry-After
	Details    []ErrorDetail `json:"details"`
}

func (e *APIError) Error() string { return fmt.Sprintf("%s: %s", e.Code, e.Message) }

func apiError(status int, code, message string) *APIError {
	return &APIError{Status: status, Code: code, Message: message}
}

func invalidUpstream() *APIError {
	return apiError(502, "upstream_invalid_response", "Apple returned an invalid response.")
}

func invalidQuery(field string) *APIError {
	e := apiError(400, "invalid_query", "The request query is invalid.")
	e.Details = []ErrorDetail{{Code: "invalid_parameter", Message: "Provide a supported parameter and value.", Field: field}}
	return e
}
