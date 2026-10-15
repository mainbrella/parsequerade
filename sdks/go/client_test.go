package parsequerade

import (
	"bytes"
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"encoding/json"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"math/big"
	"net/http"
	"os"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"
)

type roundTripFunc func(*http.Request) (*http.Response, error)

func (f roundTripFunc) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type fixture struct {
	Handler    string           `json:"handler"`
	Collection bool             `json:"collection"`
	Path       string           `json:"path"`
	Item       map[string]any   `json:"item"`
	Responses  []map[string]any `json:"responses"`
}

func fixtures(t *testing.T) []fixture {
	t.Helper()
	data, err := os.ReadFile("testdata/reads.json")
	if err != nil {
		t.Fatal(err)
	}
	var values []fixture
	if err := json.Unmarshal(data, &values); err != nil {
		t.Fatal(err)
	}
	return values
}

func fixtureFor(t *testing.T, handler string) fixture {
	t.Helper()
	for _, f := range fixtures(t) {
		if f.Handler == handler {
			return f
		}
	}
	t.Fatalf("missing fixture %s", handler)
	return fixture{}
}

func testKey(t *testing.T, curve elliptic.Curve) (*ecdsa.PrivateKey, []byte) {
	t.Helper()
	key, err := ecdsa.GenerateKey(curve, rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	der, err := x509.MarshalPKCS8PrivateKey(key)
	if err != nil {
		t.Fatal(err)
	}
	return key, pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: der})
}

func reply(value any, status int, headers ...string) *http.Response {
	data, _ := json.Marshal(value)
	h := http.Header{"Content-Type": {"application/json"}}
	for i := 0; i < len(headers); i += 2 {
		h.Set(headers[i], headers[i+1])
	}
	return &http.Response{StatusCode: status, Header: h, Body: io.NopCloser(bytes.NewReader(data))}
}

func envelope(data any, next any) map[string]any {
	return map[string]any{"data": data, "links": map[string]any{"next": next}}
}

func item(kind, id string, attrs map[string]any) map[string]any {
	return map[string]any{"type": kind, "id": id, "attributes": attrs}
}

type call struct {
	Method, URL, Token string
	Body               []byte
}

type mock struct {
	client *Client
	calls  []call
}

func newMock(t *testing.T, replies ...any) *mock {
	t.Helper()
	key, privateKey := testKey(t, elliptic.P256())
	m := &mock{}
	var mu sync.Mutex
	transport := roundTripFunc(func(request *http.Request) (*http.Response, error) {
		mu.Lock()
		defer mu.Unlock()
		if request.URL.Scheme+"://"+request.URL.Host != origin {
			t.Fatalf("untrusted URL %s", request.URL)
		}
		deadline, ok := request.Context().Deadline()
		if !ok || time.Until(deadline) <= 0 || time.Until(deadline) > 8*time.Second {
			t.Fatal("missing bounded attempt deadline")
		}
		token := strings.TrimPrefix(request.Header.Get("Authorization"), "Bearer ")
		parts := strings.Split(token, ".")
		if len(parts) != 3 {
			t.Fatal("invalid JWT")
		}
		headerData, _ := base64url.DecodeString(parts[0])
		claimsData, _ := base64url.DecodeString(parts[1])
		signature, _ := base64url.DecodeString(parts[2])
		var header, claims map[string]any
		json.Unmarshal(headerData, &header)
		json.Unmarshal(claimsData, &claims)
		if header["alg"] != "ES256" || header["kid"] != "key" || claims["iss"] != "issuer" || claims["aud"] != "appstoreconnect-v1" || claims["exp"].(float64) <= float64(time.Now().Unix()) || claims["exp"].(float64) > float64(time.Now().Unix()+600) {
			t.Fatal("invalid JWT header or claims")
		}
		hash := sha256.Sum256([]byte(parts[0] + "." + parts[1]))
		if len(signature) != 64 || !ecdsa.Verify(&key.PublicKey, hash[:], new(big.Int).SetBytes(signature[:32]), new(big.Int).SetBytes(signature[32:])) {
			t.Fatal("invalid JWT signature")
		}
		var body []byte
		if request.Body != nil {
			body, _ = io.ReadAll(request.Body)
		}
		m.calls = append(m.calls, call{request.Method, request.URL.String(), token, body})
		if len(replies) == 0 {
			t.Fatalf("unexpected request %s", request.URL)
		}
		value := replies[0]
		replies = replies[1:]
		if f, ok := value.(func() any); ok {
			value = f()
		}
		if err, ok := value.(error); ok {
			return nil, err
		}
		if response, ok := value.(*http.Response); ok {
			return response, nil
		}
		return reply(value, 200), nil
	})
	var err error
	m.client, err = NewClient(Credentials{IssuerID: "issuer", KeyID: "key", PrivateKey: privateKey}, &Options{Transport: transport})
	if err != nil {
		t.Fatal(err)
	}
	return m
}

