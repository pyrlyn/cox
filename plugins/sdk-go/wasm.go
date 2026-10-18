//go:build wasip1

package cox

// The extism side of the wire: go-pdk's memory for export input, output
// and errors, and the raw `cox:host/v1` / `cox:desktop/v1` imports. Every
// import has the same `(u64) -> u64` shape, so `call` in cox.go takes the
// import as a value and stays generic. Each wrapper names its own import,
// so TinyGo links only the imports a plugin reaches: one that never calls
// DesktopNotify imports nothing from `cox:desktop/v1`.

import (
	"errors"

	"github.com/extism/go-pdk"
)

//go:wasmimport cox:host/v1 cox_log
func rawCoxLog(uint64) uint64

//go:wasmimport cox:host/v1 cox_notify
func rawCoxNotify(uint64) uint64

//go:wasmimport cox:host/v1 cox_kv_get
func rawCoxKVGet(uint64) uint64

//go:wasmimport cox:host/v1 cox_kv_put
func rawCoxKVPut(uint64) uint64

//go:wasmimport cox:host/v1 cox_kv_delete
func rawCoxKVDelete(uint64) uint64

//go:wasmimport cox:host/v1 cox_context
func rawCoxContext(uint64) uint64

//go:wasmimport cox:host/v1 cox_invoke_tool
func rawCoxInvokeTool(uint64) uint64

//go:wasmimport cox:host/v1 cox_model_call
func rawCoxModelCall(uint64) uint64

//go:wasmimport cox:host/v1 cox_http
func rawCoxHTTP(uint64) uint64

//go:wasmimport cox:host/v1 cox_output
func rawCoxOutput(uint64) uint64

//go:wasmimport cox:host/v1 cox_cancelled
func rawCoxCancelled(uint64) uint64

//go:wasmimport cox:host/v1 cox_redraw
func rawCoxRedraw(uint64) uint64

// Its own import module (PL§15.4): a plugin that never calls it imports
// nothing from it, so it still loads in a cox that predates the section.
//
//go:wasmimport cox:desktop/v1 cox_desktop_notify
func rawCoxDesktopNotify(uint64) uint64

// TinyGo cannot take a `//go:wasmimport` function as a value, so `call`
// gets these plain wrappers instead.
func coxLog(a uint64) uint64           { return rawCoxLog(a) }
func coxNotify(a uint64) uint64        { return rawCoxNotify(a) }
func coxKVGet(a uint64) uint64         { return rawCoxKVGet(a) }
func coxKVPut(a uint64) uint64         { return rawCoxKVPut(a) }
func coxKVDelete(a uint64) uint64      { return rawCoxKVDelete(a) }
func coxContext(a uint64) uint64       { return rawCoxContext(a) }
func coxInvokeTool(a uint64) uint64    { return rawCoxInvokeTool(a) }
func coxModelCall(a uint64) uint64     { return rawCoxModelCall(a) }
func coxHTTP(a uint64) uint64          { return rawCoxHTTP(a) }
func coxOutput(a uint64) uint64        { return rawCoxOutput(a) }
func coxCancelled(a uint64) uint64     { return rawCoxCancelled(a) }
func coxRedraw(a uint64) uint64        { return rawCoxRedraw(a) }
func coxDesktopNotify(a uint64) uint64 { return rawCoxDesktopNotify(a) }

func callHost(imp func(uint64) uint64, arg []byte) ([]byte, error) {
	block := pdk.AllocateBytes(arg)
	offset := imp(block.Offset())
	block.Free()
	reply := pdk.FindMemory(offset)
	if reply.Length() == 0 {
		return nil, errors.New("abi wire: the host returned no block")
	}
	bytes := reply.ReadBytes()
	reply.Free()
	return bytes, nil
}

func readInput() []byte { return pdk.Input() }

func setOutput(out []byte) { pdk.Output(out) }

func setError(message string) { pdk.SetErrorString(message) }
