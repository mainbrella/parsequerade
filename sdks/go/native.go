//go:build cgo

package parsequerade

/*
#cgo CFLAGS: -I${SRCDIR}/native
#cgo LDFLAGS: ${SRCDIR}/native/libparsequerade_native.a
#cgo darwin LDFLAGS: -framework Security -framework CoreFoundation -framework SystemConfiguration -liconv
#cgo linux LDFLAGS: -ldl -lpthread -lm
#cgo windows LDFLAGS: -lws2_32 -luserenv -lbcrypt -lntdll
#include <stdlib.h>
#include "parsequerade.h"
*/
import "C"

import (
	"encoding/json"
	"unsafe"
)

func takeNative(value *C.char) []byte {
	if value == nil {
		return nil
	}
	defer C.pq_string_free(value)
	return []byte(C.GoString(value))
}
func nativeNew(config []byte) []byte {
	if C.pq_abi_version() != 1 {
		return []byte(`{"error":{"status":500,"code":"native_abi_mismatch","message":"Rebuild the native SDK library."}}`)
	}
	value := C.CString(string(config))
	defer C.free(unsafe.Pointer(value))
	return takeNative(C.pq_client_new(value))
}
func nativeStart(client uint64, request []byte) []byte {
	value := C.CString(string(request))
	defer C.free(unsafe.Pointer(value))
	return takeNative(C.pq_start(C.uint64_t(client), value))
}
func nativePoll(operation uint64, timeout uint32) []byte {
	return takeNative(C.pq_poll(C.uint64_t(operation), C.uint32_t(timeout)))
}
func nativeRespond(operation, event uint64, response any) {
	encoded, err := json.Marshal(response)
	if err != nil {
		encoded = []byte(`{"error":"failed"}`)
	}
	value := C.CString(string(encoded))
	defer C.free(unsafe.Pointer(value))
	C.pq_respond(C.uint64_t(operation), C.uint64_t(event), value)
}
func nativeCancel(operation uint64)        { C.pq_cancel(C.uint64_t(operation)) }
func nativeOperationFree(operation uint64) { C.pq_operation_free(C.uint64_t(operation)) }
func nativeClientFree(client uint64)       { C.pq_client_free(C.uint64_t(client)) }
