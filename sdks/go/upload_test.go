package parsequerade

import (
	"bytes"
	"context"
	"crypto/elliptic"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"reflect"
	"sync"
	"testing"
	"time"
)

type uploadStep struct {
	Method     string            `json:"method"`
	Path       string            `json:"path"`
	Query      map[string]string `json:"query"`
	Headers    map[string]string `json:"headers"`
	Body       any               `json:"body"`
	BodyBase64 *string           `json:"bodyBase64"`
	Payload    any               `json:"payload"`
	Status     int               `json:"status"`
	Error      string            `json:"error"`
}
type uploadCase struct {
	Name            string                       `json:"name"`
	Steps           []uploadStep                 `json:"steps"`
	Checkpoint      ListingImageUploadCheckpoint `json:"checkpoint"`
	Expect          *ListingImageUploadReceipt   `json:"expect"`
	Error           string                       `json:"error"`
	PersistFailure  bool                         `json:"persistFailure"`
	CancelOnPersist bool                         `json:"-"`
	CloseOnPersist  bool                         `json:"-"`
}

func TestUploadCheckpointAndRecovery(t *testing.T) {
	data, err := os.ReadFile("testdata/uploads.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixtures struct {
		Request ListingImageUpload `json:"request"`
		Binding string             `json:"binding"`
		Cases   []uploadCase       `json:"cases"`
	}
	if err := json.Unmarshal(data, &fixtures); err != nil {
		t.Fatal(err)
	}
	png, err := os.ReadFile("testdata/header.png")
	if err != nil {
		t.Fatal(err)
	}
	cases := append([]uploadCase{}, fixtures.Cases...)
	for _, fixture := range fixtures.Cases {
		if fixture.PersistFailure {
			fixture.PersistFailure = false
			fixture.Error = "write_outcome_unknown"
			fixture.Name = "cancel_before_reservation"
			fixture.CancelOnPersist = true
			cases = append(cases, fixture)
			fixture.Name = "close_before_reservation"
			fixture.CancelOnPersist = false
			fixture.CloseOnPersist = true
			cases = append(cases, fixture)
		}
	}
	for _, fixture := range cases {
		t.Run(fixture.Name, func(t *testing.T) {
			_, pem := testKey(t, elliptic.P256())
			index := 0
			client, err := NewClient(Credentials{IssuerID: "issuer", KeyID: "key", PrivateKey: pem}, &Options{Transport: roundTripFunc(func(r *http.Request) (*http.Response, error) {
				if index >= len(fixture.Steps) {
					return nil, fmt.Errorf("unexpected request %s %s", r.Method, r.URL)
				}
				step := fixture.Steps[index]
				index++
				if r.Method != step.Method || r.URL.Path != step.Path {
					t.Errorf("step %d: got %s %s, want %s %s", index, r.Method, r.URL.Path, step.Method, step.Path)
				}
				query := url.Values{}
				for key, value := range step.Query {
					query.Set(key, value)
				}
				if !reflect.DeepEqual(r.URL.Query(), query) {
					t.Errorf("step %d query %v != %v", index, r.URL.Query(), query)
				}
				for name, value := range step.Headers {
					if r.Header.Get(name) != value {
						t.Errorf("header %s mismatch", name)
					}
				}
				if r.URL.Host == "store-030.blobstore.apple.com" {
					if r.Header.Get("Authorization") != "" {
						t.Error("signed Apple authorization leaked to blob host")
					}
				}
				body, _ := io.ReadAll(r.Body)
				if step.BodyBase64 != nil {
					expected, _ := base64.StdEncoding.DecodeString(*step.BodyBase64)
					if !bytes.Equal(body, expected) {
						t.Errorf("step %d chunk does not match PNG slice", index)
					}
				} else {
					var value any
					if len(body) != 0 {
						json.Unmarshal(body, &value)
					}
					if !reflect.DeepEqual(value, step.Body) {
						t.Errorf("step %d body %s != %#v", index, body, step.Body)
					}
				}
				if step.Error != "" {
					return nil, errors.New("private transport failure")
				}
				return reply(step.Payload, step.Status), nil
			})})
			if err != nil {
				t.Fatal(err)
			}
			defer client.Close()
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			checkpoint := fixture.Checkpoint
			diskError := errors.New("disk unavailable")
			phases := []string{}
			persist := func(snapshot ListingImageUploadCheckpoint) error {
				phases = append(phases, snapshot.Phase.Value)
				if fixture.CancelOnPersist {
					cancel()
					return nil
				}
				if fixture.CloseOnPersist {
					client.Close()
					return nil
				}
				if fixture.PersistFailure {
					return diskError
				}
				// A callback may retain or change its copy without changing core state.
				snapshot.Phase = Some("forged")
				_, err := client.UploadListingImage(context.Background(), fixtures.Request, png, &checkpoint, func(ListingImageUploadCheckpoint) error { return nil })
				requireCode(t, err, "checkpoint_in_use")
				return nil
			}
			receipt, err := client.UploadListingImage(ctx, fixtures.Request, png, &checkpoint, persist)
			if fixture.PersistFailure {
				if err != diskError {
					t.Fatalf("original callback error lost: %v", err)
				}
				if checkpoint.Phase.Value != "reservation_started" {
					t.Fatalf("checkpoint lost: %#v", checkpoint)
				}
			} else if fixture.Error != "" {
				requireCode(t, err, fixture.Error)
				if fixture.Name == "uncertain_reservation" {
					before := index
					_, err := client.UploadListingImage(context.Background(), fixtures.Request, png, &checkpoint, persist)
					requireCode(t, err, "write_outcome_unknown")
					if index != before {
						t.Fatal("uncertain reservation was retried")
					}
				}
				if fixture.Name == "uncertain_commit" && checkpoint.Phase.Value != "commit_started" {
					t.Fatal("uncertain commit checkpoint lost")
				}
			} else {
				if err != nil {
					t.Fatal(err)
				}
				if !reflect.DeepEqual(receipt, *fixture.Expect) {
					t.Fatalf("receipt %#v", receipt)
				}
				if checkpoint.Phase.Value != "placement_created" || checkpoint.PlacementID.Value != "placement1" {
					t.Fatalf("checkpoint %#v", checkpoint)
				}
				if fixture.Name == "complete_chunked_upload" {
					expected := []string{"reservation_started", "reserved", "upload_started", "commit_started", "committed", "placement_started", "placement_created"}
					if !reflect.DeepEqual(phases, expected) {
						t.Fatalf("checkpoint phases %v", phases)
					}
				}
			}
			if checkpoint.Binding.Value != fixtures.Binding {
				t.Fatal("checkpoint binding lost")
			}
			if index != len(fixture.Steps) {
				t.Fatalf("got %d requests, want %d", index, len(fixture.Steps))
			}
		})
	}
}

