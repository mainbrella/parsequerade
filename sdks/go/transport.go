package parsequerade

import (
	"bytes"
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"encoding/pem"
	"errors"
	"io"
	"math"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"
)

const origin = "https://api.appstoreconnect.apple.com"
const responseBytes = 4 * 1024 * 1024
const totalBytes = 16 * 1024 * 1024

var base64url = base64.RawURLEncoding.Strict()

type auth struct {
	mu        sync.Mutex
	key       *ecdsa.PrivateKey
	issuerID  string
	keyID     string
	cached    string
	expiresAt time.Time
}

func newAuth(credentials Credentials) (*auth, error) {
	invalid := apiError(400, "invalid_credentials", "Provide an issuer ID, key ID and a P-256 PKCS#8 private key.")
	if credentials.IssuerID == "" || len(credentials.IssuerID) > 128 || credentials.KeyID == "" || len(credentials.KeyID) > 128 {
		return nil, invalid
	}
	trimmed := bytes.TrimSpace(credentials.PrivateKey)
	if !bytes.HasPrefix(trimmed, []byte("-----BEGIN PRIVATE KEY-----")) {
		return nil, invalid
	}
	block, rest := pem.Decode(trimmed)
	if block == nil || block.Type != "PRIVATE KEY" || len(block.Headers) != 0 || len(bytes.TrimSpace(rest)) != 0 {
		return nil, invalid
	}
	key, err := x509.ParsePKCS8PrivateKey(block.Bytes)
	if err != nil {
		return nil, invalid
	}
	ec, ok := key.(*ecdsa.PrivateKey)
	if !ok || ec.Curve != elliptic.P256() {
		return nil, invalid
	}
	return &auth{key: ec, issuerID: credentials.IssuerID, keyID: credentials.KeyID}, nil
}

func (a *auth) token() (string, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	now := time.Now()
	if a.cached != "" && a.expiresAt.After(now.Add(time.Minute)) {
		return a.cached, nil
	}
	header, _ := json.Marshal(map[string]any{"alg": "ES256", "typ": "JWT", "kid": a.keyID})
	payload, _ := json.Marshal(map[string]any{"iss": a.issuerID, "iat": now.Unix() - 30, "exp": now.Unix() + 600, "aud": "appstoreconnect-v1"})
	content := base64url.EncodeToString(header) + "." + base64url.EncodeToString(payload)
	digest := sha256.Sum256([]byte(content))
	r, s, err := ecdsa.Sign(rand.Reader, a.key, digest[:])
	if err != nil {
		return "", apiError(500, "signing_failed", "The Apple request could not be signed.")
	}
	signature := make([]byte, 64)
	r.FillBytes(signature[:32])
	s.FillBytes(signature[32:])
	a.cached = content + "." + base64url.EncodeToString(signature)
	a.expiresAt = now.Add(10 * time.Minute)
	return a.cached, nil
}

func (a *auth) invalidate(rejected string) {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.cached == rejected {
		a.cached = ""
	}
}

type budget struct {
	attempts int
	bytes    int
}

func upstreamError(status int, payload map[string]any, headers http.Header, r route) *APIError {
	e := apiError(502, "upstream_unavailable", "Apple could not complete this request.")
	switch status {
	case 401:
		e = apiError(502, "upstream_authentication_failed", "Apple rejected the configured credentials.")
	case 403:
		e = apiError(403, "upstream_forbidden", "The Apple key cannot access this resource.")
	case 404:
		e = notFound("resource_not_found")
	case 409:
		e = apiError(409, "upstream_conflict", "Apple reported a resource conflict.")
	case 400, 422:
		e = apiError(422, "validation_failed", "Apple rejected one or more request parameters.")
	case 429:
		e = apiError(429, "rate_limited", "Apple is throttling this connection.")
	default:
		if status >= 500 {
			e.Status = 503
		}
	}
	e.Retryable = status == 429 || status >= 500
	raw := headers.Get("Retry-After")
	seconds, err := strconv.Atoi(raw)
	if err != nil || seconds < 0 {
		if date, dateErr := http.ParseTime(raw); dateErr == nil {
			seconds, err = int(math.Ceil(time.Until(date).Seconds())), nil
			if seconds < 0 {
				seconds = 0
			}
		} else {
			err = dateErr
		}
	}
	if err == nil {
		if seconds > 86400 {
			seconds = 86400
		}
		e.RetryAfter = &seconds
	}
	items, _ := payload["errors"].([]any)
	for i, item := range items {
		if i >= 32 {
			break
		}
		value, ok := item.(map[string]any)
		if !ok {
			continue
		}
		code, _ := value["code"].(string)
		if !safeCodePattern.MatchString(code) {
			code = "upstream_error"
		}
		detail := ErrorDetail{Code: code, Message: e.Message}
		source, _ := value["source"].(map[string]any)
		for _, p := range r.Parameters {
			if p.AppleName != "" && source["parameter"] == p.AppleName {
				detail.Field = p.Name
			}
		}
		e.Details = append(e.Details, detail)
	}
	return e
}

