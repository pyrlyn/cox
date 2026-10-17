// Package cox writes a cox plugin in Go: typed wrappers over every guest
// export and host function of ABI v1 (docs/design/plugins.md PL§4,
// `cox:host/v1`, and `cox:desktop/v1` from PL§15) on top of
// github.com/extism/go-pdk's memory, the Go twin of the Rust
// `cox-plugin-sdk` (plugins/sdk). Build a plugin with TinyGo:
//
//	tinygo build -o plugin.wasm -target wasip1 -buildmode=c-shared .
//
// and set `wasi = true` in its plugin.toml, since TinyGo's wasip1 runtime
// imports WASI.
//
// The wire (docs/plugins.md "The wire, for other languages"): an export
// reads one JSON value (`null` when the host sends no input) and writes one;
// a handler error becomes the extism call error and return code 1. A host
// function takes one block holding one JSON value (an object keyed by its
// argument names when it has several, `null` when it has none) and returns
// a block holding `{"Ok": value}` or `{"Err": AbiError}`, so a refusal
// reaches the plugin as a *HostError instead of a trap.
//
// This file is the pure part, built and tested on any GOOS; the extism
// memory calls live in wasm.go (`wasip1` only) because go-pdk's
// `//go:wasmimport` declarations do not build for a host target.
package cox

import (
	"encoding/json"
	"errors"
	"fmt"
)

// Export runs one guest export: it decodes the host's input into I, calls
// handler and sets its result as the output. Use it as the whole body of a
// `//go:wasmexport` function:
//
//	//go:wasmexport cox_init
//	func coxInit() int32 { return cox.Export(initPlugin) }
//
// Only `cox_init` is required; cox probes the rest by name (PL§4). The
// payload types are in docs/plugin-abi.schema.json; the common ones are
// below, and json.RawMessage carries any other.
func Export[I, O any](handler func(I) (O, error)) int32 {
	out, err := runExport(readInput(), handler)
	if err != nil {
		setError(err.Error())
		return 1
	}
	setOutput(out)
	return 0
}

func runExport[I, O any](input []byte, handler func(I) (O, error)) ([]byte, error) {
	// `cox_shutdown` takes `()`, which a host may send as no bytes at all.
	if len(input) == 0 {
		input = []byte("null")
	}
	var in I
	if err := json.Unmarshal(input, &in); err != nil {
		return nil, fmt.Errorf("bad input: %w", err)
	}
	out, err := handler(in)
	if err != nil {
		return nil, err
	}
	bytes, err := json.Marshal(out)
	if err != nil {
		return nil, fmt.Errorf("bad output: %w", err)
	}
	return bytes, nil
}

// HostError is an `AbiError`: cox refused or failed a host call (not
// granted, not allowed from this export, denied, over budget or a cap,
// timed out). Kind is its tag; the other fields are set by the kinds that
// carry them.
type HostError struct {
	Kind       string  `json:"kind"`
	Capability string  `json:"capability,omitempty"`
	Reason     string  `json:"reason,omitempty"`
	Message    string  `json:"message,omitempty"`
	Limit      uint64  `json:"limit,omitempty"`
	Surface    Surface `json:"surface,omitempty"`
}

func (e *HostError) Error() string {
	// Each kind sets at most one detail field, so concatenating them is exact.
	detail := e.Capability + e.Reason + e.Message + string(e.Surface)
	if e.Limit != 0 {
		detail = fmt.Sprint(e.Limit)
	}
	if detail == "" {
		return "cox host call failed: " + e.Kind
	}
	return "cox host call failed: " + e.Kind + ": " + detail
}

// ErrNotWasm is what every host function returns off wasm, where only the
// pure parts exist for unit tests.
var ErrNotWasm = errors.New("cox host functions need GOOS=wasip1")

// decodeReply reads a host function's reply block into a Go value.
func decodeReply(bytes []byte, into any) error {
	var envelope map[string]json.RawMessage
	if err := json.Unmarshal(bytes, &envelope); err != nil {
		return fmt.Errorf("abi wire: %w", err)
	}
	if raw, ok := envelope["Err"]; ok {
		var hostErr HostError
		if err := json.Unmarshal(raw, &hostErr); err != nil {
			return fmt.Errorf("abi wire: %w", err)
		}
		return &hostErr
	}
	raw, ok := envelope["Ok"]
	if !ok {
		return errors.New("abi wire: reply has neither Ok nor Err")
	}
	if into == nil {
		return nil
	}
	if err := json.Unmarshal(raw, into); err != nil {
		return fmt.Errorf("abi wire: %w", err)
	}
	return nil
}