func requireCode(t *testing.T, err error, code string) *APIError {
	t.Helper()
	var api *APIError
	if !errors.As(err, &api) || api.Code != code {
		t.Fatalf("want %s, got %v", code, err)
	}
	return api
}

func TestAllTwelveGeneratedMethods(t *testing.T) {
	for _, f := range fixtures(t) {
		t.Run(f.Handler, func(t *testing.T) {
			replies := make([]any, len(f.Responses))
			for i, response := range f.Responses {
				replies[i] = response
			}
			m := newMock(t, replies...)
			name := strings.ToUpper(f.Handler[:1]) + f.Handler[1:]
			method := reflect.ValueOf(m.client).MethodByName(name)
			args := []reflect.Value{reflect.ValueOf(context.Background())}
			for _, segment := range strings.Split(f.Path, "/") {
				if segment == "123" || segment == "info_1" || segment == "version_1" || segment == "en-US" {
					args = append(args, reflect.ValueOf(segment))
				}
			}
			args = append(args, reflect.Zero(method.Type().In(method.Type().NumIn()-1)))
			result := method.Call(args)
			if !result[1].IsNil() {
				t.Fatal(result[1].Interface())
			}
			data, err := json.Marshal(result[0].Interface())
			if err != nil {
				t.Fatal(err)
			}
			var flat map[string]any
			json.Unmarshal(data, &flat)
			if f.Collection {
				flat = flat["items"].([]any)[0].(map[string]any)
			}
			if flat["id"] != f.Item["id"] || len(m.calls) != len(f.Responses) {
				t.Fatalf("unexpected result or call count: %s, %d", data, len(m.calls))
			}
			for _, excluded := range []string{"attributes", "relationships", "notes", "demoAccountPassword"} {
				if _, exists := flat[excluded]; exists {
					t.Fatalf("leaked field %s", excluded)
				}
			}
			r := routeByHandler(f.Handler)
			for key, value := range f.Item["attributes"].(map[string]any) {
				if _, allowed := r.Attributes[key]; allowed && !reflect.DeepEqual(flat[key], value) {
					t.Fatalf("field %s changed: %#v != %#v", key, flat[key], value)
				}
			}
		})
	}
}

