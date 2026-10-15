package parsequerade

import (
	_ "embed"
	"encoding/json"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"unicode/utf16"
)

//go:embed contracts.json
var contractsJSON []byte

type schema struct {
	Type     string   `json:"type"`
	Format   string   `json:"format"`
	Enum     []string `json:"enum"`
	Nullable bool     `json:"nullable"`
	Maximum  int      `json:"maximum"`
	Items    *schema  `json:"items"`
}

type parameter struct {
	Name      string `json:"name"`
	AppleName string `json:"appleName"`
	Schema    schema `json:"schema"`
	Default   *int   `json:"default"`
}

type relation struct {
	Field string `json:"field"`
	Type  string `json:"type"`
}

type route struct {
	Handler       string              `json:"handler"`
	Route         string              `json:"route"`
	Type          string              `json:"type"`
	Collection    bool                `json:"collection"`
	Attributes    map[string]schema   `json:"attributes"`
	Relationships map[string]relation `json:"relationships"`
	Parameters    []parameter         `json:"parameters"`
}

var contracts = func() struct {
	Routes         []route           `json:"routes"`
	AgeRatingWrite map[string]schema `json:"ageRatingWrite"`
} {
	var value struct {
		Routes         []route           `json:"routes"`
		AgeRatingWrite map[string]schema `json:"ageRatingWrite"`
	}
	if err := json.Unmarshal(contractsJSON, &value); err != nil {
		panic("parsequerade: invalid embedded contracts")
	}
	return value
}()

var idPattern = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$`)
var localePattern = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9-]{1,34}$`)
var limitPattern = regexp.MustCompile(`^[1-9][0-9]{0,2}$`)
var safeCodePattern = regexp.MustCompile(`^[A-Za-z0-9_.-]{1,128}$`)

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

func routeByHandler(handler string) route {
	for _, r := range contracts.Routes {
		if r.Handler == handler {
			return r
		}
	}
	panic("parsequerade: missing embedded route")
}

func matchRoute(path string) (route, map[string]string, error) {
	parts := strings.Split(path, "/")
	for _, r := range contracts.Routes {
		template := strings.Split(r.Route, "/")
		if len(template) != len(parts) {
			continue
		}
		params, matched := map[string]string{}, true
		for i, part := range template {
			if strings.HasPrefix(part, "{") {
				params[strings.Trim(part, "{}")] = parts[i]
			} else if part != parts[i] {
				matched = false
				break
			}
		}
		if matched {
			for name, value := range params {
				if err := validateSegment(value, name == "locale"); err != nil {
					return route{}, nil, err
				}
			}
			return r, params, nil
		}
	}
	return route{}, nil, notFound("route_not_found")
}

func contains(values []string, value string) bool {
	for _, v := range values {
		if v == value {
			return true
		}
	}
	return false
}

func parseQuery(r route, query Query) (url.Values, []string, string, int, error) {
	params := map[string]parameter{}
	for _, p := range r.Parameters {
		params[p.Name] = p
	}
	upstream, fields, cursor, limit := url.Values{}, []string(nil), "", 50
	for name, values := range query {
		p, ok := params[name]
		maxLength := 4096
		if name == "cursor" {
			maxLength = 8192
		}
		if !ok || len(values) != 1 || len(values[0]) == 0 || len(values[0]) > maxLength {
			return nil, nil, "", 0, invalidQuery(name)
		}
		value := values[0]
		switch p.Schema.Type {
		case "integer":
			number, _ := strconv.Atoi(value)
			if !limitPattern.MatchString(value) || number > p.Schema.Maximum {
				return nil, nil, "", 0, invalidQuery(name)
			}
		case "boolean":
			if value != "true" && value != "false" {
				return nil, nil, "", 0, invalidQuery(name)
			}
		case "array":
			parts := strings.Split(value, ",")
			if len(parts) > 200 {
				return nil, nil, "", 0, invalidQuery(name)
			}
			for _, part := range parts {
				if part == "" || len(part) > 256 || (len(p.Schema.Items.Enum) > 0 && !contains(p.Schema.Items.Enum, part)) {
					return nil, nil, "", 0, invalidQuery(name)
				}
			}
		}
		if p.AppleName != "" {
			upstream.Set(p.AppleName, value)
		}
		switch name {
		case "fields":
			fields = strings.Split(value, ",")
		case "cursor":
			cursor = value
		case "limit":
			limit, _ = strconv.Atoi(value)
		}
	}
	for _, p := range r.Parameters {
		if _, ok := query[p.Name]; !ok && p.Default != nil {
			upstream.Set(p.AppleName, strconv.Itoa(*p.Default))
		}
	}
	return upstream, fields, cursor, limit, nil
}

