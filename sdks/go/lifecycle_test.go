package parsequerade

import (
	"context"
	"crypto/elliptic"
	"errors"
	"io/fs"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestCloseCancelsConcurrentOperations(t *testing.T) {
	_, pem := testKey(t, elliptic.P256())
	entered := make(chan struct{}, 4)
	client, err := NewClient(Credentials{IssuerID: "issuer", KeyID: "key", PrivateKey: pem}, &Options{Transport: roundTripFunc(func(r *http.Request) (*http.Response, error) {
		entered <- struct{}{}
		<-r.Context().Done()
		return nil, r.Context().Err()
	})})
	if err != nil {
		t.Fatal(err)
	}
	results := make(chan error, 4)
	for i := 0; i < 4; i++ {
		go func() { _, err := client.Apps(context.Background(), nil); results <- err }()
	}
	for i := 0; i < 4; i++ {
		select {
		case <-entered:
		case <-time.After(3 * time.Second):
			t.Fatal("operation did not start")
		}
	}
	var closers sync.WaitGroup
	for i := 0; i < 8; i++ {
		closers.Add(1)
		go func() { defer closers.Done(); client.Close() }()
	}
	closers.Wait()
	for i := 0; i < 4; i++ {
		select {
		case err := <-results:
			if !errors.Is(err, context.Canceled) {
				t.Fatalf("active read: %v", err)
			}
		case <-time.After(3 * time.Second):
			t.Fatal("Close did not cancel operation")
		}
	}
	_, err = client.Apps(context.Background(), nil)
	requireCode(t, err, "client_closed")
}

func TestContextDeadlineCancelsActiveTransport(t *testing.T) {
	_, pem := testKey(t, elliptic.P256())
	client, err := NewClient(Credentials{IssuerID: "issuer", KeyID: "key", PrivateKey: pem}, &Options{Transport: roundTripFunc(func(r *http.Request) (*http.Response, error) {
		<-r.Context().Done()
		return nil, r.Context().Err()
	})})
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
	defer cancel()
	_, err = client.Apps(ctx, nil)
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("deadline: %v", err)
	}
}

func TestConsumerBuildOutsideCheckout(t *testing.T) {
	if testing.Short() {
		t.Skip("isolated consumer build")
	}
	root := t.TempDir()
	sdk := filepath.Join(root, "sdk")
	if err := os.MkdirAll(sdk, 0755); err != nil {
		t.Fatal(err)
	}
	err := filepath.WalkDir(".", func(path string, entry fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if entry.IsDir() {
			if path == "testdata" || path == "examples" {
				return filepath.SkipDir
			}
			return nil
		}
		if strings.HasSuffix(path, "_test.go") || strings.HasPrefix(path, ".") {
			return nil
		}
		if !strings.HasSuffix(path, ".go") && path != "go.mod" && !strings.HasPrefix(path, "native"+string(filepath.Separator)) {
			return nil
		}
		destination := filepath.Join(sdk, path)
		if err := os.MkdirAll(filepath.Dir(destination), 0755); err != nil {
			return err
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		return os.WriteFile(destination, data, 0644)
	})
	if err != nil {
		t.Fatal(err)
	}
	module := "module consumer\n\ngo 1.22\n\nrequire github.com/mainbrella/parsequerade/sdks/go v0.0.0\nreplace github.com/mainbrella/parsequerade/sdks/go => ./sdk\n"
	if err := os.WriteFile(filepath.Join(root, "go.mod"), []byte(module), 0644); err != nil {
		t.Fatal(err)
	}
	main := `package main
import (
 "errors"
 p "github.com/mainbrella/parsequerade/sdks/go"
)
func main(){
 _,err:=p.NewClient(p.Credentials{},nil)
 var api *p.APIError
 if !errors.As(err,&api)||api.Code!="invalid_credentials"{panic("native SDK unavailable")}
 var _ p.ListingImageUpload
 var _ p.Build
}
`
	if err := os.WriteFile(filepath.Join(root, "main.go"), []byte(main), 0644); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, "go", "run", ".")
	command.Dir = root
	command.Env = append(os.Environ(), "GOWORK=off", "GOPROXY=off")
	if output, err := command.CombinedOutput(); err != nil {
		t.Fatalf("isolated consumer: %v\n%s", err, output)
	}
}