func TestQueriesProjectionAndOptionalValues(t *testing.T) {
	m := newMock(t, envelope([]any{item("apps", "123", map[string]any{"name": "Notes", "isOrEverWasMadeForKids": false, "subscriptionStatusUrl": nil})}, nil))
	page, err := m.client.Apps(context.Background(), &AppsQuery{BundleID: []string{"one", "two"}, Fields: []string{"isOrEverWasMadeForKids", "subscriptionStatusUrl"}, Limit: 2})
	if err != nil {
		t.Fatal(err)
	}
	app := page.Items[0]
	if app.Name.Present || !app.IsOrEverWasMadeForKids.Present || app.IsOrEverWasMadeForKids.Value || app.IsOrEverWasMadeForKids.Null || !app.SubscriptionStatusURL.Null || !app.SubscriptionStatusURL.Present {
		t.Fatalf("incorrect optional fields: %#v", app)
	}
	if !strings.Contains(m.calls[0].URL, "filter%5BbundleId%5D=one%2Ctwo") {
		t.Fatal("query was not translated")
	}
	data, _ := json.Marshal(app)
	if string(data) != `{"id":"123","isOrEverWasMadeForKids":false,"subscriptionStatusUrl":null}` {
		t.Fatalf("roundtrip changed absent/null/false: %s", data)
	}
	info := item("appInfos", "info_1", map[string]any{})
	info["relationships"] = map[string]any{"primaryCategory": map[string]any{"data": map[string]any{"id": "GAMES", "type": "appCategories"}}, "secondaryCategory": map[string]any{"data": nil}}
	m = newMock(t, envelope([]any{info}, nil))
	infos, err := m.client.AppInfos(context.Background(), "123", &AppInfosQuery{Fields: []string{"state"}})
	if err != nil || infos.Items[0].PrimaryCategoryID.Value != "GAMES" || !infos.Items[0].SecondaryCategoryID.Null {
		t.Fatalf("category flattening: %#v, %v", infos, err)
	}
	if !strings.Contains(m.calls[0].URL, "include=primaryCategory%2CsecondaryCategory") {
		t.Fatal("category includes missing")
	}
}

func TestInvalidQueriesAndPathsDoNotDispatch(t *testing.T) {
	m := newMock(t)
	for _, query := range []Query{{"limit": {"0"}}, {"limit": {"01"}}, {"limit": {"true"}}, {"limit": {"201"}}, {"limit": {"1", "2"}}, {"fields": {"secret"}}, {"fields": {""}}, {"unknown": {"yes"}}, {"versionPlatform": {"FUTURE"}}} {
		_, err := m.client.Get(context.Background(), "/apps", query)
		requireCode(t, err, "invalid_query")
	}
	for _, id := range []string{"../123", "a/b", "a?b", "a#b", "", "é", "x\n"} {
		_, err := m.client.App(context.Background(), id, nil)
		requireCode(t, err, "invalid_path")
	}
	_, err := m.client.Get(context.Background(), "https://evil.example/apps", nil)
	requireCode(t, err, "route_not_found")
	_, err = m.client.Apps(context.Background(), &AppsQuery{Fields: []string{}})
	requireCode(t, err, "invalid_query")
	if len(m.calls) != 0 {
		t.Fatal("invalid input dispatched")
	}
}

func TestCursorBindingFiltersAndLoops(t *testing.T) {
	first := envelope([]any{item("apps", "123", map[string]any{})}, "/v1/apps?cursor=page2")
	m := newMock(t, first, envelope([]any{item("apps", "456", map[string]any{})}, nil))
	page, err := m.client.Apps(context.Background(), &AppsQuery{BundleID: []string{"notes"}})
	if err != nil || page.NextCursor == nil {
		t.Fatalf("first page: %v", err)
	}
	for _, test := range []struct {
		client *Client
		query  *AppsQuery
	}{
		{newMock(t).client, &AppsQuery{BundleID: []string{"notes"}, Cursor: *page.NextCursor}},
		{m.client, &AppsQuery{Cursor: *page.NextCursor}},
		{m.client, &AppsQuery{BundleID: []string{"notes"}, Cursor: *page.NextCursor + "x"}},
	} {
		_, err := test.client.Apps(context.Background(), test.query)
		requireCode(t, err, "invalid_cursor")
	}
	page, err = m.client.Apps(context.Background(), &AppsQuery{BundleID: []string{"notes"}, Cursor: *page.NextCursor})
	if err != nil || page.NextCursor != nil || page.Items[0].ID != "456" || !strings.Contains(m.calls[1].URL, "filter%5BbundleId%5D=notes") {
		t.Fatalf("continuation lost original query: %#v, %v", page, err)
	}
	m = newMock(t, first, first)
	page, _ = m.client.Apps(context.Background(), nil)
	_, err = m.client.Apps(context.Background(), &AppsQuery{Cursor: *page.NextCursor})
	requireCode(t, err, "upstream_incomplete_result")
}

