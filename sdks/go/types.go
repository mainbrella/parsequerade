// Package parsequerade runs the Parsequerade facade locally and calls App Store
// Connect directly. It requires an Apple team API key and no hosted connection.
package parsequerade

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/url"
)

// Query is a facade query, using camel case names. Get validates every value.
// Multiple values for one key are rejected; use a comma-separated selection.
type Query = url.Values

// Optional preserves absent, null, and concrete JSON values, including false.
// The zero value is absent. Some constructs a value; Null constructs JSON null.
type Optional[T any] struct {
	Present bool
	Null    bool
	Value   T
}

func Some[T any](value T) Optional[T] { return Optional[T]{Present: true, Value: value} }
func Null[T any]() Optional[T]        { return Optional[T]{Present: true, Null: true} }

func (v *Optional[T]) UnmarshalJSON(data []byte) error {
	*v = Optional[T]{Present: true, Null: bytes.Equal(bytes.TrimSpace(data), []byte("null"))}
	if v.Null {
		return nil
	}
	return json.Unmarshal(data, &v.Value)
}

func (v Optional[T]) MarshalJSON() ([]byte, error) {
	if !v.Present || v.Null {
		return []byte("null"), nil
	}
	return json.Marshal(v.Value)
}

func addField[T any](m map[string]any, name string, value Optional[T]) {
	if value.Present {
		m[name] = value
	}
}

type Page[T any] struct {
	Items      []T     `json:"items"`
	NextCursor *string `json:"nextCursor"`
}

// Credentials contains the PEM contents of an Apple P-256 PKCS#8 team key.
type Credentials struct {
	IssuerID   string
	KeyID      string
	PrivateKey []byte
}

// Options configures a trusted transport. It receives signed Apple requests and
// must honor request contexts. Nil uses http.DefaultTransport. Redirects are
// never followed, even with a custom transport.
type Options struct {
	Transport http.RoundTripper
}
