package cox

import (
	"encoding/json"
	"errors"
	"strings"
	"testing"
)

func jsonOf(t *testing.T, v any) string {
	t.Helper()
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func TestHostRefusalDecodesAsHostError(t *testing.T) {
	err := decodeReply([]byte(`{"Err":{"kind":"not_granted","capability":"kv"}}`), new(json.RawMessage))
	var hostErr *HostError
	if !errors.As(err, &hostErr) || hostErr.Kind != "not_granted" || hostErr.Capability != "kv" {
		t.Fatalf("want a not_granted HostError, got %v", err)
	}
	if got := hostErr.Error(); got != "cox host call failed: not_granted: kv" {
		t.Fatalf("message: %q", got)
	}
	err = decodeReply([]byte(`{"Err":{"kind":"not_on_this_surface","surface":"terminal"}}`), nil)
	if !errors.As(err, &hostErr) || hostErr.Surface != SurfaceTerminal {
		t.Fatalf("want a surface refusal, got %v", err)
	}
}

func TestHostSuccessDecodesOkPayload(t *testing.T) {
	var cancelled bool
	if err := decodeReply([]byte(`{"Ok":true}`), &cancelled); err != nil || !cancelled {
		t.Fatalf("got %v, %v", cancelled, err)
	}
	if err := decodeReply([]byte(`{"Ok":null}`), new(json.RawMessage)); err != nil {
		t.Fatalf("unit ok: %v", err)
	}
}

// Run under `tinygo test` too: TinyGo evaluated `return out, f(&out)` with
// the zero `out`, so every typed host reply read as empty.
func TestTypedReplyCarriesTheDecodedValue(t *testing.T) {
	raw, err := decodeAs[json.RawMessage]([]byte(`{"Ok":5}`))
	if err != nil || string(raw) != "5" {
		t.Fatalf("got %q, %v", raw, err)
	}
	cancelled, err := decodeAs[bool]([]byte(`{"Ok":true}`))
	if err != nil || !cancelled {
		t.Fatalf("got %v, %v", cancelled, err)
	}
}

func TestMalformedReplyIsAWireError(t *testing.T) {
	for _, reply := range []string{`true`, `{}`, `not json`} {
		err := decodeReply([]byte(reply), new(bool))
		var hostErr *HostError
		if err == nil || errors.As(err, &hostErr) || !strings.HasPrefix(err.Error(), "abi wire") {
			t.Fatalf("%s: want a wire error, got %v", reply, err)
		}
	}
}

func TestEmptyInputReachesAUnitExport(t *testing.T) {
	out, err := runExport([]byte{}, func(in json.RawMessage) (any, error) { return nil, nil })
	if err != nil || string(out) != "null" {
		t.Fatalf("got %s, %v", out, err)
	}
}

func TestExportRoundTripsTypedPayloads(t *testing.T) {
	out, err := runExport([]byte(`{"name":"reset","args":""}`), func(c CommandIn) (CommandOut, error) {
		return Prompt(c.Name), nil
	})
	if err != nil || string(out) != `{"kind":"prompt","text":"reset"}` {
		t.Fatalf("got %s, %v", out, err)
	}
}

func TestHandlerAndInputErrorsBecomeTheCallError(t *testing.T) {
	_, err := runExport([]byte(`{}`), func(json.RawMessage) (any, error) { return nil, errors.New("boom") })
	if err == nil || err.Error() != "boom" {
		t.Fatalf("got %v", err)
	}
	_, err = runExport([]byte(`not json`), func(json.RawMessage) (any, error) { return nil, nil })
	if err == nil || !strings.HasPrefix(err.Error(), "bad input") {
		t.Fatalf("got %v", err)
	}
}

// The host reads a missing list as empty but refuses `null`, which is what
// Go writes for a nil slice without `omitempty`.
func TestEmptyInitOutSendsNoNullLists(t *testing.T) {
	if got := jsonOf(t, InitOut{}); got != `{}` {
		t.Fatalf("got %s", got)
	}
	got := jsonOf(t, InitOut{Status: []Slot{StatusRight}, Subscribe: []string{"turn_started"}})
	if got != `{"status":["status.right"],"subscribe":["turn_started"]}` {
		t.Fatalf("got %s", got)
	}
}

func TestCommandOutKeepsEachKindsFields(t *testing.T) {
	cases := map[string]CommandOut{
		`{"kind":"nothing"}`:                           Nothing(),
		`{"kind":"notice","level":"info","text":"ok"}`: Notice(Info, "ok"),
		`{"kind":"compact"}`:                           Compact(""),
		`{"focus":"tests","kind":"compact"}`:           Compact("tests"),
	}
	for want, out := range cases {
		if got := jsonOf(t, out); got != want {
			t.Fatalf("got %s, want %s", got, want)
		}
	}
}

func TestTextWidgetIsPlainLines(t *testing.T) {
	got := jsonOf(t, TextWidget([]Span{{Text: "turns 1", Style: StyleDim}}))
	if got != `{"text":[[{"text":"turns 1","style":"dim"}]]}` {
		t.Fatalf("got %s", got)
	}
	if got := jsonOf(t, TextWidget()); got != `{"text":[]}` {
		t.Fatalf("empty: %s", got)
	}
}

func TestSurfaceIsTerminalWhenTheHostDoesNotSay(t *testing.T) {
	var init InitIn
	if err := json.Unmarshal([]byte(`{"api":1,"plugin_id":"p","config":null,"session":{"id":"s","cwd":"/w"},"granted":{}}`), &init); err != nil {
		t.Fatal(err)
	}
	if SurfaceOf(init) != SurfaceTerminal {
		t.Fatalf("got %q", SurfaceOf(init))
	}
	init.Surface = SurfaceDesktop
	if SurfaceOf(init) != SurfaceDesktop {
		t.Fatalf("got %q", SurfaceOf(init))
	}
}

func TestHostFunctionsRefuseOffWasm(t *testing.T) {
	if err := Redraw(); !errors.Is(err, ErrNotWasm) {
		t.Fatalf("got %v", err)
	}
	if err := DesktopNotify(DesktopNotice{Level: Info, Title: "t"}); !errors.Is(err, ErrNotWasm) {
		t.Fatalf("got %v", err)
	}
	if _, err := KVGet("turns"); !errors.Is(err, ErrNotWasm) {
		t.Fatalf("got %v", err)
	}
}
