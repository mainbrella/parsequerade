package parsequerade

import (
	"context"
	"crypto/rand"
	"encoding/json"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// Client is safe for concurrent use. Reuse it to share JWT caching, cursor
// signing, and the four-request concurrency bound. Do not copy a Client.
type Client struct {
	auth      *auth
	http      *http.Client
	permits   chan struct{}
	cursorKey [32]byte
}

// NewClient validates credentials locally without making a network request.
func NewClient(credentials Credentials, options *Options) (*Client, error) {
	a, err := newAuth(credentials)
	if err != nil {
		return nil, err
	}
	var transport http.RoundTripper
	if options != nil {
		transport = options.Transport
	}
	c := &Client{
		auth:    a,
		permits: make(chan struct{}, 4),
		http: &http.Client{
			Transport: transport,
			CheckRedirect: func(*http.Request, []*http.Request) error {
				return http.ErrUseLastResponse
			},
		},
	}
	if _, err := rand.Read(c.cursorKey[:]); err != nil {
		return nil, apiError(500, "initialization_failed", "The client could not initialize cursor signing.")
	}
	return c, nil
}

// Get reads an implemented relative facade path, such as /apps/123, and returns
// flat JSON. Arbitrary URLs and hosted connection prefixes are unsupported.
func (c *Client) Get(ctx context.Context, path string, query Query) (json.RawMessage, error) {
	ctx, cancel := context.WithTimeout(ctx, 25*time.Second)
	defer cancel()
	value, err := c.read(ctx, path, query, &budget{})
	if err != nil {
		return nil, err
	}
	return json.Marshal(value)
}

func readTyped[T any](c *Client, ctx context.Context, path string, query Query) (T, error) {
	var value T
	data, err := c.Get(ctx, path, query)
	if err != nil {
		return value, err
	}
	if json.Unmarshal(data, &value) != nil {
		return value, invalidUpstream()
	}
	return value, nil
}

func pageData(payload map[string]any, kind string, limit int) ([]map[string]any, error) {
	items, ok := payload["data"].([]any)
	if !ok || len(items) > limit {
		return nil, invalidUpstream()
	}
	if _, ok := payload["links"].(map[string]any); !ok {
		return nil, invalidUpstream()
	}
	seen := map[any]bool{}
	result := make([]map[string]any, 0, len(items))
	for _, item := range items {
		v, err := resource(item, kind)
		if err != nil || seen[v["id"]] {
			return nil, invalidUpstream()
		}
		seen[v["id"]] = true
		result = append(result, v)
	}
	return result, nil
}

func (c *Client) traverse(ctx context.Context, path string, query url.Values, r route, b *budget, visit func([]map[string]any) (bool, error)) error {
	initial := appleURL(path, query)
	current, seen := initial, map[string]bool{}
	for page := 0; page < 20; page++ {
		if seen[digest(current)] {
			return incomplete()
		}
		seen[digest(current)] = true
		payload, err := c.apple(ctx, current, r, b, nil)
		if err != nil {
			return err
		}
		items, err := pageData(payload, r.Type, 200)
		if err != nil {
			return err
		}
		stop, err := visit(items)
		if err != nil || stop {
			return err
		}
		current, err = nextPage(payload, initial)
		if err != nil || current == "" {
			return err
		}
	}
	return incomplete()
}

func (c *Client) read(ctx context.Context, path string, query Query, b *budget) (map[string]any, error) {
	r, params, err := matchRoute(path)
	if err != nil {
		return nil, err
	}
	upstream, fields, cursor, limit, err := parseQuery(r, query)
	if err != nil {
		return nil, err
	}
	handler := r.Handler
	info := strings.HasPrefix(handler, "appInfo") || handler == "ageRating"
	appID := params["appId"]
	parentParam, parentType, parentHandler, localeHandler, localeType, parentRelation := "versionId", "appStoreVersions", "versions", "versionLocalizations", "appStoreVersionLocalizations", "appStoreVersion"
	if info {
		parentParam, parentType, parentHandler, localeHandler, localeType, parentRelation = "appInfoId", "appInfos", "appInfos", "appInfoLocalizations", "appInfoLocalizations", "appInfo"
	}
	parentID := params[parentParam]
	context := map[string]any{}
	if handler != "apps" && handler != "app" {
		context["appId"] = appID
		if parentID != "" && handler != "appInfo" && handler != "version" {
			context[parentParam] = parentID
		}
	}
	if parentID != "" {
		found := false
		q := url.Values{"limit": {"200"}}
		if !info {
			q.Set("filter[id]", parentID)
		}
		err = c.traverse(ctx, "/v1/apps/"+appID+"/"+parentType, q, routeByHandler(parentHandler), b, func(items []map[string]any) (bool, error) {
			for _, item := range items {
				if err := checkParent(item, "app", appID, "apps"); err != nil {
					return false, err
				}
				found = found || item["id"] == parentID
			}
			return found, nil
		})
		if err != nil {
			return nil, err
		}
		if !found {
			return nil, notFound("resource_not_found")
		}
	}
	paths := map[string]string{
		"apps": "/v1/apps", "app": "/v1/apps/" + appID,
		"appInfos": "/v1/apps/" + appID + "/appInfos", "appInfo": "/v1/appInfos/" + parentID,
		"versions": "/v1/apps/" + appID + "/appStoreVersions", "version": "/v1/appStoreVersions/" + parentID,
		"appInfoLocalizations": "/v1/appInfos/" + parentID + "/appInfoLocalizations",
		"versionLocalizations": "/v1/appStoreVersions/" + parentID + "/appStoreVersionLocalizations",
		"ageRating":            "/v1/appInfos/" + parentID + "/ageRatingDeclaration",
		"reviewContact":        "/v1/appStoreVersions/" + parentID + "/appStoreReviewDetail",
	}
	selected := ""
	if locale, exact := params["locale"]; exact {
		q := url.Values{"limit": {"200"}, "filter[locale]": {locale}, "fields[" + localeType + "]": {"locale"}}
		err = c.traverse(ctx, "/v1/"+parentType+"/"+parentID+"/"+localeType, q, routeByHandler(localeHandler), b, func(items []map[string]any) (bool, error) {
			for _, item := range items {
				if err := checkParent(item, parentRelation, parentID, parentType); err != nil {
					return false, err
				}
				attrs, _ := item["attributes"].(map[string]any)
				value, ok := attrs["locale"].(string)
				if !ok {
					return false, invalidUpstream()
				}
				if value == locale {
					if selected != "" {
						return false, apiError(409, "ambiguous_selection", "More than one localization matches this locale.")
					}
					selected = item["id"].(string)
				}
			}
			return false, nil
		})
		if err != nil {
			return nil, err
		}
		if selected == "" {
			return nil, notFound("localization_not_found")
		}
		paths[handler] = "/v1/" + localeType + "/" + selected
		if fields != nil && !contains(fields, "locale") {
			upstream.Set("fields["+localeType+"]", strings.Join(fields, ",")+",locale")
		}
	}
	if handler == "appInfos" || handler == "appInfo" {
		upstream.Set("include", "primaryCategory,secondaryCategory")
		if fields != nil {
			upstream.Set("fields[appInfos]", strings.Join(fields, ",")+",primaryCategory,secondaryCategory")
		}
	}
	initial := appleURL(paths[handler], upstream)
	if !r.Collection {
		payload, err := c.apple(ctx, initial, r, b, nil)
		if err != nil {
			return nil, err
		}
		if payload["data"] == nil {
			code := "resource_not_found"
			if handler == "reviewContact" {
				code = "review_contact_not_found"
			}
			return nil, notFound(code)
		}
		item, err := resource(payload["data"], r.Type)
		if err != nil {
			return nil, err
		}
		expected := selected
		if handler == "app" {
			expected = appID
		} else if handler == "appInfo" || handler == "version" {
			expected = parentID
			if err := checkParent(item, "app", appID, "apps"); err != nil {
				return nil, err
			}
		}
		if expected != "" && item["id"] != expected {
			return nil, invalidUpstream()
		}
		if locale, exact := params["locale"]; exact {
			attrs, _ := item["attributes"].(map[string]any)
			if attrs["locale"] != locale {
				return nil, invalidUpstream()
			}
			if err := checkParent(item, parentRelation, parentID, parentType); err != nil {
				return nil, err
			}
		}
		if handler == "reviewContact" {
			if err := checkParent(item, parentRelation, parentID, parentType); err != nil {
				return nil, err
			}
		}
		return flatten(item, r, context, fields)
	}
	state := cursorState{Binding: digest(path + "\n" + initial), Seen: []string{}, Expires: time.Now().Unix() + 900}
	current := initial
	if cursor != "" {
		state, err = c.decodeCursor(cursor, state.Binding)
		if err != nil {
			return nil, err
		}
		upstream.Set("cursor", state.Next)
		current = appleURL(paths[handler], upstream)
	}
	if len(state.Seen) >= 100 || contains(state.Seen, digest(current)) {
		return nil, incomplete()
	}
	state.Seen = append(state.Seen, digest(current))
	payload, err := c.apple(ctx, current, r, b, nil)
	if err != nil {
		return nil, err
	}
	data, err := pageData(payload, r.Type, limit)
	if err != nil {
		return nil, err
	}
	items := make([]map[string]any, 0, len(data))
	for _, item := range data {
		if handler == "appInfos" || handler == "versions" {
			if err := checkParent(item, "app", appID, "apps"); err != nil {
				return nil, err
			}
		}
		if handler == "appInfoLocalizations" || handler == "versionLocalizations" {
			if err := checkParent(item, parentRelation, parentID, parentType); err != nil {
				return nil, err
			}
		}
		flat, err := flatten(item, r, context, fields)
		if err != nil {
			return nil, err
		}
		items = append(items, flat)
	}
	nextURL, err := nextPage(payload, initial)
	if err != nil {
		return nil, err
	}
	var nextCursor any
	if nextURL != "" {
		if contains(state.Seen, digest(nextURL)) {
			return nil, incomplete()
		}
		u, _ := url.Parse(nextURL)
		state.Next = u.Query().Get("cursor")
		nextCursor, err = c.encodeCursor(state)
		if err != nil {
			return nil, err
		}
	}
	return map[string]any{"items": items, "nextCursor": nextCursor}, nil
}

// UpdateAgeRating snapshots and validates attributes, verifies ownership, and
// sends one Apple PATCH. Writes are never retried, including authentication failures.
func (c *Client) UpdateAgeRating(ctx context.Context, appID, appInfoID string, attributes AgeRatingUpdate) (AgeRating, error) {
	var result AgeRating
	for _, id := range []string{appID, appInfoID} {
		if err := validateSegment(id, false); err != nil {
			return result, err
		}
	}
	attrs, err := validateUpdate(attributes)
	if err != nil {
		return result, err
	}
	ctx, cancel := context.WithTimeout(ctx, 25*time.Second)
	defer cancel()
	b := &budget{}
	current, err := c.read(ctx, "/apps/"+appID+"/app-infos/"+appInfoID+"/age-rating", nil, b)
	if err != nil {
		return result, err
	}
	id := current["id"].(string)
	r := routeByHandler("ageRating")
	body, _ := json.Marshal(map[string]any{"data": map[string]any{"type": r.Type, "id": id, "attributes": attrs}})
	payload, err := c.apple(ctx, appleURL("/v1/ageRatingDeclarations/"+id, nil), r, b, body)
	if err != nil {
		return result, err
	}
	item, err := resource(payload["data"], r.Type)
	if err != nil || item["id"] != id {
		return result, unknownWrite()
	}
	flat, err := flatten(item, r, map[string]any{"appId": appID, "appInfoId": appInfoID}, nil)
	if err != nil {
		return result, unknownWrite()
	}
	data, _ := json.Marshal(flat)
	if json.Unmarshal(data, &result) != nil {
		return result, unknownWrite()
	}
	return result, nil
}
