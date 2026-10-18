package main

import (
	"encoding/json"
	"testing"
)

func TestDeclaresStatusResetAndTurnStarted(t *testing.T) {
	out, err := json.Marshal(declarations())
	if err != nil {
		t.Fatal(err)
	}
	want := `{"commands":[{"name":"reset","description":"Reset the turn and failure counters"}],"status":["status.right"],"subscribe":["turn_started"]}`
	if string(out) != want {
		t.Fatalf("got %s", out)
	}
}

func TestCountsOnlyTurnStartedEvents(t *testing.T) {
	events := []json.RawMessage{
		json.RawMessage(`{"type":"turn_started","seq":1}`),
		json.RawMessage(`{"type":"notice","text":"x"}`),
		json.RawMessage(`{"type":"turn_started","seq":2}`),
	}
	if n := turnsIn(events); n != 2 {
		t.Fatalf("got %d", n)
	}
}

func TestOnlyAnErrorResponseIsAFailure(t *testing.T) {
	if !isFailure(json.RawMessage(`{"tool_response":{"text":"no such file","is_error":true}}`)) {
		t.Fatal("an error response is a failure")
	}
	for _, ok := range []string{`{"tool_response":{"text":"x","is_error":false}}`, `{}`, `null`} {
		if isFailure(json.RawMessage(ok)) {
			t.Fatalf("%s is not a failure", ok)
		}
	}
}

func TestStatusWarnsOnceAToolFailed(t *testing.T) {
	out, _ := json.Marshal(status(3, 1))
	if string(out) != `{"text":[[{"text":"turns 3 · failed tools 1","style":"warn"}]]}` {
		t.Fatalf("got %s", out)
	}
	calm, _ := json.Marshal(status(3, 0))
	if string(calm) != `{"text":[[{"text":"turns 3 · failed tools 0","style":"dim"}]]}` {
		t.Fatalf("got %s", calm)
	}
}