func TestInvalidUploadAndNativeJSONInputs(t *testing.T) {
	m := newMock(t)
	state := ListingImageUploadCheckpoint{}
	_, err := m.client.UploadListingImage(context.Background(), ListingImageUpload{}, []byte("not PNG"), &state, func(ListingImageUploadCheckpoint) error { return nil })
	if err == nil {
		t.Fatal("invalid upload accepted")
	}
	_, err = m.client.UpdateMetadata(context.Background(), "/apps/app1", map[string]any{"contentRightsDeclaration": make(chan int)})
	requireCode(t, err, "invalid_request")
	if len(m.calls) != 0 {
		t.Fatal("invalid input dispatched")
	}
}

func checkpointPreflight(t *testing.T) (ListingImageUpload, []byte, []any) {
	t.Helper()
	data, err := os.ReadFile("testdata/uploads.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixtures struct {
		Request ListingImageUpload `json:"request"`
		Cases   []uploadCase       `json:"cases"`
	}
	if err := json.Unmarshal(data, &fixtures); err != nil {
		t.Fatal(err)
	}
	png, err := os.ReadFile("testdata/header.png")
	if err != nil {
		t.Fatal(err)
	}
	for _, fixture := range fixtures.Cases {
		if fixture.PersistFailure {
			replies := make([]any, len(fixture.Steps))
			for i, step := range fixture.Steps {
				replies[i] = step.Payload
			}
			return fixtures.Request, png, replies
		}
	}
	t.Fatal("missing checkpoint preflight fixture")
	return ListingImageUpload{}, nil, nil
}

