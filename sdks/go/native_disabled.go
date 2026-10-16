//go:build !cgo

package parsequerade

func nativeNew([]byte) []byte {
	return []byte(`{"error":{"status":500,"code":"native_unavailable","message":"The Go SDK requires CGO_ENABLED=1 and the shared Rust static library. Run python3 scripts/build_go_sdk.py in the source checkout."}}`)
}
func nativeStart(uint64, []byte) []byte { return nil }
func nativePoll(uint64, uint32) []byte  { return nil }
func nativeRespond(uint64, uint64, any) {}
func nativeCancel(uint64)               {}
func nativeOperationFree(uint64)        {}
func nativeClientFree(uint64)           {}
