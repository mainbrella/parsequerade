package parsequerade

import (
	"context"
	"net/http"
	"testing"
	"time"
)

func TestExpiredHTTPEventNeverReachesTransport(t *testing.T) {
	calls := 0
	client := &Client{transport: roundTripFunc(func(*http.Request) (*http.Response, error) {
		calls++
		return reply(nil, 200), nil
	})}
	event := nativeResult{Method: "PATCH", URL: "https://api.appstoreconnect.apple.com/v1/apps/app1", TimeoutMS: 8000, DeadlineMS: time.Now().Add(-time.Millisecond).UnixMilli(), MaxResponseBytes: 1024}
	response := client.send(context.Background(), event)
	if response["error"] != "timeout" || calls != 0 {
		t.Fatalf("expired event dispatched: %#v, calls %d", response, calls)
	}
}

func TestHTTPEventUsesRemainingAttemptDeadline(t *testing.T) {
	deadline := time.Now().Add(2 * time.Second)
	client := &Client{transport: roundTripFunc(func(request *http.Request) (*http.Response, error) {
		actual, ok := request.Context().Deadline()
		if !ok || actual.After(deadline) || time.Until(actual) <= 0 {
			t.Errorf("attempt deadline was extended: %v, want <= %v", actual, deadline)
		}
		return reply(nil, 200), nil
	})}
	event := nativeResult{Method: "GET", URL: "https://api.appstoreconnect.apple.com/v1/apps", TimeoutMS: 8000, DeadlineMS: deadline.UnixMilli(), MaxResponseBytes: 1024}
	response := client.send(context.Background(), event)
	if response["status"] != 200 {
		t.Fatalf("unexpected transport result %#v", response)
	}
}
