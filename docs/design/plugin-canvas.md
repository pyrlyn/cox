//! Plugin canvas: a desktop pane where a plugin shows pages it draws as static HTML,
//! and where the person selects an element to hand to the agent. Design only (A156, T33.47).

# Design: plugin canvas (A156, phase P33)

## Problem

A plugin that produces UI (Weft's cox plugin, `github.com/listepo/weft`, `plugins/cox`: an agent authors app screens in Weft markup) can return a screen to the model as text and as an HTML page in a tool result, but the person never sees the screen drawn. Claude Design's canvas is the model: the agent's screens are drawn as real UI beside the conversation, they redraw as the agent edits them, and a click on an element tells the agent which one "this" is.

The desktop plugin API (PL§15.4) is declarative widgets only. It lists "plugin-supplied SwiftUI, HTML or web views" as never offered, marked open (A134). The creator approved this feature through Weft's T106.6 on the condition that the guards hold: plugin and model content is untrusted, it is shown sandboxed, with no network and no script from that content, behind its own grant line. This document defines that narrow kind of web view: static pages, page JavaScript off, and the only script a host-owned one.

## The field

- **cox's browser pane** (T51.9, T51.10, `desktop/macos/Packages/CoxPlatform/Sources/CoxPlatform/BrowserController.swift`): a WebKit `WebPage` with a non-persistent data store and no bridge back to the app (DT§10). It loads the network on purpose; the canvas must not.
- **MCP Apps** (`v0.3-mcp-apps.md`, A123): cox shows only the text result of a tool that ships a `ui://` resource. That path stays closed; this one is for granted plugins, whose code the person approved by digest.
- **Claude Design** shows the agent's designs on a canvas and lets the person point at parts of them. cox needs the drawing and the pointing, not the editing (§10, question 4).

## Decision

A desktop-only contribution, the **canvas**: one pane beside the transcript, like the browser pane, holding the *frames* one plugin draws. Each frame is one static HTML page in its own isolated web view. A click selects an element. The plugin turns the selection into text, and the app puts that text in the composer as an attachment the person sends or removes.

## 1. Manifest and grant

```toml
[capabilities.desktop]
canvas = true
```

- `DesktopCaps.canvas: bool` (`crates/cox-plugin-api/src/manifest.rs`), grant line `desktop.canvas` (`crates/cox-plugin/src/grant.rs`), shown "desktop only" in the grant prompt like the other `desktop.*` lines (PL§15.5).
- `desktop.canvas` also allows `cox_redraw()` (PL§4 lists it under "any `ui`"), since a canvas redraws on the same signal.
- A manifest whose `surfaces` excludes `desktop` and asks for `canvas` is an error (PL§15.5).

## 2. ABI

All additions are minor (PL§4 "Versioning"): `api` stays 1, an older host ignores the new `InitOut` field and never calls the new exports.

| Item | Shape | Notes |
| --- | --- | --- |
| `InitOut.desktop.canvas` | `Option<CanvasDecl { title, symbol, select_attr }>` | `title` ≤ 64 chars; `symbol` as PL§15.4; `select_attr` matches `^data-[a-z0-9-]{1,48}$` |
| export `cox_canvas` | `CanvasIn { width_pt, height_pt }` → `CanvasOut { frames: Vec<Frame> }` | `Frame { id, title, width_pt, html }`; called only while the pane is visible |
| export `cox_canvas_select` | `CanvasSelect { frame, value }` → `Option<CanvasAttach { name, text }>` | `value` is the selection attribute's value |
| `CommandOut::OpenCanvas` | unit variant | shows the pane on a person's command, like `OpenInspector` |

**Caps** are checked in `cox-plugin-api` (one function both the host and the app-server client call), and anything over them is dropped with one `Notice(Warn)`:

- at most 12 frames; `id` matches `^[a-z0-9_.-]{1,64}$` and is unique; `title` ≤ 80 chars after `cox_sanitize::sanitize`;
- `html` ≤ 1 MiB per frame and 4 MiB per `CanvasOut`; `width_pt` clamped to 200–2560;
- `CanvasSelect.value` ≤ 256 chars, no control characters, else the click is dropped before the plugin sees it;
- `CanvasAttach.name` ≤ 80 chars and `text` ≤ 8 KiB, both sanitized.

**Deadlines.** `cox_canvas` runs on the control queue within `min(limits.call_ms, 2000 ms)`, not render's 20 ms: drawing a page can be real work, and it never blocks the UI thread, because the app keeps the last frames until the answer arrives. `cox_canvas_select` has `min(limits.call_ms, 500 ms)`. Three misses in a row stop the canvas for the session with a notice (PL§8, D14). Both run in "render" context for host functions: read-only, no `cox_notify`, no kv writes, no tools.

## 3. The pane

- Beside the transcript, like the browser pane; ⌘⇧D shows and hides it. The plugin's symbol and title head it, plus a picker when more than one plugin declares a canvas. A palette action "Show <title>" and `CommandOut::OpenCanvas` open it.
- Frames sit side by side in a scroll view the person can zoom (pinch, ⌘+/⌘− while the pane has focus), each at its `width_pt`, its height that of its content, with the title above it.
- Appearance: the web views take the window's appearance, so a page written with `prefers-color-scheme` switches with the app. The host passes no appearance field.
- VoiceOver reads each frame as a web area: WebKit's own accessibility, no extra code.
- Not in the transcript, the rollout or the model's context: the canvas is a view, and only an attachment the person sends reaches the model.

## 4. Isolation

The HTML is untrusted and is not filtered as a string: no HTML sanitizer, which would be a second, weaker boundary. WebKit enforces the boundary, set up in one place (`CanvasPage` in CoxPlatform) and proved by tests:

1. **No page script.** `allowsContentJavaScript = false` on the page's navigation preferences. WebKit's header (`Source/WebKit/UIProcess/API/Cocoa/WKWebpagePreferences.h`, `https://github.com/WebKit/WebKit`, main, checked 2026-10-14): with it off, inline `<script>`, external scripts and `javascript:` URLs do not run, while the app's own `WKUserScript`s and scripts evaluated in a content world still do.
2. **No network.** A `WKContentRuleList` with a single rule, trigger `url-filter: ".*"` and action `block`, added to the page's user content controller. WebKit's content-blocker format (`https://webkit.org/blog/3476/content-blockers-first-look/`, checked 2026-10-14) evaluates `url-filter` against the URL of every resource load, and `block` aborts the load. Images, fonts, stylesheets and frames from anywhere, `data:` included, are never fetched; an `<img>` shows its `alt`.
3. **No navigation.** The page's navigation decider allows exactly the first `load(html:baseURL:)` with `about:blank` and refuses every other navigation: links, forms, `<meta http-equiv="refresh">` and frames.
4. **No state, no bridge.** A non-persistent data store per page, as the browser pane has (DT§10), and no script message handler in the page world.
5. **Host script in its own world.** One `WKUserScript` in a named `WKContentWorld` ("cox-canvas"), with the selection message handler added for that world only. WebKit's `WKUserContentController.h` (same repository and date) says a handler added with `addScriptMessageHandler:contentWorld:name:` is available in the given world. The script outlines the hovered element and the selected one, cancels the default action of a click, and posts `{ value }`, the `select_attr` value of the nearest ancestor that carries it. The page shares the DOM with that world but has no script running to read the outline or forge a post.

The SwiftUI `WebPage` configuration exposes the user content controller (the browser pane uses it) and a navigation decider. The tests in T33.47.6 are the proof on the shipped macOS, not this paragraph: a page `<script>` does not run, an `https` image and a `data:` image are not loaded, a link click does not navigate, and the host script's post arrives with the value.

## 5. Selection

1. The host script posts `{ value }`. Swift adds the frame id, which it owns, and calls `canvas_select(frame, value)` through `cox-ffi`.
2. `cox-app` checks the frame is one the plugin drew last, checks `value` (§2 caps) and calls `cox_canvas_select`.
3. A returned `CanvasAttach` goes back to Swift, which adds it to the composer's draft as an `Attachment` with media type `text/plain` and a chip showing `name`. The person sees it, can remove it, and sends it with their next turn. cox-core already gives a text attachment to the model as `<attachment name=… media_type=…>` (`crates/cox-core/src/context.rs`).
4. A new selection replaces the previous chip from the same canvas, so stray clicks do not pile up. `None` adds nothing.

Why not a `UserPromptSubmit` hook rewriting the prompt: the person would not see what was added until after sending. Why not a turn per click: a click must never spend tokens.

## 6. Live updates

The PL§8 redraw model: `cox_canvas` runs when the pane becomes visible, when the plugin calls `cox_redraw()` (from a tool call, an event or a command), and when the pane's size changes by more than a frame's clamp. The app compares each frame's HTML with the last one and reloads only the frames that changed. A reloaded frame keeps its scroll offset and re-applies the outline when the selected value is still there. While the pane is hidden nothing renders.

## 7. Surfaces and remote sessions

- Terminal, headless and ACP: not offered (PL§15.6). `desktop.canvas` is ignored there, and a plugin's text output stays the fallback.
- A remote session (`cox app-server --stdio`, PL§15.1): `CanvasOut` crosses the wire as `ServerEvent::PluginCanvas`, and a selection as `Call::CanvasSelect` answered by `Reply::CanvasAttach`. The server applies the caps before sending and the client applies them again, because the wire is a trust boundary too. `docs/app-server.schema.json` is regenerated.

## 8. Where the code lives

| Crate or package | Gains |
| --- | --- |
| `cox-plugin-api` | `DesktopCaps.canvas`, `CanvasDecl`, `CanvasIn`, `CanvasOut`, `Frame`, `CanvasSelect`, `CanvasAttach`, `CommandOut::OpenCanvas`, the caps check; regenerated `docs/plugin.schema.json` and `docs/plugin-abi.schema.json` |
| `cox-plugin` | the `desktop.canvas` line; the two calls with their deadlines; the `InitOut.desktop` filter; `cox_redraw` under the canvas grant |
| `plugins/sdk` | `register!` arms `canvas` and `canvas_select` |
| `cox-app` | canvas state in `plugin_ui` (visible, last frames, redraw, select, `OpenCanvas`), `LiveSession` methods, the wire payloads and `RemoteSession` |
| `cox-ffi` | `CanvasView`, `show_canvas`, `canvas_select` on the local and remote sessions |
| CoxPlatform | `CanvasPage`: the configuration of §4, `load(html:)`, the selection stream |
| CoxUI and App | `PluginCanvasPane`, ⌘⇧D, the palette action, the composer chip |

No new dependency: WebKit is a system framework the app already links (CoxPlatform's `Package.swift`).

## 9. Falsifiers

1. A frame runs a page script, loads anything, or navigates: the isolation failed; the canvas is switched off until fixed.
2. A selection reaches the model without the person sending it: the design failed; the attachment path is the only route.
3. The canvas slows the transcript: `cox_canvas` must never run on the main actor, and frame reloads must not block typing. Measured against DT§1's budget (no hitch over 16 ms while streaming) with the SwiftUI Instruments template on the T33.47.7 build while a fixture plugin redraws twice a second; `just desktop-bench` (T37.33) is on hold, so this is by hand.

## 10. Open questions for the creator

Each carries a recommendation. The defaults above follow the recommendations until the creator answers.

1. **Placement.** A pane beside the transcript (recommended: the inspector is 320 pt, too narrow for a screen), an inspector tab, or a separate window.
2. **Selection to the agent.** A visible composer attachment (recommended), a `UserPromptSubmit` rewrite, or a turn per click (no).
3. **Remote images.** Blocked, showing `alt` (recommended: the canvas never touches the network), or loaded from the hosts the plugin's `net` grant lists.
4. **Editing on the canvas** (drag, resize, inline text). Out of this design (recommended: selection first; editing needs patches flowing from the canvas to the plugin and gets its own design).

## 11. Plan amendment (A156) and tasks

A156 §3 P33: new T33.47 and T33.47.1–T33.47.8. PL§15.4 changes "plugin-supplied … HTML or web views — never" into the canvas row and its rules, PL§15.5 gets `canvas`, PL§14 gets decision 17, DT§5.1, §5.5 and §10 get the pane, the shortcut and the isolation paragraph. Each task stays within 200 lines and three files (AGENTS.md), generated schemas aside.

| Task | Files | Check |
| --- | --- | --- |
| T33.47.1 API: manifest, grant line, ABI types and caps | `cox-plugin-api` `manifest.rs`, `abi.rs`; `cox-plugin` `grant.rs` (PL§ text, schemas) | caps unit tests (count, size, id, attr, value); `grant_new_canvas_needs_approval`; schema drift tests |
| T33.47.2 Host calls and SDK | `cox-plugin` `live.rs`, `hostfn.rs`; `plugins/sdk/src/lib.rs` | a fixture plugin's frames come back capped; a slow `cox_canvas` misses three times and stops; `cox_redraw` allowed under `desktop.canvas` |
| T33.47.3 cox-app canvas state | `cox-app` `plugin_ui.rs`, `live.rs` | redraw only while visible; unchanged frames not re-sent; a select on an unknown frame is dropped; the attachment is sanitized and capped |
| T33.47.4 Wire and remote | `cox-app` `wire.rs`, `remote.rs` (schema) | `wire_round_trips_every_message`; the client re-applies caps to an over-size `PluginCanvas` |
| T33.47.5 FFI | `cox-ffi` `session.rs`, `remote.rs`, `types.rs` | the FFI mirrors list the new shapes; a local and a remote select round-trip |
| T33.47.6 `CanvasPage` | CoxPlatform `CanvasPage.swift`, `CanvasPageTests.swift` | no page script, no `https:` or `data:` load, no navigation, the host script's post arrives (§4) |
| T33.47.7 Pane, shortcut, chip | CoxUI `PluginCanvasPane.swift`; App `SessionWindow.swift`, `SessionCanvas.swift` (DT§ text) | snapshot of the pane with two frames; ⌘⇧D toggles; a selection adds one replaceable chip |
| T33.47.8 User docs | `docs/plugins.md` (and the `ru`/`uk` pages when T33.45.10 has made them) | the canvas section; the API-reference drift test |
