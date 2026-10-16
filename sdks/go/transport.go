package parsequerade

import (
	"bytes"
	"context"
	"encoding/base64"
	"errors"
	"io"
	"net/http"
	"strings"
	"time"
)

// send only adapts the Go transport to the core. Request policy, signing,
// deadlines, retries, response validation and write safety belong to Rust.
func (c *Client) send(ctx context.Context, event nativeResult) map[string]any {
	fail := func(code string) map[string]any { return map[string]any{"error": code} }
	var body []byte
	if event.Body != nil {
		var err error
		body, err = base64.StdEncoding.DecodeString(*event.Body)
		if err != nil {
			return fail("invalid_response")
		}
	}
	if event.MaxResponseBytes < 0 {
		return fail("invalid_response")
	}
	timeout := time.Duration(event.TimeoutMS) * time.Millisecond
	if remaining := time.Until(time.UnixMilli(event.DeadlineMS)); remaining < timeout {
		timeout = remaining
	}
	if timeout <= 0 {
		return fail("timeout")
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	request, err := http.NewRequestWithContext(ctx, event.Method, event.URL, bytes.NewReader(body))
	if err != nil {
		return fail("failed")
	}
	for name, value := range event.Headers {
		request.Header.Set(name, value)
	}
	// Body decoding and request construction can consume the remaining budget.
	// Never hand a stale queued event to a user transport, including one that
	// would ignore an already-expired request context.
	if time.Until(time.UnixMilli(event.DeadlineMS)) <= 0 || ctx.Err() == context.DeadlineExceeded {
		return fail("timeout")
	}
	if ctx.Err() != nil {
		return fail("failed")
	}
	response, err := c.transport.RoundTrip(request)
	if err != nil {
		if response != nil && response.Body != nil {
			response.Body.Close()
		}
		if errors.Is(err, context.DeadlineExceeded) || ctx.Err() == context.DeadlineExceeded {
			return fail("timeout")
		}
		return fail("failed")
	}
	if response == nil || response.Body == nil {
		return fail("invalid_response")
	}
	defer response.Body.Close()
	if response.StatusCode < 100 || response.StatusCode > 599 {
		return fail("invalid_response")
	}
	payload, err := io.ReadAll(io.LimitReader(response.Body, event.MaxResponseBytes+1))
	if int64(len(payload)) > event.MaxResponseBytes {
		return fail("invalid_response")
	}
	if err != nil {
		if errors.Is(err, context.DeadlineExceeded) || ctx.Err() == context.DeadlineExceeded {
			return fail("timeout")
		}
		return fail("failed")
	}
	headers := map[string]string{}
	for name, values := range response.Header {
		headers[strings.ToLower(name)] = strings.Join(values, ", ")
	}
	return map[string]any{"status": response.StatusCode, "headers": headers, "body": base64.StdEncoding.EncodeToString(payload)}
}