func validAppleURL(raw string) bool {
	u, err := url.Parse(raw)
	if err != nil || u.Scheme+"://"+u.Host != origin || u.User != nil || u.Fragment != "" || !strings.HasPrefix(u.Path, "/v1/") || u.RawPath != "" {
		return false
	}
	for _, c := range u.Path {
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || strings.ContainsRune("/_-", c)) {
			return false
		}
	}
	return true
}

func (c *Client) attempt(ctx context.Context, rawURL, token string, body []byte, r route, b *budget) (map[string]any, *APIError, bool) {
	write := body != nil
	method := http.MethodGet
	if write {
		method = http.MethodPatch
	}
	request, err := http.NewRequestWithContext(ctx, method, rawURL, bytes.NewReader(body))
	if err != nil {
		return nil, invalidUpstream(), false
	}
	request.Header.Set("Authorization", "Bearer "+token)
	request.Header.Set("Accept", "application/json")
	request.Header.Set("User-Agent", "parsequerade-go/0.1.0")
	if write {
		request.Header.Set("Content-Type", "application/json")
	}
	response, err := c.http.Do(request)
	if err != nil {
		if write {
			return nil, unknownWrite(), false
		}
		e := apiError(503, "upstream_unavailable", "The Apple read could not complete.")
		if errors.Is(err, context.DeadlineExceeded) || ctx.Err() != nil {
			e = apiError(504, "upstream_timeout", "The Apple read could not complete.")
		}
		e.Retryable = true
		return nil, e, false
	}
	defer response.Body.Close()
	data, err := io.ReadAll(io.LimitReader(response.Body, responseBytes+1))
	b.bytes += len(data)
	if err != nil || ctx.Err() != nil {
		if write {
			return nil, unknownWrite(), false
		}
		e := apiError(503, "upstream_unavailable", "The Apple read could not complete.")
		e.Retryable = true
		return nil, e, false
	}
	if len(data) > responseBytes || b.bytes > totalBytes || response.StatusCode >= 300 && response.StatusCode < 400 {
		if write {
			return nil, unknownWrite(), false
		}
		return nil, invalidUpstream(), false
	}
	var payload map[string]any
	decodeErr := json.Unmarshal(data, &payload)
	if response.StatusCode >= 200 && response.StatusCode < 300 {
		mime := strings.ToLower(strings.TrimSpace(strings.Split(response.Header.Get("Content-Type"), ";")[0]))
		_, hasData := payload["data"]
		if decodeErr != nil || !hasData || !(mime == "application/json" || strings.HasPrefix(mime, "application/") && strings.HasSuffix(mime, "+json")) {
			if write {
				return nil, unknownWrite(), false
			}
			return nil, invalidUpstream(), false
		}
		return payload, nil, false
	}
	e := upstreamError(response.StatusCode, payload, response.Header, r)
	if write {
		e.Retryable = false
	}
	return nil, e, response.StatusCode == 401
}

func (c *Client) apple(ctx context.Context, rawURL string, r route, b *budget, body []byte) (map[string]any, error) {
	if !validAppleURL(rawURL) {
		return nil, invalidUpstream()
	}
	write := body != nil
	select {
	case c.permits <- struct{}{}:
		defer func() { <-c.permits }()
	default:
		e := apiError(429, "rate_limited", "This client has too many concurrent Apple requests.")
		seconds := 1
		e.Retryable, e.RetryAfter = !write, &seconds
		return nil, e
	}
	refreshed := false
	for attempt := 0; attempt < 3; attempt++ {
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		if b.attempts >= 32 {
			e := apiError(504, "upstream_budget_exceeded", "The bounded Apple operation could not finish.")
			e.Retryable = !write
			return nil, e
		}
		b.attempts++
		token, err := c.auth.token()
		if err != nil {
			return nil, err
		}
		attemptCtx, cancel := context.WithTimeout(ctx, 8*time.Second)
		payload, failure, rejected := c.attempt(attemptCtx, rawURL, token, body, r, b)
		cancel()
		if failure == nil {
			return payload, nil
		}
		// After dispatch a cancelled write has an uncertain outcome, even when
		// the caller's context is cancelled. Reads retain Go context errors.
		if write {
			return nil, failure
		}
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		retryAuth := rejected && !refreshed
		if retryAuth {
			c.auth.invalidate(token)
			refreshed = true
		}
		if attempt == 2 || !failure.Retryable && !retryAuth {
			return nil, failure
		}
		delay := time.Duration(200*(1<<attempt)) * time.Millisecond
		if retryAuth {
			delay = 0
		} else if failure.RetryAfter != nil {
			delay = time.Duration(*failure.RetryAfter) * time.Second
		}
		deadline, _ := ctx.Deadline()
		if delay > 2*time.Second || !deadline.IsZero() && time.Now().Add(delay).After(deadline) {
			return nil, failure
		}
		timer := time.NewTimer(delay)
		select {
		case <-ctx.Done():
			timer.Stop()
			return nil, ctx.Err()
		case <-timer.C:
		}
	}
	return nil, invalidUpstream()
}
