package parsequerade

import (
	"context"
	"crypto/elliptic"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"reflect"
	"strings"
	"testing"
)

type scenarioStep struct {
	Method   string            `json:"method"`
	Path     string            `json:"path"`
	Query    map[string]string `json:"query"`
	Body     any               `json:"body"`
	Response any               `json:"response"`
	Status   int               `json:"status"`
}

func newScenarioClient(t *testing.T, steps []scenarioStep) (*Client, *int) {
	t.Helper()
	_, pem := testKey(t, elliptic.P256())
	index := 0
	client, err := NewClient(Credentials{IssuerID: "issuer", KeyID: "key", PrivateKey: pem}, &Options{Transport: roundTripFunc(func(r *http.Request) (*http.Response, error) {
		if index >= len(steps) {
			return nil, fmt.Errorf("unexpected %s %s", r.Method, r.URL)
		}
		step := steps[index]
		index++
		if r.Method != step.Method || r.URL.Path != step.Path {
			t.Errorf("step %d: got %s %s, want %s %s", index, r.Method, r.URL.Path, step.Method, step.Path)
		}
		query := url.Values{}
		for key, value := range step.Query {
			query.Set(key, value)
		}
		if !reflect.DeepEqual(r.URL.Query(), query) {
			t.Errorf("step %d query %v, want %v", index, r.URL.Query(), query)
		}
		body, _ := io.ReadAll(r.Body)
		var value any
		if len(body) > 0 {
			if err := json.Unmarshal(body, &value); err != nil {
				t.Error(err)
			}
		}
		if !reflect.DeepEqual(value, step.Body) {
			t.Errorf("step %d body %s, want %#v", index, body, step.Body)
		}
		status := step.Status
		if status == 0 {
			status = 200
		}
		return reply(step.Response, status), nil
	})})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { client.Close() })
	return client, &index
}

func assertSubset(t *testing.T, actual, expected any) {
	t.Helper()
	switch wanted := expected.(type) {
	case map[string]any:
		got, ok := actual.(map[string]any)
		if !ok {
			t.Fatalf("expected object, got %#v", actual)
		}
		for key, value := range wanted {
			found, ok := got[key]
			if !ok {
				t.Fatalf("missing %s in %#v", key, got)
			}
			assertSubset(t, found, value)
		}
	case []any:
		got, ok := actual.([]any)
		if !ok || len(got) != len(wanted) {
			t.Fatalf("array mismatch: %#v != %#v", actual, expected)
		}
		for i, value := range wanted {
			assertSubset(t, got[i], value)
		}
	default:
		if !reflect.DeepEqual(actual, expected) {
			t.Fatalf("%#v != %#v", actual, expected)
		}
	}
}

func TestSharedRustOperationScenarios(t *testing.T) {
	data, err := os.ReadFile("testdata/scenarios.json")
	if err != nil {
		t.Fatal(err)
	}
	var scenarios []struct {
		Name       string         `json:"name"`
		Operation  string         `json:"operation"`
		Args       []any          `json:"args"`
		Method     string         `json:"pythonMethod"`
		MethodArgs []any          `json:"pythonArgs"`
		Steps      []scenarioStep `json:"steps"`
		Expect     any            `json:"expect"`
		Error      map[string]any `json:"error"`
	}
	if err := json.Unmarshal(data, &scenarios); err != nil {
		t.Fatal(err)
	}
	for _, scenario := range scenarios {
		t.Run(scenario.Name, func(t *testing.T) {
			client, index := newScenarioClient(t, scenario.Steps)
			var value any
			var err error
			ctx := context.Background()
			switch scenario.Operation {
			case "get":
				name := ""
				for _, word := range strings.Split(scenario.Method, "_") {
					name += strings.ToUpper(word[:1]) + word[1:]
				}
				method := reflect.ValueOf(client).MethodByName(name)
				if !method.IsValid() {
					t.Fatalf("missing generated method %s", name)
				}
				args := []reflect.Value{reflect.ValueOf(ctx)}
				for i, arg := range scenario.MethodArgs {
					if query, ok := arg.(map[string]any); ok {
						typed := reflect.New(method.Type().In(i + 1).Elem())
						for key, value := range query {
							field := typed.Elem().FieldByName(strings.ToUpper(key[:1]) + key[1:])
							switch field.Kind() {
							case reflect.Int:
								field.SetInt(int64(value.(float64)))
							case reflect.String:
								field.SetString(value.(string))
							case reflect.Slice:
								if text, ok := value.(string); ok {
									field.Set(reflect.ValueOf(strings.Split(text, ",")))
								} else {
									list := []string{}
									for _, v := range value.([]any) {
										list = append(list, v.(string))
									}
									field.Set(reflect.ValueOf(list))
								}
							default:
								t.Fatalf("unsupported query %s", key)
							}
						}
						args = append(args, typed)
					} else {
						args = append(args, reflect.ValueOf(arg))
					}
				}
				result := method.Call(args)
				if !result[1].IsNil() {
					err = result[1].Interface().(error)
				} else {
					value = result[0].Interface()
				}
			case "update_metadata":
				value, err = client.UpdateMetadata(ctx, scenario.Args[0].(string), scenario.Args[1].(map[string]any))
			case "create_version":
				value, err = client.CreateVersion(ctx, scenario.Args[0].(string), scenario.Args[1].(string), scenario.Args[2].(string))
			case "attach_build":
				value, err = client.AttachBuild(ctx, scenario.Args[0].(string), scenario.Args[1].(string), scenario.Args[2].(string))
			default:
				t.Fatalf("unknown operation %s", scenario.Operation)
			}
			if scenario.Error != nil {
				api := requireCode(t, err, scenario.Error["code"].(string))
				encoded, _ := json.Marshal(api)
				var actual any
				json.Unmarshal(encoded, &actual)
				assertSubset(t, actual, scenario.Error)
			} else {
				if err != nil {
					t.Fatal(err)
				}
				encoded, err := json.Marshal(value)
				if err != nil {
					t.Fatal(err)
				}
				var actual any
				if err := json.Unmarshal(encoded, &actual); err != nil {
					t.Fatal(err)
				}
				assertSubset(t, actual, scenario.Expect)
			}
			if *index != len(scenario.Steps) {
				t.Fatalf("%d requests, want %d", *index, len(scenario.Steps))
			}
		})
	}
}
