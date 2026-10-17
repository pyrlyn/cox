//go:build !wasip1

package cox

// Off wasm there is no extism memory and no host: the pure parts in cox.go
// build for unit tests, and every host function returns ErrNotWasm. The
// imports are nil func values so cox.go names them the same way on both
// builds.

var coxLog, coxNotify, coxKVGet, coxKVPut, coxKVDelete, coxContext, coxInvokeTool,
	coxModelCall, coxHTTP, coxOutput, coxCancelled, coxRedraw, coxDesktopNotify func(uint64) uint64

func callHost(func(uint64) uint64, []byte) ([]byte, error) { return nil, ErrNotWasm }

func readInput() []byte { return nil }

func setOutput([]byte) {}

func setError(string) {}