func TestBlockedCheckpointCallbackDoesNotBlockCancellation(t *testing.T) {
	for _, action := range []string{"cancel", "close"} {
		t.Run(action, func(t *testing.T) {
			request, png, replies := checkpointPreflight(t)
			m := newMock(t, replies...)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			entered, release, callbackReturned := make(chan struct{}), make(chan struct{}), make(chan struct{})
			var releaseOnce sync.Once
			t.Cleanup(func() { releaseOnce.Do(func() { close(release) }) })
			checkpoint := ListingImageUploadCheckpoint{}
			result := make(chan error, 1)
			lateError := errors.New("late disk failure")
			go func() {
				_, err := m.client.UploadListingImage(ctx, request, png, &checkpoint, func(snapshot ListingImageUploadCheckpoint) error {
					defer close(callbackReturned)
					close(entered)
					<-release
					snapshot.Phase = Some("forged after cancellation")
					return lateError
				})
				result <- err
			}()
			select {
			case <-entered:
			case <-time.After(3 * time.Second):
				t.Fatal("persistence callback did not start")
			}
			if action == "close" {
				m.client.Close()
			} else {
				cancel()
			}
			select {
			case err := <-result:
				requireCode(t, err, "write_outcome_unknown")
				if err == lateError {
					t.Fatal("operation waited for blocked persistence")
				}
			case <-time.After(3 * time.Second):
				t.Fatal("blocked persistence prevented cancellation")
			}
			if checkpoint.Phase.Value != "reservation_started" || checkpoint.Binding.Value == "" {
				t.Fatalf("latest checkpoint was lost: %#v", checkpoint)
			}
			if len(m.calls) != len(replies) {
				t.Fatal("cancellation dispatched a reservation")
			}
			for _, call := range m.calls {
				if call.Method != "GET" {
					t.Fatal("cancellation dispatched a write")
				}
			}
			retry := newMock(t)
			_, err := retry.client.UploadListingImage(context.Background(), request, png, &checkpoint, func(ListingImageUploadCheckpoint) error { return nil })
			requireCode(t, err, "checkpoint_in_use")
			before, _ := json.Marshal(checkpoint)
			releaseOnce.Do(func() { close(release) })
			select {
			case <-callbackReturned:
			case <-time.After(3 * time.Second):
				t.Fatal("released persistence callback did not return")
			}
			after, _ := json.Marshal(checkpoint)
			if !bytes.Equal(before, after) {
				t.Fatal("late persistence callback changed returned checkpoint")
			}
			// callbackReturned closes inside the user function. Its wrapper must
			// then release the lease; retries remain local until that happens.
			deadline := time.Now().Add(3 * time.Second)
			for {
				_, err := retry.client.UploadListingImage(context.Background(), request, png, &checkpoint, func(ListingImageUploadCheckpoint) error { return nil })
				var api *APIError
				if errors.As(err, &api) && api.Code == "checkpoint_in_use" {
					if time.Now().After(deadline) {
						t.Fatal("checkpoint remained busy after callback finished")
					}
					time.Sleep(time.Millisecond)
					continue
				}
				requireCode(t, err, "write_outcome_unknown")
				break
			}
			if len(retry.calls) != 0 {
				t.Fatal("uncertain reservation retry dispatched a write")
			}

		})
	}
}

func TestCheckpointCallbackPanicPropagates(t *testing.T) {
	request, png, replies := checkpointPreflight(t)
	m := newMock(t, replies...)
	checkpoint := ListingImageUploadCheckpoint{}
	failure := errors.New("callback panic")
	defer func() {
		if value := recover(); value != failure {
			t.Fatalf("callback panic changed: %v", value)
		}
		if checkpoint.Phase.Value != "reservation_started" || len(m.calls) != len(replies) {
			t.Fatal("panic lost checkpoint or dispatched write")
		}
	}()
	m.client.UploadListingImage(context.Background(), request, png, &checkpoint, func(ListingImageUploadCheckpoint) error { panic(failure) })
	t.Fatal("callback panic was swallowed")
}