func TestUntrustedContinuations(t *testing.T) {
	for _, link := range []string{"https://evil.example/v1/apps?cursor=x", "/v1/other?cursor=x", "/v1/apps?cursor=x&cursor=y", "/v1/apps?limit=3&cursor=x", "/v1/apps?cursor=x#fragment", "/v1/apps", "/v1/apps?cursor=x&filter%5Bname%5D=other", "https://user:pass@api.appstoreconnect.apple.com/v1/apps?cursor=x"} {
		t.Run(link, func(t *testing.T) {
			m := newMock(t, envelope([]any{}, link))
			_, err := m.client.Apps(context.Background(), nil)
			requireCode(t, err, "upstream_invalid_response")
			if len(m.calls) != 1 {
				t.Fatal("unsafe continuation dispatched")
			}
		})
	}
}

func TestParentOwnershipAndLocaleUniqueness(t *testing.T) {
	m := newMock(t, envelope([]any{}, nil))
	_, err := m.client.Version(context.Background(), "123", "foreign", nil)
	requireCode(t, err, "resource_not_found")
	f := fixtureFor(t, "versionLocalization")
	duplicate := item("appStoreVersionLocalizations", "locale_2", map[string]any{"locale": "en-US"})
	f.Responses[1]["data"] = append(f.Responses[1]["data"].([]any), duplicate)
	m = newMock(t, f.Responses[0], f.Responses[1])
	_, err = m.client.VersionLocalization(context.Background(), "123", "version_1", "en-US", nil)
	requireCode(t, err, "ambiguous_selection")
	f = fixtureFor(t, "versionLocalization")
	f.Responses[2]["data"].(map[string]any)["attributes"].(map[string]any)["locale"] = "fr-FR"
	m = newMock(t, f.Responses[0], f.Responses[1], f.Responses[2])
	_, err = m.client.VersionLocalization(context.Background(), "123", "version_1", "en-US", &VersionLocalizationQuery{Fields: []string{"description"}})
	requireCode(t, err, "upstream_invalid_response")
	parent := item("appStoreVersions", "version_1", map[string]any{})
	parent["relationships"] = map[string]any{"app": map[string]any{"data": map[string]any{"type": "apps", "id": "foreign"}}}
	m = newMock(t, envelope([]any{parent}, nil))
	_, err = m.client.Version(context.Background(), "123", "version_1", nil)
	requireCode(t, err, "upstream_invalid_response")
}

func TestReadRetriesRefreshAndSanitizedErrors(t *testing.T) {
	m := newMock(t, reply(nil, 401), reply(nil, 503, "Retry-After", "0"), envelope([]any{}, nil))
	_, err := m.client.Apps(context.Background(), nil)
	if err != nil || len(m.calls) != 3 || m.calls[0].Token == m.calls[1].Token || m.calls[1].Token != m.calls[2].Token {
		t.Fatalf("JWT refresh/retry failed: %v", err)
	}
	m = newMock(t, reply(map[string]any{"errors": []any{map[string]any{"code": "BAD.PARAM", "detail": "secret", "source": map[string]any{"parameter": "limit"}}}}, 422))
	_, err = m.client.Apps(context.Background(), nil)
	api := requireCode(t, err, "validation_failed")
	if api.Status != 422 || len(api.Details) != 1 || api.Details[0].Field != "limit" || strings.Contains(fmt.Sprint(api.Details), "secret") {
		t.Fatalf("unsafe or unhelpful error: %#v", api)
	}
	m = newMock(t, reply(nil, 429, "Retry-After", "10"))
	_, err = m.client.Apps(context.Background(), nil)
	api = requireCode(t, err, "rate_limited")
	if api.RetryAfter == nil || *api.RetryAfter != 10 || len(m.calls) != 1 {
		t.Fatal("long throttle delay retried")
	}
}

