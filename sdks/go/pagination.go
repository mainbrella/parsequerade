package parsequerade

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/json"
	"net/url"
	"strings"
	"time"
)

func appleURL(path string, query url.Values) string {
	if len(query) == 0 {
		return origin + path
	}
	return origin + path + "?" + query.Encode()
}

func digest(value string) string {
	hash := sha256.Sum256([]byte(value))
	return base64url.EncodeToString(hash[:16])
}

func nextPage(payload map[string]any, initial string) (string, error) {
	links, ok := payload["links"].(map[string]any)
	if !ok {
		return "", invalidUpstream()
	}
	link := links["next"]
	if link == nil {
		return "", nil
	}
	raw, ok := link.(string)
	if !ok || len(raw) > 8192 {
		return "", invalidUpstream()
	}
	start, _ := url.Parse(initial)
	reference, err := url.Parse(raw)
	if err != nil {
		return "", invalidUpstream()
	}
	u := start.ResolveReference(reference)
	if u.Scheme != start.Scheme || u.Host != start.Host || u.Path != start.Path || u.RawPath != "" || u.User != nil || u.Fragment != "" {
		return "", invalidUpstream()
	}
	query, err := url.ParseQuery(u.RawQuery)
	if err != nil {
		return "", invalidUpstream()
	}
	original := start.Query()
	for name, values := range query {
		if len(values) != 1 || name != "cursor" && (len(original[name]) != 1 || original.Get(name) != values[0]) {
			return "", invalidUpstream()
		}
	}
	cursor := query.Get("cursor")
	if cursor == "" || len(cursor) > 2048 {
		return "", invalidUpstream()
	}
	original.Set("cursor", cursor)
	return appleURL(start.Path, original), nil
}

type cursorState struct {
	Binding string   `json:"binding"`
	Next    string   `json:"next"`
	Seen    []string `json:"seen"`
	Expires int64    `json:"expires"`
}

func (c *Client) encodeCursor(state cursorState) (string, error) {
	data, err := json.Marshal(state)
	if err != nil {
		return "", invalidUpstream()
	}
	payload := base64url.EncodeToString(data)
	mac := hmac.New(sha256.New, c.cursorKey[:])
	mac.Write([]byte(payload))
	token := payload + "." + base64url.EncodeToString(mac.Sum(nil))
	if len(token) > 8192 {
		return "", invalidUpstream()
	}
	return token, nil
}

func (c *Client) decodeCursor(token, binding string) (cursorState, error) {
	invalid := apiError(400, "invalid_cursor", "The cursor is invalid, expired, or belongs to another client or query.")
	parts := strings.Split(token, ".")
	if len(token) > 8192 || len(parts) != 2 {
		return cursorState{}, invalid
	}
	mac := hmac.New(sha256.New, c.cursorKey[:])
	mac.Write([]byte(parts[0]))
	signature, err := base64url.DecodeString(parts[1])
	if err != nil || !hmac.Equal(mac.Sum(nil), signature) {
		return cursorState{}, invalid
	}
	data, err := base64url.DecodeString(parts[0])
	if err != nil {
		return cursorState{}, invalid
	}
	var state cursorState
	now := time.Now().Unix()
	if json.Unmarshal(data, &state) != nil || state.Binding != binding || state.Expires <= now || state.Expires > now+900 || state.Next == "" || len(state.Next) > 2048 || len(state.Seen) == 0 || len(state.Seen) > 100 {
		return cursorState{}, invalid
	}
	seen := map[string]bool{}
	for _, item := range state.Seen {
		if item == "" || seen[item] {
			return cursorState{}, invalid
		}
		seen[item] = true
	}
	return state, nil
}
