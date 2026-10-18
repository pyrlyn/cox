package cox

// The ABI v1 payloads a plugin uses most, mirroring `cox-plugin-api`
// (docs/plugin-abi.schema.json). Lists are `omitempty` because the host
// reads a missing list as empty but refuses `null`. Payloads that carry a
// cox protocol type (`Event`, `HookOutcome`, `ToolSpec`, `ToolOutput`,
// `ProviderEvent`) stay json.RawMessage; their shapes are in
// docs/protocol.jsonschema.

import "encoding/json"

// NoticeLevel is a plugin notice's severity, capped at warn.
type NoticeLevel string

const (
	Info NoticeLevel = "info"
	Warn NoticeLevel = "warn"
)

// Slot is a UI slot (PL§8, PL§15.4).
type Slot string

const (
	StatusLeft       Slot = "status.left"
	StatusRight      Slot = "status.right"
	Panel            Slot = "panel"
	Overlay          Slot = "overlay"
	DesktopInspector Slot = "desktop.inspector"
)

// Surface is who drives the session (PL§15.1).
type Surface string

const (
	SurfaceTerminal Surface = "terminal"
	SurfaceDesktop  Surface = "desktop"
	SurfaceHeadless Surface = "headless"
	SurfaceACP      Surface = "acp"
)

// StyleToken is a theme colour role for a Span ("text", "dim", "accent",
// "warn", "error", "ok", … — the full list is the schema's StyleToken).
type StyleToken string

const (
	StyleText  StyleToken = "text"
	StyleDim   StyleToken = "dim"
	StyleWarn  StyleToken = "warn"
	StyleError StyleToken = "error"
	StyleOK    StyleToken = "ok"
)

// SessionInfo is the session a plugin is initialised for.
type SessionInfo struct {
	ID  string `json:"id"`
	Cwd string `json:"cwd"`
}

// InitIn is `cox_init`'s input.
type InitIn struct {
	API      uint32          `json:"api"`
	PluginID string          `json:"plugin_id"`
	Config   json.RawMessage `json:"config"`
	Session  SessionInfo     `json:"session"`
	Granted  json.RawMessage `json:"granted"`
	Surface  Surface         `json:"surface,omitempty"`
}

// InitOut is `cox_init`'s output: what the plugin contributes.
type InitOut struct {
	Tools     []json.RawMessage `json:"tools,omitempty"`
	Commands  []CommandDecl     `json:"commands,omitempty"`
	Keys      []KeyDecl         `json:"keys,omitempty"`
	Status    []Slot            `json:"status,omitempty"`
	Panels    []Slot            `json:"panels,omitempty"`
	Renderers []string          `json:"renderers,omitempty"`
	Subscribe []string          `json:"subscribe,omitempty"`
	Desktop   json.RawMessage   `json:"desktop,omitempty"`
}

// CommandDecl is a slash command, shown as `/<id>:<name>`.
type CommandDecl struct {
	Name        string `json:"name"`
	Description string `json:"description,omitempty"`
}

// KeyDecl is a key under the plugin leader; pressing it calls `cox_key`.
type KeyDecl struct {
	Key         string `json:"key"`
	Name        string `json:"name"`
	Description string `json:"description,omitempty"`
}

// EventBatch is `cox_on_event`'s input (PL§5).
type EventBatch struct {
	FirstSeq uint64            `json:"first_seq"`
	Dropped  uint64            `json:"dropped"`
	Events   []json.RawMessage `json:"events"`
}

// Effects is `cox_on_event`'s output.
type Effects struct {
	Redraw  bool           `json:"redraw,omitempty"`
	Notices []PluginNotice `json:"notices,omitempty"`
}

// PluginNotice is a notice from a plugin.
type PluginNotice struct {
	Level NoticeLevel `json:"level"`
	Text  string      `json:"text"`
}

// HookCall is `cox_hook`'s input (PL§6); the output is a `HookOutcome`
// such as `{"type": "continue"}`.
type HookCall struct {
	Event   json.RawMessage `json:"event"`
	Payload json.RawMessage `json:"payload"`
}

// ToolCallIn is the input of `cox_tool_subject`, `cox_tool_risk` and
// `cox_tool_call`.
type ToolCallIn struct {
	Name  string          `json:"name"`
	Input json.RawMessage `json:"input"`
}

// CommandIn is the input of `cox_command` and `cox_key`.
type CommandIn struct {
	Name string `json:"name"`
	Args string `json:"args"`
}

// CommandOut is `cox_command`'s / `cox_key`'s output; build it with the
// functions below, which keep each kind's required fields.
type CommandOut map[string]any

func Nothing() CommandOut           { return CommandOut{"kind": "nothing"} }
func Prompt(text string) CommandOut { return CommandOut{"kind": "prompt", "text": text} }
func TogglePanel() CommandOut       { return CommandOut{"kind": "toggle_panel"} }
func OpenOverlay() CommandOut       { return CommandOut{"kind": "open_overlay"} }
func OpenInspector() CommandOut     { return CommandOut{"kind": "open_inspector"} }

// Compact asks for a compaction now; an empty focus means none.
func Compact(focus string) CommandOut {
	if focus == "" {
		return CommandOut{"kind": "compact"}
	}
	return CommandOut{"kind": "compact", "focus": focus}
}

func Notice(level NoticeLevel, text string) CommandOut {
	return CommandOut{"kind": "notice", "level": level, "text": text}
}

// RenderIn is `cox_render`'s input; the output is a Widget.
type RenderIn struct {
	Slot   Slot   `json:"slot"`
	Width  uint16 `json:"width"`
	Height uint16 `json:"height"`
}

// Span is a run of text with one style.
type Span struct {
	Text   string     `json:"text"`
	Style  StyleToken `json:"style,omitempty"`
	Bold   bool       `json:"bold,omitempty"`
	Italic bool       `json:"italic,omitempty"`
	Link   string     `json:"link,omitempty"`
}

// Widget is a `Widget` value; TextWidget builds the plain-lines one, and a
// json.RawMessage carries the others (list, image, …).
type Widget = any

// TextWidget is the `text` widget: plain lines of spans.
func TextWidget(lines ...[]Span) Widget {
	if lines == nil {
		lines = [][]Span{}
	}
	return map[string][][]Span{"text": lines}
}

// HTTPReq is `cox_http`'s input.
type HTTPReq struct {
	Method  string            `json:"method"`
	URL     string            `json:"url"`
	Headers map[string]string `json:"headers,omitempty"`
	Body    *string           `json:"body,omitempty"`
}

// HTTPResp is `cox_http`'s output.
type HTTPResp struct {
	Status  uint16            `json:"status"`
	Headers map[string]string `json:"headers"`
	Body    string            `json:"body"`
}

// DesktopNotice is `cox_desktop_notify`'s input (PL§15.4).
type DesktopNotice struct {
	Level   NoticeLevel    `json:"level"`
	Title   string         `json:"title"`
	Body    string         `json:"body,omitempty"`
	Actions []NoticeAction `json:"actions,omitempty"`
}

// NoticeAction is a button on a DesktopNotice; Command names one of the
// plugin's own commands.
type NoticeAction struct {
	Label   string `json:"label"`
	Command string `json:"command"`
}
