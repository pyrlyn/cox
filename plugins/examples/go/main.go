// The reference plugin PL§13 asks every guest language to implement, in Go
// over cox's Go SDK (plugins/sdk-go), matching plugins/examples/rust: it
// counts turns through a `turn_started` subscription, counts failed tool
// calls in a `PostToolUse`/`PostToolUseFailure` hook, shows both in a
// status segment and clears them with `/example-go:reset`. Both counters
// persist in the plugin's kv store, mirrored in memory for `cox_render`,
// which may not touch kv (PL§4).
//
// The decisions are plain functions so `go test` checks them on the host;
// the exports run only in the TinyGo wasip1 build
// (`just plugin-examples go`).
package main

import (
	"encoding/json"
	"fmt"

	cox "github.com/pyrlyn/cox/plugins/sdk-go"
)

const (
	turnsKey    = "turns"
	failuresKey = "failures"
)

// One instance serves the whole session, so these survive between calls.
var turns, failures uint64

func declarations() cox.InitOut {
	return cox.InitOut{
		Commands:  []cox.CommandDecl{{Name: "reset", Description: "Reset the turn and failure counters"}},
		Status:    []cox.Slot{cox.StatusRight},
		Subscribe: []string{"turn_started"},
	}
}

// turnsIn counts the turns started in one batch. The host sends only
// subscribed kinds, but a newer host may widen a batch, so the tag is
// checked here too.
func turnsIn(events []json.RawMessage) uint64 {
	var n uint64
	for _, raw := range events {
		var event struct {
			Type string `json:"type"`
		}
		if json.Unmarshal(raw, &event) == nil && event.Type == "turn_started" {
			n++
		}
	}
	return n
}

// isFailure reports whether a post-tool hook payload is a failed call.
func isFailure(payload json.RawMessage) bool {
	var hook struct {
		ToolResponse struct {
			IsError bool `json:"is_error"`
		} `json:"tool_response"`
	}
	return json.Unmarshal(payload, &hook) == nil && hook.ToolResponse.IsError
}

// status is dim when nothing failed, a warning colour otherwise.
func status(turns, failures uint64) cox.Widget {
	style := cox.StyleDim
	if failures > 0 {
		style = cox.StyleWarn
	}
	return cox.TextWidget([]cox.Span{{Text: fmt.Sprintf("turns %d · failed tools %d", turns, failures), Style: style}})
}

func stored(key string) (uint64, error) {
	raw, err := cox.KVGet(key)
	if err != nil || raw == nil {
		return 0, err
	}
	var n uint64
	// Not `return n, json.Unmarshal(raw, &n)`: TinyGo reads n first.
	err = json.Unmarshal(raw, &n)
	return n, err
}

func initPlugin(cox.InitIn) (cox.InitOut, error) {
	var err error
	if turns, err = stored(turnsKey); err != nil {
		return cox.InitOut{}, err
	}
	if failures, err = stored(failuresKey); err != nil {
		return cox.InitOut{}, err
	}
	return declarations(), nil
}

func onEvent(batch cox.EventBatch) (cox.Effects, error) {
	started := turnsIn(batch.Events)
	if started > 0 {
		if err := cox.KVPut(turnsKey, turns+started); err != nil {
			return cox.Effects{}, err
		}
		turns += started
	}
	return cox.Effects{Redraw: started > 0}, nil
}

func hook(call cox.HookCall) (any, error) {
	if isFailure(call.Payload) {
		if err := cox.KVPut(failuresKey, failures+1); err != nil {
			return nil, err
		}
		failures++
		// A refusal (the notice queue is full) only loses the notice; the
		// count is already stored, so the hook still succeeds.
		_ = cox.Notify(cox.Info, fmt.Sprintf("failed tool calls: %d", failures))
	}
	// Informational: a post-tool hook never changes the call.
	return map[string]string{"type": "continue"}, nil
}

func command(c cox.CommandIn) (cox.CommandOut, error) {
	if c.Name != "reset" {
		return cox.Nothing(), nil
	}
	for _, key := range []string{turnsKey, failuresKey} {
		if err := cox.KVDelete(key); err != nil {
			return nil, err
		}
	}
	turns, failures = 0, 0
	return cox.Notice(cox.Info, "counters reset"), nil
}

func render(cox.RenderIn) (cox.Widget, error) { return status(turns, failures), nil }

//go:wasmexport cox_init
func coxInit() int32 { return cox.Export(initPlugin) }

//go:wasmexport cox_on_event
func coxOnEvent() int32 { return cox.Export(onEvent) }

//go:wasmexport cox_hook
func coxHook() int32 { return cox.Export(hook) }

//go:wasmexport cox_command
func coxCommand() int32 { return cox.Export(command) }

//go:wasmexport cox_render
func coxRender() int32 { return cox.Export(render) }

// TinyGo's c-shared build never runs main; the host calls the exports.
func main() {}