func resource(value any, kind string) (map[string]any, error) {
	v, ok := value.(map[string]any)
	if !ok || v["type"] != kind {
		return nil, invalidUpstream()
	}
	id, ok := v["id"].(string)
	if !ok || !idPattern.MatchString(id) {
		return nil, invalidUpstream()
	}
	for _, key := range []string{"attributes", "relationships"} {
		if item, exists := v[key]; exists {
			if _, ok := item.(map[string]any); !ok {
				return nil, invalidUpstream()
			}
		}
	}
	return v, nil
}

func relationship(value map[string]any, name string) (map[string]any, error) {
	rels, _ := value["relationships"].(map[string]any)
	if item, exists := rels[name]; exists {
		rel, ok := item.(map[string]any)
		if !ok {
			return nil, invalidUpstream()
		}
		return rel, nil
	}
	return nil, nil
}

func checkParent(value map[string]any, name, id, kind string) error {
	rel, err := relationship(value, name)
	if err != nil {
		return err
	}
	if data, exists := rel["data"]; exists {
		parent, err := resource(data, kind)
		if err != nil || parent["id"] != id {
			return invalidUpstream()
		}
	}
	return nil
}

func flatten(value any, r route, context map[string]any, fields []string) (map[string]any, error) {
	v, err := resource(value, r.Type)
	if err != nil {
		return nil, err
	}
	result := map[string]any{"id": v["id"]}
	for key, item := range context {
		result[key] = item
	}
	attrs, _ := v["attributes"].(map[string]any)
	for name, schema := range r.Attributes {
		if fields != nil && !contains(fields, name) {
			continue
		}
		if item, exists := attrs[name]; exists {
			if item != nil {
				_, text := item.(string)
				_, boolean := item.(bool)
				if (schema.Type == "string" && !text) || (schema.Type == "boolean" && !boolean) {
					return nil, invalidUpstream()
				}
			}
			result[name] = item
		}
	}
	for name, relation := range r.Relationships {
		rel, err := relationship(v, name)
		if err != nil {
			return nil, err
		}
		if data, exists := rel["data"]; exists {
			result[relation.Field] = nil
			if data != nil {
				item, err := resource(data, relation.Type)
				if err != nil {
					return nil, err
				}
				result[relation.Field] = item["id"]
			}
		}
	}
	return result, nil
}

func validateUpdate(value AgeRatingUpdate) (map[string]any, error) {
	encoded, err := json.Marshal(value)
	if err != nil {
		return nil, apiError(400, "invalid_request", "Provide valid JSON attributes.")
	}
	if len(encoded) > 8192 {
		return nil, apiError(413, "request_too_large", "Age rating attributes must fit within 8 KiB.")
	}
	var attrs map[string]any
	if json.Unmarshal(encoded, &attrs) != nil || len(attrs) == 0 {
		return nil, apiError(400, "invalid_request", "Provide a nonempty object of supported age rating attributes.")
	}
	for name, item := range attrs {
		schema, valid := contracts.AgeRatingWrite[name]
		if item == nil {
			valid = valid && schema.Nullable
		} else if schema.Type == "boolean" {
			_, valid = item.(bool)
		} else {
			text, ok := item.(string)
			valid = valid && ok && len(utf16.Encode([]rune(text))) <= 2048 && (len(schema.Enum) == 0 || contains(schema.Enum, text))
			if valid && schema.Format == "uri" {
				u, err := url.Parse(text)
				valid = err == nil && (u.Scheme == "http" || u.Scheme == "https") && u.Hostname() != "" && u.User == nil
			}
		}
		if !valid {
			e := apiError(400, "invalid_request", "Unsupported age rating attribute or value.")
			e.Details = []ErrorDetail{{Code: "invalid_attribute", Message: "This age rating attribute or value is unsupported.", Field: name}}
			return nil, e
		}
	}
	return attrs, nil
}
