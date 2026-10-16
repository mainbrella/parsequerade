package parsequerade

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"regexp"
	"runtime"
	"sort"
	"sync"
)

// Client is safe for concurrent use. Reuse it to share the Rust core's JWT cache,
// cursor signing, and concurrency bound. Do not copy a Client. Call Close when done.
type Client struct {
	mu        sync.Mutex
	handle    uint64
	active    map[uint64]context.CancelFunc
	transport http.RoundTripper
}

// NewClient validates a P-256 PKCS#8 team key locally without a network request.
func NewClient(credentials Credentials, options *Options) (*Client, error) {
	config, err := json.Marshal(map[string]any{
		"issuerId": credentials.IssuerID, "keyId": credentials.KeyID,
		"privateKey": string(credentials.PrivateKey), "customTransport": true,
	})
	if err != nil {
		return nil, err
	}
	var result nativeResult
	if err = decodeNative(nativeNew(config), &result); err != nil {
		return nil, err
	}
	transport := http.DefaultTransport
	if options != nil && options.Transport != nil {
		transport = options.Transport
	}
	client := &Client{handle: result.Client, active: map[uint64]context.CancelFunc{}, transport: transport}
	runtime.SetFinalizer(client, (*Client).Close)
	return client, nil
}

// Close releases the client and cancels its active operations. It is idempotent
// and may be called concurrently, including from a checkpoint callback.
func (c *Client) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.handle != 0 {
		handle := c.handle
		c.handle = 0
		for operation, cancel := range c.active {
			cancel()
			nativeCancel(operation)
		}
		nativeClientFree(handle)
		runtime.SetFinalizer(c, nil)
	}
	return nil
}

// Get reads an implemented relative facade path and returns flat JSON.
func (c *Client) Get(ctx context.Context, path string, query Query) (json.RawMessage, error) {
	pairs := make([][2]string, 0, len(query))
	names := make([]string, 0, len(query))
	for name := range query {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		values := query[name]
		if len(values) == 0 {
			return nil, invalidQuery(name)
		}
		for _, value := range values {
			pairs = append(pairs, [2]string{name, value})
		}
	}
	return c.invoke(ctx, "get", []any{path, pairs}, nil, nil, nil)
}

func readTyped[T any](c *Client, ctx context.Context, path string, query Query) (T, error) {
	var value T
	raw, err := c.Get(ctx, path, query)
	if err == nil && json.Unmarshal(raw, &value) != nil {
		err = invalidUpstream()
	}
	return value, err
}

func operationTyped[T any](c *Client, ctx context.Context, operation string, args []any) (T, error) {
	var value T
	raw, err := c.invoke(ctx, operation, args, nil, nil, nil)
	if err == nil && json.Unmarshal(raw, &value) != nil {
		err = invalidUpstream()
	}
	return value, err
}

// UpdateAgeRating validates the attributes, proves ownership, and sends one PATCH.
func (c *Client) UpdateAgeRating(ctx context.Context, appID, appInfoID string, attributes AgeRatingUpdate) (AgeRating, error) {
	return operationTyped[AgeRating](c, ctx, "update_age_rating", []any{appID, appInfoID, attributes})
}

// UpdateMetadata updates supported app, version, or exact-localization attributes.
func (c *Client) UpdateMetadata(ctx context.Context, path string, attributes map[string]any) (json.RawMessage, error) {
	return c.invoke(ctx, "update_metadata", []any{path, attributes}, nil, nil, nil)
}

// CreateVersion creates one unsubmitted version. Uncertain writes are not retried.
func (c *Client) CreateVersion(ctx context.Context, appID, platform, versionString string) (Version, error) {
	return operationTyped[Version](c, ctx, "create_version", []any{appID, platform, versionString})
}

// AttachBuild validates an eligible build, attaches it to its matching draft, and
// reads back the version to verify the attachment.
func (c *Client) AttachBuild(ctx context.Context, appID, versionID, buildID string) (Version, error) {
	return operationTyped[Version](c, ctx, "attach_build", []any{appID, versionID, buildID})
}

// ListingImageReferenceData reads Apple's current placement and asset specifications.
func (c *Client) ListingImageReferenceData(ctx context.Context) (json.RawMessage, error) {
	return c.invoke(ctx, "listing_image_reference_data", []any{}, nil, nil, nil)
}

var checkpointGuards = struct {
	sync.Mutex
	active map[*ListingImageUploadCheckpoint]*checkpointLease
}{active: map[*ListingImageUploadCheckpoint]*checkpointLease{}}