func TestWriteValidationAndUncertainOutcomes(t *testing.T) {
	for _, attrs := range []AgeRatingUpdate{{}, {Gambling: Null[bool]()}, {KidsAgeBand: Some("bad")}, {DeveloperAgeRatingInfoURL: Some("https://user:pass@example.com")}, {GracRatingClassificationNumber: Some(strings.Repeat("x", 2049))}} {
		m := newMock(t)
		_, err := m.client.UpdateAgeRating(context.Background(), "123", "info_1", attrs)
		requireCode(t, err, "invalid_request")
	}
	f := fixtureFor(t, "ageRating")
	m := newMock(t, f.Responses[0], f.Responses[1], f.Responses[1])
	rating, err := m.client.UpdateAgeRating(context.Background(), "123", "info_1", AgeRatingUpdate{Gambling: Some(false), KidsAgeBand: Null[string]()})
	if err != nil || rating.Gambling.Value || !rating.Gambling.Present || len(m.calls) != 3 || m.calls[2].Method != "PATCH" {
		t.Fatalf("write failed: %#v, %v", rating, err)
	}
	var patch map[string]any
	json.Unmarshal(m.calls[2].Body, &patch)
	attrs := patch["data"].(map[string]any)["attributes"].(map[string]any)
	if !reflect.DeepEqual(attrs, map[string]any{"gambling": false, "kidsAgeBand": nil}) || !strings.HasSuffix(m.calls[2].URL, "/v1/ageRatingDeclarations/rating_1") {
		t.Fatalf("incorrect PATCH: %s", m.calls[2].Body)
	}
	for _, test := range []struct {
		response any
		code     string
	}{
		{errors.New("secret"), "write_outcome_unknown"},
		{reply(nil, 200, "Content-Type", "text/html"), "write_outcome_unknown"},
		{reply(nil, 401), "upstream_authentication_failed"},
		{reply(nil, 429, "Retry-After", "0"), "rate_limited"},
		{reply(nil, 503, "Retry-After", "0"), "upstream_unavailable"},
		{envelope(item("ageRatingDeclarations", "wrong", map[string]any{}), nil), "write_outcome_unknown"},
	} {
		m := newMock(t, f.Responses[0], f.Responses[1], test.response)
		_, err := m.client.UpdateAgeRating(context.Background(), "123", "info_1", AgeRatingUpdate{Gambling: Some(false)})
		api := requireCode(t, err, test.code)
		if api.Retryable || len(m.calls) != 3 {
			t.Fatal("write retried or marked retryable")
		}
	}
}

func TestMalformedResponsesAndBudgets(t *testing.T) {
	app := item("apps", "123", map[string]any{})
	for _, value := range []any{
		reply(nil, 302, "Location", "https://evil.example/"),
		envelope([]any{app, app}, nil), envelope([]any{item("apps", "123", map[string]any{"name": 42})}, nil),
		envelope([]any{item("apps", "123", nil)}, nil), envelope([]any{}, 42),
		&http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader("invalid"))},
		&http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(strings.Repeat("x", responseBytes+1)))},
	} {
		m := newMock(t, value)
		_, err := m.client.Apps(context.Background(), nil)
		requireCode(t, err, "upstream_invalid_response")
	}
	m := newMock(t)
	_, err := m.client.apple(context.Background(), appleURL("/v1/apps", nil), routeByHandler("apps"), &budget{attempts: 32}, nil)
	requireCode(t, err, "upstream_budget_exceeded")
	m = newMock(t, envelope([]any{}, nil))
	_, err = m.client.apple(context.Background(), appleURL("/v1/apps", nil), routeByHandler("apps"), &budget{bytes: totalBytes}, nil)
	requireCode(t, err, "upstream_invalid_response")
	m = newMock(t)
	for _, raw := range []string{"https://evil.example/v1/apps", origin + "/v1/../apps", origin + "/v1/apps#x"} {
		_, err = m.client.apple(context.Background(), raw, routeByHandler("apps"), &budget{}, nil)
		requireCode(t, err, "upstream_invalid_response")
	}
}