// call encodes arg, calls the named import and decodes its reply.
func call[T any](imp func(uint64) uint64, arg any) (T, error) {
	var out T
	bytes, err := json.Marshal(arg)
	if err != nil {
		return out, fmt.Errorf("abi wire: %w", err)
	}
	reply, err := callHost(imp, bytes)
	if err != nil {
		return out, err
	}
	return decodeAs[T](reply)
}

// decodeAs decodes a reply into a fresh T. The error is taken before `out`
// is returned: Go leaves the order of `return out, f(&out)` unspecified and
// TinyGo reads `out` before f fills it.
func decodeAs[T any](reply []byte) (T, error) {
	var out T
	err := decodeReply(reply, &out)
	return out, err
}

type unit = json.RawMessage

// Log writes a line to cox's log (`tracing`), rate-limited.
func Log(level NoticeLevel, text string) error {
	_, err := call[unit](coxLog, PluginNotice{Level: level, Text: text})
	return err
}

// Notify shows a notice in the transcript, sanitized; not from `cox_render`.
func Notify(level NoticeLevel, text string) error {
	_, err := call[unit](coxNotify, PluginNotice{Level: level, Text: text})
	return err
}

// KVGet (capability `kv`) returns the stored value, or nil when there is none.
func KVGet(key string) (json.RawMessage, error) {
	raw, err := call[json.RawMessage](coxKVGet, key)
	if string(raw) == "null" {
		raw = nil
	}
	return raw, err
}

// KVPut (capability `kv`) stores value within the plugin's quota.
func KVPut(key string, value any) error {
	_, err := call[unit](coxKVPut, map[string]any{"key": key, "value": value})
	return err
}

// KVDelete (capability `kv`) removes key.
func KVDelete(key string) error {
	_, err := call[unit](coxKVDelete, key)
	return err
}

// Context (capability `context`) returns the host's event-folded snapshot.
func Context() (json.RawMessage, error) { return call[json.RawMessage](coxContext, nil) }

// InvokeTool (capability `invoke` lists name) runs a tool through hooks,
// the permission engine and the sandbox like a model call; returns a
// `ToolOutput`.
func InvokeTool(name string, input any) (json.RawMessage, error) {
	return call[json.RawMessage](coxInvokeTool, map[string]any{"name": name, "input": input})
}

// ModelCall (capability `model`) sends a `ModelCall` and returns its
// `ProviderEvent` array.
func ModelCall(modelCall any) ([]json.RawMessage, error) {
	return call[[]json.RawMessage](coxModelCall, modelCall)
}

// HTTP (capability `net`, or a provider host inside `cox_provider_stream`).
func HTTP(request HTTPReq) (HTTPResp, error) { return call[HTTPResp](coxHTTP, request) }

// Output writes one progress line, inside `cox_tool_call` only.
func Output(line string) error {
	_, err := call[unit](coxOutput, line)
	return err
}

// Cancelled reports whether the running tool call was cancelled.
func Cancelled() (bool, error) { return call[bool](coxCancelled, nil) }

// Redraw (any `ui` capability) marks the plugin's slots dirty.
func Redraw() error {
	_, err := call[unit](coxRedraw, nil)
	return err
}

// DesktopNotify (capability `desktop.notify`, `cox:desktop/v1`, PL§15.4)
// shows a notification whose buttons run the plugin's own commands. Off the
// desktop it fails with Kind `not_on_this_surface`: use Notify instead.
func DesktopNotify(notice DesktopNotice) error {
	_, err := call[unit](coxDesktopNotify, notice)
	return err
}

// SurfaceOf is the surface `init` says the session runs on. A cox older
// than PL§15 omits it and only loaded plugins into a terminal session.
func SurfaceOf(init InitIn) Surface {
	if init.Surface == "" {
		return SurfaceTerminal
	}
	return init.Surface
}