// A cancelled operation can return before persistence finishes. Keep its
// checkpoint busy until both lifetimes end so a late durable save cannot race
// a resumed upload using the same checkpoint.
type checkpointLease struct {
	checkpoint *ListingImageUploadCheckpoint
	finished   bool
	pending    int
}

func (lease *checkpointLease) finishOperation() {
	checkpointGuards.Lock()
	defer checkpointGuards.Unlock()
	lease.finished = true
	if lease.pending == 0 && checkpointGuards.active[lease.checkpoint] == lease {
		delete(checkpointGuards.active, lease.checkpoint)
	}
}

func retainCheckpointCallback(checkpoint *ListingImageUploadCheckpoint) *checkpointLease {
	checkpointGuards.Lock()
	defer checkpointGuards.Unlock()
	lease := checkpointGuards.active[checkpoint]
	if lease != nil {
		lease.pending++
	}
	return lease
}

func (lease *checkpointLease) finishCallback() {
	if lease == nil {
		return
	}
	checkpointGuards.Lock()
	defer checkpointGuards.Unlock()
	lease.pending--
	if lease.finished && lease.pending == 0 && checkpointGuards.active[lease.checkpoint] == lease {
		delete(checkpointGuards.active, lease.checkpoint)
	}
}

// UploadListingImage uploads or resumes one PNG. Each independent snapshot must
// be durably saved by persist before the next write. The checkpoint is updated
// after each snapshot and on failure. Reuse it and the same PNG to reconcile an
// uncertain write. Concurrent uploads must use distinct checkpoint pointers.
func (c *Client) UploadListingImage(ctx context.Context, request ListingImageUpload, png []byte, checkpoint *ListingImageUploadCheckpoint, persist func(ListingImageUploadCheckpoint) error) (ListingImageUploadReceipt, error) {
	var receipt ListingImageUploadReceipt
	if checkpoint == nil || persist == nil {
		return receipt, apiError(400, "invalid_request", "Provide a checkpoint and durable persistence callback.")
	}
	checkpointGuards.Lock()
	if checkpointGuards.active[checkpoint] != nil {
		checkpointGuards.Unlock()
		return receipt, apiError(409, "checkpoint_in_use", "This checkpoint already has an upload in progress.")
	}
	lease := &checkpointLease{checkpoint: checkpoint}
	checkpointGuards.active[checkpoint] = lease
	checkpointGuards.Unlock()
	defer lease.finishOperation()
	raw, err := c.invoke(ctx, "upload_listing_image", []any{request, *checkpoint}, png, checkpoint, persist)
	if err == nil && json.Unmarshal(raw, &receipt) != nil {
		err = invalidUpstream()
	}
	return receipt, err
}

type nativeResult struct {
	Client           uint64            `json:"client"`
	Operation        uint64            `json:"operation"`
	Kind             string            `json:"kind"`
	ID               uint64            `json:"id"`
	Method           string            `json:"method"`
	URL              string            `json:"url"`
	Headers          map[string]string `json:"headers"`
	Body             *string           `json:"body"`
	TimeoutMS        uint64            `json:"timeoutMs"`
	DeadlineMS       int64             `json:"deadlineMs"`
	MaxResponseBytes int64             `json:"maxResponseBytes"`
	Value            json.RawMessage   `json:"value"`
	Error            *APIError         `json:"error"`
	Checkpoint       json.RawMessage   `json:"checkpoint"`
}

func decodeNative(raw []byte, result *nativeResult) error {
	if len(raw) == 0 || json.Unmarshal(raw, result) != nil {
		return apiError(500, "native_bridge_failed", "The native SDK bridge returned an invalid response.")
	}
	if result.Error != nil {
		return result.Error
	}
	return nil
}