func TestResolverBoundAndCursorExpiry(t *testing.T) {
	replies := make([]any, 20)
	for i := range replies {
		replies[i] = envelope([]any{}, fmt.Sprintf("/v1/apps/123/appInfos?cursor=%d", i))
	}
	m := newMock(t, replies...)
	_, err := m.client.AppInfo(context.Background(), "123", "info_1", nil)
	requireCode(t, err, "upstream_incomplete_result")
	if len(m.calls) != 20 {
		t.Fatal("resolver did not stop at 20 pages")
	}
	for _, state := range []cursorState{
		{Binding: "binding", Next: "x", Seen: []string{"a"}, Expires: time.Now().Unix() - 1},
		{Binding: "binding", Next: "x", Seen: []string{"a", "a"}, Expires: time.Now().Unix() + 900},
	} {
		token, _ := m.client.encodeCursor(state)
		_, err := m.client.decodeCursor(token, "binding")
		requireCode(t, err, "invalid_cursor")
	}
}

func TestCancellationAndConcurrency(t *testing.T) {
	m := newMock(t)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, err := m.client.Apps(ctx, nil)
	if !errors.Is(err, context.Canceled) || len(m.calls) != 0 {
		t.Fatalf("cancelled read dispatched: %v", err)
	}
	entered, release := make(chan struct{}, 4), make(chan struct{})
	_, privateKey := testKey(t, elliptic.P256())
	c, err := NewClient(Credentials{IssuerID: "issuer", KeyID: "key", PrivateKey: privateKey}, &Options{Transport: roundTripFunc(func(r *http.Request) (*http.Response, error) {
		entered <- struct{}{}
		select {
		case <-release:
			return reply(envelope([]any{}, nil), 200), nil
		case <-r.Context().Done():
			return nil, r.Context().Err()
		}
	})})
	if err != nil {
		t.Fatal(err)
	}
	results := make(chan error, 4)
	for i := 0; i < 4; i++ {
		go func() { _, err := c.Apps(context.Background(), nil); results <- err }()
	}
	for i := 0; i < 4; i++ {
		select {
		case <-entered:
		case <-time.After(5 * time.Second):
			t.Fatal("concurrent request failed to enter")
		}
	}
	_, err = c.Apps(context.Background(), nil)
	api := requireCode(t, err, "rate_limited")
	if api.RetryAfter == nil || *api.RetryAfter != 1 {
		t.Fatal("missing local throttle delay")
	}
	close(release)
	for i := 0; i < 4; i++ {
		if err := <-results; err != nil {
			t.Fatal(err)
		}
	}
}

func TestCancellationAfterDispatch(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	m := newMock(t, func() any {
		cancel()
		return envelope([]any{}, nil)
	})
	_, err := m.client.Apps(ctx, nil)
	if !errors.Is(err, context.Canceled) || len(m.calls) != 1 {
		t.Fatalf("cancelled read: %v, calls=%d", err, len(m.calls))
	}

	ctx, cancelWrite := context.WithCancel(context.Background())
	defer cancelWrite()
	f := fixtureFor(t, "ageRating")
	m = newMock(t, f.Responses[0], f.Responses[1], func() any {
		cancelWrite()
		return errors.New("private transport details")
	})
	_, err = m.client.UpdateAgeRating(ctx, "123", "info_1", AgeRatingUpdate{Gambling: Some(false)})
	api := requireCode(t, err, "write_outcome_unknown")
	if api.Retryable || len(m.calls) != 3 {
		t.Fatal("cancelled write retried or marked retryable")
	}
}

func TestCredentialsValidatedLocally(t *testing.T) {
	_, p384 := testKey(t, elliptic.P384())
	key, valid := testKey(t, elliptic.P256())
	sec1, _ := x509.MarshalECPrivateKey(key)
	for _, privateKey := range [][]byte{[]byte("invalid"), p384, pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: sec1}), append(valid, valid...)} {
		_, err := NewClient(Credentials{IssuerID: "issuer", KeyID: "key", PrivateKey: privateKey}, nil)
		requireCode(t, err, "invalid_credentials")
	}
	_, err := NewClient(Credentials{KeyID: "key", PrivateKey: valid}, nil)
	requireCode(t, err, "invalid_credentials")
}