func (c *Client) invoke(ctx context.Context, operation string, args []any, png []byte, checkpoint *ListingImageUploadCheckpoint, persist func(ListingImageUploadCheckpoint) error) (json.RawMessage, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	request := map[string]any{"operation": operation, "args": args}
	if operation == "upload_listing_image" {
		request["png"] = base64.StdEncoding.EncodeToString(png)
	}
	raw, err := json.Marshal(request)
	if err != nil {
		return nil, apiError(400, "invalid_request", "Provide a JSON-compatible request.")
	}
	opctx, cancel := context.WithCancel(ctx)
	defer cancel()
	c.mu.Lock()
	if err := opctx.Err(); err != nil {
		c.mu.Unlock()
		return nil, err
	}
	if c.handle == 0 {
		c.mu.Unlock()
		return nil, apiError(400, "client_closed", "The client is closed.")
	}
	var started nativeResult
	err = decodeNative(nativeStart(c.handle, raw), &started)
	if err != nil {
		c.mu.Unlock()
		return nil, err
	}
	id := started.Operation
	c.active[id] = cancel
	c.mu.Unlock()
	finished, watchDone := make(chan struct{}), make(chan struct{})
	go func() {
		defer close(watchDone)
		select {
		case <-opctx.Done():
			nativeCancel(id)
		case <-finished:
		}
	}()
	defer func() {
		close(finished)
		<-watchDone
		c.mu.Lock()
		delete(c.active, id)
		nativeOperationFree(id)
		c.mu.Unlock()
		runtime.KeepAlive(c)
	}()
	var callbackError error
	for {
		if opctx.Err() != nil {
			nativeCancel(id)
		}
		encoded := nativePoll(id, 25)
		if string(encoded) == "null" {
			continue
		}
		var event nativeResult
		if err := decodeNative(encoded, &event); err != nil && event.Kind != "result" {
			return nil, err
		}
		switch event.Kind {
		case "http":
			response := map[string]any{"error": "failed"}
			if opctx.Err() == nil {
				response = c.send(opctx, event)
			}
			nativeRespond(id, event.ID, response)
		case "checkpoint":
			response := map[string]any{"ok": true}
			var snapshot ListingImageUploadCheckpoint
			if checkpoint == nil || json.Unmarshal(event.Checkpoint, &snapshot) != nil {
				response = map[string]any{"error": "checkpoint_persistence_failed"}
			} else if opctx.Err() != nil {
				*checkpoint = snapshot
				nativeCancel(id)
				response = map[string]any{"error": "checkpoint_persistence_failed"}
			} else {
				*checkpoint = snapshot
				// Decode independently so future nested checkpoint fields cannot be
				// shared between a detached callback and the caller's live state.
				var callbackSnapshot ListingImageUploadCheckpoint
				if json.Unmarshal(event.Checkpoint, &callbackSnapshot) != nil {
					response = map[string]any{"error": "checkpoint_persistence_failed"}
				} else {
					saved := make(chan checkpointSaveResult, 1)
					lease := retainCheckpointCallback(checkpoint)
					go saveCheckpoint(callbackSnapshot, persist, saved, lease)
					select {
					case result := <-saved:
						if result.panicked {
							panic(result.panicValue)
						}
						if result.err != nil {
							if callbackError == nil {
								callbackError = result.err
							}
							response = map[string]any{"error": "checkpoint_persistence_failed"}
						}
					case <-opctx.Done():
						nativeCancel(id)
						response = map[string]any{"error": "checkpoint_persistence_failed"}
					}
				}
			}
			nativeRespond(id, event.ID, response)
		case "result":
			if checkpoint != nil && len(event.Checkpoint) != 0 {
				var snapshot ListingImageUploadCheckpoint
				if err := json.Unmarshal(event.Checkpoint, &snapshot); err != nil {
					return nil, invalidUpstream()
				}
				*checkpoint = snapshot
			}
			if callbackError != nil {
				return nil, callbackError
			}
			if event.Error != nil {
				if event.Error.Code == "operation_cancelled" && opctx.Err() != nil {
					return nil, opctx.Err()
				}
				return nil, event.Error
			}
			return event.Value, nil
		default:
			return nil, apiError(500, "native_bridge_failed", "The native SDK bridge returned an invalid event.")
		}
	}
}

type checkpointSaveResult struct {
	err        error
	panicked   bool
	panicValue any
}

// A blocked callback must not prevent cancellation. Its single buffered result
// cannot mutate operation state after the caller has stopped waiting.
func saveCheckpoint(snapshot ListingImageUploadCheckpoint, persist func(ListingImageUploadCheckpoint) error, saved chan<- checkpointSaveResult, lease *checkpointLease) {
	result := checkpointSaveResult{panicked: true}
	defer func() {
		result.panicValue = recover()
		lease.finishCallback()
		saved <- result
	}()
	result.err = persist(snapshot)
	result.panicked = false
}

var idPattern = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$`)
var localePattern = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9-]{1,34}$`)

func validateSegment(value string, locale bool) error {
	pattern := idPattern
	if locale {
		pattern = localePattern
	}
	if !pattern.MatchString(value) {
		return apiError(400, "invalid_path", "The resource path is invalid.")
	}
	return nil
}
