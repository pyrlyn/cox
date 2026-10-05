# cox for macOS — design system

The one reference for how the desktop app looks and how its SwiftUI is built. It is written for two
readers: a person reviewing a screen and an agent generating one. If a screen needs something this file
does not have, add it here first (token, component or variant), then build the screen.

- **Source of truth for values:** `tokens/*.json` (W3C Design Tokens Format Module 2025.10). This file
  explains them; it never restates a value the JSON holds, except in the tables marked *mirror*.
- **Source of truth for looks:** `mockups/mockups.html`; `mockups/render.sh <screen-id>…` renders PNGs into `mockups/screens/` (not committed). The glass main screen
  is `28-main-glass-frosted`, `29-main-glass-glossy`, `30-main-glass-tokens`; in dark,
  `31-main-glass-dark-frosted` and `32-main-glass-dark-glossy`.
- **Behaviour and architecture:** `docs/design/desktop.md` (cited as DT§n). This file covers only the view layer.

## 1. Principles

1. **Views are dumb.** A SwiftUI view renders a value and reports intents. Business logic, formatting of
   numbers and text, and every decision about *what* to show live in Rust (`cox-app`) and reach Swift as
   view state through `CoxModel`. A view never calls FFI, never reads config, never opens a file.
2. **Tokens, never literals.** No colour, size, font, radius, shadow, duration or opacity is written as a
   number or literal in a view. Enforced by lint (§9).
3. **Decompose to reuse.** Every visual element is the smallest component that has a name in §6, built
   from the ones below it. Two screens that show the same thing use the same component; a difference is a
   *variant* parameter, never a copy.
4. **Depth has meaning.** Elevation says what is on top of what: prose is flat, cards are lifted, the
   composer floats highest in a pane, popovers float over panes, the window floats over the wallpaper.
   Never lift something to decorate it.
5. **Glass is for chrome, not for reading.** Transparency applies to the window and panes. Anything that
   carries text a person must read stays at least `material.readableFloor` opaque at every setting.
6. **The system wins.** Reduce Transparency forces Solid; Reduce Motion drops movement to cross-fades;
   Increase Contrast switches to high-contrast colour variants, drops the specular sweep and makes glass
   more opaque (§3.5); the accent follows the system accent.

## 2. Token pipeline

```
design/tokens/color.light.json ┐
design/tokens/color.dark.json  ├─ Style Dictionary 5.x ─┬─ CoxUI/Tokens/Colors.xcassets  (colorsets: Any / Dark / High Contrast)
design/tokens/base.json        ┘                        ├─ CoxUI/Tokens/Tokens.swift     (Space, Radius, Size, Font, Elevation, Motion, Material)
                                                        └─ design/tokens/tokens.css     (the HTML mockups read the same values)
```

- Format: DTCG 2025.10 — `$type`/`$value`; colours as `{colorSpace, components, alpha, hex}`; dimensions
  as `{value, unit: "px"}` (1 px = 1 pt on macOS); shadows as arrays of layers with `inset`.
  Source: https://www.designtokens.org/tr/2025.10/format/ (checked 2026-09-28).
- Generator: Style Dictionary (npm `style-dictionary` 5.5.5, released 2026-09-20; checked in the npm
  registry 2026-09-28), pinned with its lockfile in `package.json`; `style-dictionary.config.mjs` holds
  the build. Built-in formats do not cover DTCG composite values or colorsets, so the Swift and CSS
  outputs are custom formats and the colorsets a custom action. `just desktop-tokens` runs it.
- Each `color.<mode>.json` holds every colour role for one appearance: `light` (Any) and `dark` are
  required; `light-hc` and `dark-hc` are optional and add the High Contrast entries. A token that also
  has children (`accent` and `accent.soft`) is the group's `$root` token (DTCG §6.2).
- Colours become asset-catalog colours; the build generates a `ColorResource` per colorset, so a view
  writes `Color(.accent)`, `Color(.surfaceWindow)` and so on (SwiftPM generates no `Color.<name>`
  extensions). Light, dark and high-contrast variants live in one colorset.
- `Tokens.swift` flattens each group's path to camelCase: `Space.m`, `Radius.pane`, `Size.readingWidth`,
  `Motion.durationFast`, `Motion.easingStandard` (a `UnitCurve`), `MaterialToken.frostedBlur`, and the
  values `FontToken.transcriptH3` and `ElevationToken.e2` that `.textStyle(_:)` and `.elevation(_:)`
  take. Font, Material and Elevation carry a `Token` suffix so they do not shadow SwiftUI's `Font` and
  `Material` or the `Elevation` modifier.
- A drift test (the `desktop-tokens` CI job) regenerates the outputs and fails on any diff, like
  `docs/config.jsonschema`. Edit the JSON, never a generated file.

### Figma mirror

The Figma file [cox desktop](https://www.figma.com/design/KA9a0R7n6P0QbwDn92e167) mirrors the tokens
and the mockups for review. It is a mirror, not a source (A114): nothing here reads it back, and a
change made only in Figma is lost at the next re-sync. Change `tokens/*.json` or `mockups.html`, then
re-sync.

- **Variables and styles.** Collections `Color` (modes Light, Dark, Light HC, Dark HC), `Spacing`,
  `Radius`, `Size`, `Type`, `Motion` and `Material`; text styles `font/*` and effect styles
  `elevation/*`, each variable carrying its CSS name as web code syntax. `figma/variables.mjs` builds
  them from the JSON (`node --test 'figma/*.test.mjs'` covers it).
- **Screens.** One page per mockup group holds each rendered screen as a 1520 × 980 frame named by
  its screen id. `Screen 28 · editable` rebuilds `28-main-glass-frosted` as auto-layout layers bound
  to the variables, derived from the mockup; values without a token (glass material fills, the
  wallpaper, the traffic lights) stay raw.
- **Fonts.** The Figma file uses stand-ins (A125): Inter for SF Pro, Roboto Mono for SF Mono, at the
  token's weight and size. `use_figma` lists SF Pro but does not render it, does not list SF Mono,
  and shared fonts need an Organization plan. The product, CoxUI and the HTML renders use SF, so
  compare type metrics against the renders, not Figma. The generator still reports any font Figma
  lacks or cannot render (`missingFonts`, `unrenderedFonts`).

Re-sync:

1. `npm run figma -- --out <dir>` writes the scripts; run each `<dir>/NN.js` in order with the Figma
   MCP tool `use_figma`. Each is idempotent: it updates by name and prunes what the tokens dropped.
2. `mockups/render.sh <screen-id>…` re-renders the PNGs; upload each with the Figma MCP tool
   `upload_assets` as the fill of the frame with the same name.

Pixel diff (A119). `npm run diff -- <snapshot.png> <frame.png> [--out diff.png]` compares a CoxUI
snapshot with its frame (a Figma export or `mockups/screens/<id>.png`) and prints the share of
differing pixels and the largest differing regions (`x,y width×height`, largest box first, in the
snapshot's pixels), so spacing, colour and type drift shows up without comparing by eye. The frame is
resampled to the snapshot's size (`--scale-to frame` goes the other way) and a warning names an
aspect mismatch. Snapshots include a 20 pt margin of wallpaper and mockups a 40 px one at 2x, so cut
the window out of both: for the main screen `--crop-snapshot 20,20,1440,900 --crop-frame
80,80,2880,1800`. `--threshold` (pixelmatch, default 0.1) is the per-pixel colour tolerance; raise it
to about 0.3 to see layout and type through a glass tint that differs everywhere. The diff image (red
= differs) goes to `diff/out/`, which is not committed; `--json` prints the report as JSON. It uses
`pixelmatch` 7.2.0 (released 2026-04-29) and `sharp` 0.35.5 (released 2026-09-27), both checked in
the npm registry 2026-09-28.

## 3. Foundations

### 3.1 Colour — semantic roles

Name by role, never by hue. A view asks for `text.secondary`, not "grey".

| Group | Tokens | Use |
| --- | --- | --- |
| surface | `window`, `sidebar`, `code`, `capsule`, `capsuleBorder`, `popover`, `terminal` | Backgrounds, by layer |
| glass | `fill`, `border`, `highlight` | A glass pane's tint, rim and top-edge highlight (§3.5) |
| fill | `primary`, `secondary` | Quiet fills inside a surface |
| text | `primary`, `secondary`, `tertiary`, `placeholder`, `terminal`, `terminalOk`, `onAccent` | Foregrounds; `placeholder` is the hint in an empty field, stronger than `tertiary` so it holds §8 on glass (A112); `onAccent` is the label on an accent or status fill (primary button, Bypass segment), white, and High Contrast holds it to 7:1 on those fills |
| line | `separator` | 0.5 pt hairlines |
| quote | `bar` | A quote's bar in the transcript, one per depth, `size.quoteBar` (3 pt) wide: `text.tertiary`'s value, stronger than a hairline; High Contrast holds it to 3:1 (A97) |
| intent | `accent`, `accent.soft`, `accent.selected`, `status.success/warning/danger/plan` (+ `Soft`) | Meaning: selection, done, needs you, error, plan mode; `accent.selected` fills the selected session row only, paler than `accent.soft` so `text.primary` and `text.secondary` hold §8 on it in every material (A115) |
| role | `role.project` (+ `Soft`) | The project config layer's badge: the mockup's purple, lightened in dark mode to hold 4.5:1 (§8) |
| risk | `risk.low/medium/high` (+ `Soft`) | A RiskChip's label and face per level: low quiet (`text.secondary` on `fill.secondary`), medium the mockup's `.risk` orange, high `status.danger`; their own roles so a level can change without moving the status colours |
| diff | `add`, `addGutter`, `del`, `delGutter` | Diff lines and gutters |
| syntax | `keyword`, `string`, `number`, `function`, `comment`, `type` | Highlighting; same roles as `cox-render`'s `StyleToken`, so TUI and app match |
| data | `meter.sent`, `meter.received`, `context.system/tools/instructions/history` | Token meter and context bar |
| tile | `tile.<kind>.top/bottom/glyph` for `neutral`, `edit`, `shell`, `search`, `write`; `tile.settings.<page>.top/bottom/glyph` per Settings page | Tool icon tiles; Settings page tiles (A96) |
| shadow | `shadow.tint` | The colour every elevation layer uses |
| shadow | `shadow.scrim` | The dim laid over a window under the command palette (mockup 12's `.scrim`) |

Mode colours: Ask uses `text.primary` on the selected segment, Plan uses `status.plan`, Auto uses
`accent`, Bypass fills the segment with `status.danger` and draws a 3 pt `status.danger` strip under
the toolbar (§4).

### 3.2 Typography

SF Pro Text for UI and SF Mono for code. Sizes are points at 100 % text size; the user scales all of
them together between 85 % and 150 % (⌘+ / ⌘−). Digits that change while you watch (cost, tokens,
tok/s, timers) are always tabular (`.monospacedDigit()`).

| Token | Use | *mirror* size / weight |
| --- | --- | --- |
| `font.title.window` | Session title in the toolbar | 13.5 / semibold |
| `font.title.page` | Settings page title (`.set-main h1`) | 20 / bold |
| `font.title.hero` | First-run window title (screen 21); no view draws it yet | 26 / bold |
| `font.title.group` | Settings group box title (`.gtitle`), sentence case | 12 / semibold |
| `font.title.palette` | The command palette's query (`.palette .q`) | 18 / regular |
| `font.title.session` | Session row title in the sidebar (`.row .t`) | 13 / medium |
| `font.body` | Default UI text | 13 / regular |
| `font.transcript` | Messages | 13.5 / regular, line height 1.55 |
| `font.transcript.h1` | Markdown headings, level 1 (DT§5.9, A94) | 17 / semibold, line height 1.35 |
| `font.transcript.h3` | Markdown headings, level 2 | 15 / semibold, line height 1.35 |
| `font.transcript.h4` | Markdown headings, level 3, and levels 4–6, which DT§5.9 does not size | 13 / semibold, line height 1.35 |
| `font.control` | Buttons, capsules, segments | 12.5 / medium |
| `font.caption` | Thinking line, notices | 12 / regular |
| `font.footnote` | Tool status, meter, key–value rows | 11.5 / regular |
| `font.compact` | Dense rows: inspector file and plan rows (`.fr`, `.todo`), the decision bar (`.pinned`), the cost capsule's `· ctx` | 12.5 / regular |
| `font.segment` | Segment and inspector tab labels (`.seg span`, `.tabs span`) | 12 / medium |
| `font.stop` | The Stop button's label (`.stop`) | 12 / semibold |
| `font.detail` | Second lines: a session row's subtitle and cost, the token popover's rate line and note | 11 / regular |
| `font.legend` | The token popover's legend (`.tokpop .leg`) | 10 / regular |
| `font.label` | Uppercase section headers, tracking 0.03 em | 11 / semibold |
| `font.micro` | Key caps, legends, badges | 10.5 / medium |
| `font.metric` | Big live numbers (tok/s) | 26 / semibold, tracking −0.02 em |
| `font.mono.code`, `mono.inline`, `mono.terminal` | Code, inline code, terminal tail | 12 / 12 / 11.5 |
| `font.mono.command` | The command in an approval card's well (`.appr .cmd`) | 12.5 / regular |

### 3.3 Space, radius, size

- **Space** is a fixed scale: `xxs 2 · xs 4 · s 6 · m 8 · ml 10 · l 12 · xl 16 · xxl 20 · xxxl 24 · huge 32`.
  Inside a component use `xs`–`l`; between components use `m`–`xl`; between sections use `xxl`+.
  Off the scale, by role, where a mockup insists: `tab` 9 (an inspector tab's side inset), `popover`
  14 (a popover's top inset), `toolbarTrailing` 14, `composerBottom` 18 (under the composer).
- **Radius** goes up with the size of the thing: key cap `xs` → icon tile `s` → button `m` → tool card
  and row `l` → bubble `xl` → approval `xxl` → sidebar `panel` → pane and composer `pane` → popover
  `popover` → window `window`; pills use `capsule`. Nested shapes are concentric: inner radius = outer
  radius − inset. Badges and inline code use `badge` (5), between `xs` and `s`; an inspector tab
  `tab` (7); a legend swatch `swatch` (2).
- **Size** fixes the layout skeleton: toolbar 56, sidebar 252, inspector 324, reading column 760,
  gap between floating panes 8, capsule 32, button 28 / 24, icon tile 22, status dot 9 (halo 3, idle
  ring 1.5), count badge 16, hairline 0.5, quote bar 3; composer chip `chipHeight` 26, Send
  `sendButton` 30, the composer's text area at least `composerTextMinHeight` 46 with its insets,
  completion list `completionWidth` 470, the command palette `paletteWidth` 640 with its row
  symbol well `paletteIcon` 26, Review's file list `reviewFileListWidth` 260, and the
  first-run window `windowSmallWidth` 980 (the mockup's `.window.small`), and a new session window
  `windowDefaultWidth` × `windowDefaultHeight` 1440 × 900 (the mockups' window, A126).

### 3.4 Elevation (depth)

Six levels. Each is a stack of shadow layers tinted `shadow.tint` plus, from e1 up, a 1 pt inner
highlight on the top edge — the highlight is what makes glass read as a solid object. In dark
mode the highlight follows two `[desktop.appearance]` settings (A109): `dark_highlight` is `none`
(the default, the dark mockup) or `subtle` (`material.darkHighlight.subtle` of
`glass.highlight`, §3.5), and `dark_highlight_scope` applies it to controls only (`e1`, the default) or to every
lifted level (`e1`–`e4`, the user bubble too); the other levels keep the full highlight.

| Level | What sits there |
| --- | --- |
| `e0` | Prose, collapsed tool rows, list rows at rest |
| `e1` | Chips, capsules, segmented controls, icon tiles, the selected row, key caps |
| `e2` | Cards: user bubble, expanded tool, approval, sidebar, inspector |
| `e3` | The composer — the highest thing inside a pane |
| `e4` | Popovers, menus, the command palette |
| `e5` | The window itself over the wallpaper |

The **Depth** setting (Flat … 3D) scales every level's shadow opacity and y-offset by 0…1; at Flat the
highlights go too and the app looks like a standard macOS app. Only `e5` ignores Depth.

### 3.5 Materials — glass

| Material | Window opacity | Blur | Specular | SwiftUI / AppKit |
| --- | --- | --- | --- | --- |
| Frosted | `material.frosted.windowOpacity` (default) | heavy | soft sweep | `glassEffect(.regular)`; window behind: `NSVisualEffectView`, `.behindWindow` |
| Glossy | lower | light | strong sweep and streak | `glassEffect(.clear)` + the `specular` sweep |
| Solid | 1 | none | none | Plain `surface.window`; forced by Reduce Transparency |

Glass colours (`color.glass.*`, T51.1): the pane tint, rim and top-edge highlight glass draws over
the window's own tint (`surface.window` at the material's window opacity). Light takes mockup 28's
values; dark takes mockups 31-main-glass-dark-frosted and 32-main-glass-dark-glossy, the same
layout over the same wallpaper — a cool near-black tint that keeps the wallpaper's colour, and
white rims.

| Token | *mirror* light | *mirror* dark | High Contrast (`high-contrast.mjs`) |
| --- | --- | --- | --- |
| `glass.fill` | `#ffffff` at 0.34 | `#14141a` at 0.34 | glass: keeps `glassKeep` of its transparency (0.835) |
| `glass.border` | `#ffffff` at 0.75 | `#ffffff` at 0.16 | a border: solid, at least 3:1 on every page surface |
| `glass.highlight` | `#ffffff` at 0.95 | `#ffffff` at 0.22 | kept: depth, nothing to read on it |
| `glass.specular` | `#ffffff` | `#ffffff` | kept: the sweep's light; Increase Contrast drops the sweep instead |

On glass (Frosted, Glossy) the panes — sidebar, transcript column, inspector — draw `glass.fill`
as the token has it, not scaled by the window opacity (the slider moves the window's tint under
them), and every shell layer, the window's too, is rimmed by `glass.border`; in Solid they keep
their plain `surface.*` and the `separator` hairline (T51.23). The specular sweep and streak are
`glass.specular` at `material.*Specular` strength, in SwiftUI and in the transcript's AppKit bubble.

In dark, lifted panes (e2 and up) draw `glass.highlight` as it is; controls (e1) draw it at
`material.darkHighlight`'s share — none by default, as mockups 31 and 32 show (A109, §3.4).
CoxUI keeps the light highlight in the elevation tokens and scales it by `glass.highlight`'s
strength in the drawn appearance, read from the colour asset (T51.22). On the
dark glass, `text.primary` is 15.3:1, `text.secondary` 6.7:1, the selected row's subtitle 5.4:1 and
the filter prompt 7.1:1, laid over opaque `surface.window` (§8).

- The user sets material, window transparency, blur (frosted) or reflection (glossy), Depth, and
  "Tint from wallpaper" in the Appearance popover (toolbar paintbrush, ⌘⌥A) and in Settings ›
  Appearance. They are stored in Rust-owned config under `[desktop.appearance]`
  (`material`, `opacity`, `blur`, `depth`, `tint`), so they have a schema and provenance like any other
  setting.
- Text-bearing surfaces — messages, code, diffs, terminal, popovers, the composer — never drop below
  `material.readableFloor`. The transparency slider only moves the window's background; the panes
  over it keep `glass.fill` (T51.23).
- Increase Contrast (A89, A100): no specular sweep, and every glass background — window, panes and
  readable surfaces — keeps `material.highContrast.glassKeep` (a quarter) of its transparency:
  `opacity' = 1 − (1 − opacity) × glassKeep`. `Appearance.effective` applies it; Reduce Transparency
  still wins and forces Solid. `high-contrast.mjs` reads the same token for the glass colours.
- Panes are separate glass layers (sidebar, transcript column, inspector) with an 8 pt gap, so the
  wallpaper shows between them. Group neighbouring glass in one `GlassEffectContainer` so shapes blend
  and render in one pass.

### 3.6 Motion

- `motion.duration.fast` for hover and press, `base` for disclosure, `slow` for popovers and panes;
  easing `standard`, or `decelerate` for things entering.
- Streaming text is never animated per token. A new block fades in; a state change (spinner → ✓)
  cross-fades; the sparkline scrolls without easing.
- Reduce Motion: movement becomes a cross-fade of the same duration.

### 3.7 Icons

SF Symbols only, weight medium, rendering hierarchical; sizes 11, 12, 13, 14, 16. The mockup's icon
names map one-to-one:

| Mockup | Symbol | Mockup | Symbol |
| --- | --- | --- | --- |
| doc | `doc.text` | clip | `paperclip` |
| pencil | `pencil` | up | `arrow.up` |
| term | `terminal` | check | `checkmark` |
| search | `magnifyingglass` | chev / chevd | `chevron.right` / `chevron.down` |
| globe | `globe` | people | `person.2` |
| eye | `eye` | rewind | `arrow.uturn.backward` |
| paint | `paintbrush` | refresh | `arrow.clockwise` |
| insp | `sidebar.right` | comment | `text.bubble` |
| sparkle | `sparkle` | bolt | `bolt` |
| branch | `arrow.triangle.branch` | stop | `stop.fill` |
| gear | `gearshape` | shield | `shield` |
| lock | `lock` | dollar | `dollarsign.circle` |
| plug | `powerplug` | cpu | `cpu` |
| — (Settings › Advanced) | `slider.horizontal.3` | — (Changes › deleted file) | `trash` |
| — (Changes › checkpoint) | `clock` | | |

## 4. Layout

```
┌ window (e5, radius.window) ───────────────────────────────────────────────────────────┐
│ ┌ Sidebar (e2) ┐ ┌ Toolbar: Breadcrumb · spacer · ModelCapsule · ModeSegmented ·     ┐ │
│ │ SessionFilter│ │          CostCapsule · StopButton · AppearanceButton · Inspector  │ │
│ │ SectionHeader│ ├ TranscriptPane (glass) ──────────────┐ ┌ Inspector (e2) ──────────┤ │
│ │ SessionRow…  │ │   reading column 760, centred        │ │ InspectorTabs            │ │
│ │              │ │   Turn → blocks                      │ │ tab content              │ │
│ │ SidebarFooter│ │   Composer (e3) + TokenMeter         │ │                          │ │
│ └──────────────┘ └──────────────────────────────────────┘ └──────────────────────────┘ │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

- `MainScreen` lays the panes out itself (`ShellPane`: window, sidebar, column, inspector) rather than
  in a `NavigationSplitView`: the system's split view draws its own sidebar glass and toolbar, which
  follow neither `[desktop.appearance]` (material, opacity, Depth) nor the tokens, and a window
  toolbar cannot be rendered by the snapshot harness. Sidebar and inspector are collapsible; the
  system window buttons sit over the sidebar's top row, and the toolbar leaves room for them while the
  sidebar is hidden. The app window hides its title bar and puts the behind-window blur under the
  window pane.
- The sidebar and inspector toggles answer ⌃⌘S and ⌃⌘I, the default keys of the system
  `SidebarCommands` and `InspectorCommands` (A89), and the Appearance button answers ⌘⌥A. The buttons
  carry the keys (`ShellShortcut`) and their tooltips name them; the app's View menu shows the same
  items on the same keys.
- While the session is in Bypass mode a 3 pt `status.danger` strip runs under the toolbar, along the
  panes below it, for as long as the mode is on (§3.1).
- The reading column is `size.readingWidth` wide and centred; the composer shares its width.
- A new session window opens at `size.windowDefaultWidth` × `size.windowDefaultHeight`, each side cut
  to the screen's visible frame (`Size.defaultWindow`, T37.44.14); a restored window keeps its frame.
- Minimum window `size.windowMinWidth` × `size.windowMinHeight`. Below 1280 pt of window width the
  inspector floats over the transcript column, `size.paneGap` in from its edges, instead of taking width
  from it: the column keeps the width it has with the inspector hidden.

## 5. Swift package layout for the view layer

```
Packages/CoxUI/Sources/CoxUI/
├─ Tokens/        generated: Tokens.swift, Colors.xcassets — never edited by hand
├─ Foundations/   Appearance (the settings every modifier reads, with Reduce Transparency and Reduce
│                 Motion applied once) and the ViewModifiers and styles: Elevation, GlassPane,
│                 Specular, HairlineModifier, InsetWell, TextStyle, ControlState (rest, hovered,
│                 pressed, disabled for every button-like style), CoxButtonStyle, CapsuleStyle,
│                 CoxSegmented, CoxToggleStyle, CoxSlider, Knob
├─ Atoms/         one file per atom (§6.2)
├─ Molecules/     one file per molecule (§6.3)
├─ Organisms/     one file per organism (§6.4)
├─ Screens/       composition only: organisms + layout, no styling
└─ Previews/      PreviewState fixtures shared by every #Preview and snapshot test
```

A layer may use only the layers above it in this list. `Screens` contains no modifiers from
`Foundations`; if a screen needs styling, the styling belongs to a component. The Foundations
modifiers are `internal`, so nothing outside `CoxUI` can style a view; the app sets only the public
`coxAppearance` environment value from `[desktop.appearance]`.

## 6. Component catalogue

Every component: one file; takes a value-type state (`Equatable`, `Sendable`) and closures or an intent
enum; has a `#Preview` for each variant in light, dark, Solid and Frosted; has a snapshot test. The
"CSS" column names the mockup class, so an agent can translate a mockup element straight to its
component.

### 6.1 Foundations (modifiers and styles)

| Name | What it does | Tokens | CSS |
| --- | --- | --- | --- |
| `.elevation(_ level:, cornerRadius:)` | Shadow layers + top highlight, scaled by Depth; in dark mode the highlight takes the `dark_highlight` share on the `dark_highlight_scope` levels (A109) | `elevation.e0–e5`, Depth, `material.darkHighlight.*` | `--lift1…3` |
| `.glassPane(_ shape:, surface:, role:)` | Pane material: glass or solid per setting; `role: .readable` holds the readable floor | `material.*`, `surface.*` | `.glass .col`, `.sidebar`, `.insp` |
| `.specular(_ strength:, in:)` | Diagonal highlight behind the content and over the surface, so text keeps its token colour (strong for Glossy, faint for Frosted, none for Solid or under Increase Contrast) | `material.*.specular` | `.window:after` |
| `.hairline(_ edges:)`, `.hairline(in:, color:)` | 0.5 pt `separator` line on edges or around a shape; capsules pass `surface.capsuleBorder` | `size.hairline`, `separator`, `surface.capsuleBorder` | `border:.5px` |
| `.dashedBorder(in:, color:)` | 1.5 pt dashed line (dash `space.s`, gap `space.xs`) just inside a shape: the outline of a drop target, `separator` at rest and `accent` while a drag hovers | `separator`, `accent`, `space.*` | `border:1.5px dashed` |
| `.insetWell(_ surface:, cornerRadius:)` | Pressed-in look for terminal and fields | `surface.terminal`, inner shadow | `.tail`, `.filter` |
| `.textStyle(_ token:, tabularDigits:)` | Font at the text size, line height, tracking, tabular digits | `font.*` | font rules |
| `.symbolStyle(_ token:)` | An SF Symbol at a font token's size, weight medium, rendered hierarchical (§3.7) | `font.*` | `svg` icons |
| `CoxButtonStyle(.primary/.secondary/.danger/.plain, size: .regular/.small)` | All push buttons: e1 face with specular; hover tints, press sinks to e0; disabled keeps the readable floor and a `text.secondary` label; the primary label is `text.onAccent` | `size.button*`, `radius.m`, `font.control`, `fill.*`, `text.onAccent` | `.pb`, `.pri`, `.dan` |
| `CapsuleStyle(.plain/.active, isIcon:)` | Toolbar capsules and filter chips; `isIcon` makes a round capsule for a symbol alone (`.cap.icon`): readable glass face at e1 with a `surface.capsuleBorder` hairline (the mockup's `--glass-b`); active takes `surface.window`, an `accent` label and an `accent.soft` halo; states as `CoxButtonStyle` | `size.capsuleHeight`, `radius.capsule`, `surface.capsule`, `font.control` | `.cap`, `.cap.hot` |
| `CoxSegmented(_ label:, selection:, options:, look:, title:)` | Segmented control; the e1-lifted selection pill slides between segments, or cross-fades under Reduce Motion (`coxMatchedGeometry`). `look` marks the selected segment per option: `plain`, `tinted(colour)` label, or `filled(colour)` pill with a `text.onAccent` label (the §3.1 mode colours). A view, not a `PickerStyle`: SwiftUI has no public hook to restyle segments on macOS | `e1`, `surface.capsule`, `font.segment` | `.seg` |
| `CoxToggleStyle`, `CoxSlider(_ label:, value:, in:)` | Toggles and sliders with the shared 3D `Knob` (white disc, hairline rim, e1) over an `insetWell` track filled with `accent`; the toggle's knob slides, or cross-fades under Reduce Motion. Disabled, both lose the accent and the knob's lift, as a disabled button does: the toggle's track shows `fill.secondary` and its label `text.secondary`, the slider drops its fill. The slider is a view: macOS has no public `SliderStyle` | `e1`, `accent`, `fill.secondary` | `.tog`, `.slider` |

### 6.2 Atoms

| Atom | Variants / states | CSS |
| --- | --- | --- |
| `StatusDot` | running, waiting, idle, error | `.dot .d-*` |
| `IconTile(kind, symbol)` | neutral, edit, shell, search, write; the tool picks the DS§3.7 symbol. `IconTile(face:glyph:symbol:)` takes its caller's colours, as a Settings page's | `.tool .ic.c-*`, `.set-side .sq` |
| `AppIcon()` | cox's mark: `cx` in `font.mono.appIcon` and `tile.app.glyph` on the `tile.app` top-to-bottom gradient (145°), `size.appIconHero` square, `radius.xxl`, e2; decorative (Figma frame 22) | `.appicon` |
| `KeyCap` | glass; inverted (an outline in `surface.window`, on StopButton's `text.primary` face) | `.kbd` |
| `Badge` | neutral, user, project (`role.project`), env, default, warning, danger; `radius.badge` | `.badge .b-*` |
| `CountBadge` | `text.onAccent` digits on a `status.warning` pill at e1, `size.countBadge` high | `.sect .cnt` |
| `RiskChip(text, level)` | low, medium, high — drawn as a `Badge` in the level's `risk.*` role | `.risk` |
| `Spinner`, `ProgressRing(fraction)` | — | `.spin`, `.ring` |
| `Sparkline(samples)` | tint | `svg` in `.meter` |
| `StackedBar(segments)` | — | `.tokpop .bar` |
| `DiffStat(added, removed)` | an add-only change shows no "−0" | `.plus`, `.minus` |
| `SectionHeader(title, trailing)` | title in `text.secondary`, not the mockup's tertiary, so it stays readable on Frosted (§8) | `.sect`, `.ih` |
| `InlineCode`, `Hairline(orientation)` | `Hairline`: horizontal, vertical; drawn by `.hairline` | `code`, `.divider:before`, `.sep` |
| `Thumbnail(attachment)` | image, file | `.thumb` |

### 6.3 Molecules

| Molecule | Built from | CSS |
| --- | --- | --- |
| `SessionRow(item, isSelected:)` | StatusDot, title, subtitle, cost; selected with InspectorRow's `rowSelection` on `accent.selected` at e1 (A115) | `.row` |
| `SessionFilter(text:, prompt:, shortcut:)` | search field in an `insetWell`, the prompt and magnifier in `text.placeholder` (§8, A112), KeyCap | `.filter` |
| `Breadcrumb(title, project:, branch:)` | title, project, branch | `.crumb` |
| `ModelCapsule(model, isOpen:)`, `CostCapsule(cost:, context:, fraction:, isOpen:)` | CapsuleStyle (active while open), ProgressRing; the model's sparkle in `role.project` (the mockup's `--purple`) | `.cap` |
| `ModeSegmented(selection:)` | CoxSegmented; ask, plan, auto, bypass (offered only while on) | `.seg` |
| `StopButton` | inverted KeyCap; inverted `text.primary` capsule answering ⌘. | `.stop` |
| `ToolHeader(item, isExpanded:)` | IconTile, summary (subject bold, monospaced for a command), DiffStat, RiskChip, Spinner / check / cross and duration, disclosure chevron; expanded on `fill.primary` over a hairline | `.tool .h` |
| `DiffLineView(line, widestNumber:)`, `DiffHunkView(header:, lines:)` | gutter number (`text.secondary`, `text.primary` on a `diff.*Gutter`), sign, `CodeRun` syntax runs on `diff.add` / `diff.del` (coloured by the session theme's light or dark variant, as the view's appearance picks, A95), a replaced pair's changed words (the core's word diff) on `diff.*Gutter`; the hunk: header on `fill.primary`, one gutter width, `surface.code`; in Review (`revert:`) the header's trailing "Revert hunk" (caption, `text.secondary`) is laid out always and shown while the hunk is hovered (T51.21) | `.diff .ln`, `.hh` |
| `CodeBlockView(language:, lines:, copy:)` | header (language, icon-only `doc.on.doc` copy button, `CoxButtonStyle(.plain, size: .small)`), `CodeRun` lines scrolling sideways on `surface.code`, `radius.l` | `.codeblock` |
| `TerminalTail(lines, exit:)` | insetWell on `surface.terminal`, `font.mono.terminal` lines in `text.terminal`, cut with an ellipsis; exit line: check + status in `text.terminalOk`, or `status.danger` cross + status in `text.terminal`; none while running | `.tail` |
| `UserBubble(text, attachments:)`, `PromptActions(_:act:)` | prompt in `font.transcript`, a row of Thumbnail `space.m` below the text; readable face at e2, the glass sweep behind the text. On hover, `PromptActions` sits in the bubble's top trailing corner: Edit and resend (`pencil` and its title) and Copy (its title) as labelled `CoxButtonStyle(.secondary, size: .small)` push buttons `space.s` apart (T37.23.9; Figma frame 14) | `.user`, `.user .att`, `.pb` |
| `TurnGutter(turn:isMarked:open:)` | a prompt's turn number `size.turnGutter` wide, right-aligned, `size.turnGutterOffset` left of its bubble, `font.detail` tabular in `text.secondary`; marked (the turn a rewind goes back to before), semibold `accent` with `arrow.uturn.backward`, and with `open` a button that opens `RewindMenu` (Figma frames 01, 14). In the transcript CoxTranscriptText draws the number itself beside each prompt's bubble from `TranscriptStyle.gutter`, mapped to the same tokens (T37.47) | `.gutter` |
| `ThinkingDisclosure(summary, text:, isExpanded:)` | chevron and caption summary; open, the reasoning in italic caption beside a hairline; open state is the view's own. Its row is public as `ThinkingHeader(summary, isExpanded:, action:)`, which the transcript shows above reasoning it draws as text (T37.23.4) | `.think`, `.think-body` |
| `NoticeRow(text, kind:, symbol:)`, `TurnDivider(label)`, `TurnMeta(facts)` | symbol in the kind's colour (info, warning, error) + caption in a readable colour / Hairline, caption, Hairline / model, tokens, cache, cost, duration, stop reason in tabular footnote | `.notice`, `.divider`, `.meta` |
| `ComposerChip(label, kind:, shortcut:, onRemove:)` | mention, attachment, command, shell, queued, model, `think(Bool)`, `mode(SessionMode)`: symbol (none for a mode or the queue, as the mockup draws them), caption label (empty for the paperclip's icon-only chip), optional KeyCap and `xmark` remove button on a readable capsule at e1; mention, command and queued tinted `accent`; a mode in its DS§3.1 colour (Ask plain, Plan `status.plan`, Auto `accent`, Bypass `status.danger`), as `ModeSegmented` shows it; think (`brain`) plain while off, tinted `accent` while on. `ThinkChip(isOn:, toggle:)` is the think chip as a button with its tooltip (A103) | `.chip`, `.chip.blue` |
| `CompletionList(state:, pick:)` | SectionHeader over the rows the core ranked for `@` or `/` (title, detail in footnote), the selected one in `text.onAccent` on an `accent` face (the mockup's `.it.on`; ModelPopover's rows too); readable `surface.popover` glass at e4, `radius.xxl` | `.pop`, `.pop .it` |
| `TokenMeter(state:, isOpen:, action:)` | ↑ sent, ↓ received, then behind a hairline StatusDot (running while a turn runs), tok/s and Sparkline, on a CapsuleStyle capsule, active while the popover is open; every figure and the VoiceOver line come formatted from `cox_app::MeterText` | `.meter` |
| `KeyValueGrid(columns:, rows:)` | rows of label / values under optional column headers; detail rows indented in `text.secondary` | `.tokpop .grid` |
| `PluginWidgetView(widget)` | a plugin's PL§8 widget tree (T52.16) drawn natively in `font.monoCode`, one view per variant: lines of spans; a list with its selected row on `accent.soft` at `radius.xs`; a Grid table under a semibold header; key–value Grid rows; a linear ProgressView tinted `accent` after its label; a VStack or HStack of child trees; a block stroked in `separator` on `radius.s` under a semibold title. Each span's role maps to a DS§3.1 token (`text`/`agent` → `text.primary`, `dim`/`border` → `text.secondary`, `accent`/`selection` → `accent`, `user` → `role.project`, `tool` → `syntax.function`, `diff_hunk` → `syntax.keyword`, `ok`/`diff_add` → `status.success`, `warn` → `status.warning`, `error`/`diff_del` → `status.danger`), never a raw colour and never plan mode's `status.plan`; the terminal's cell widths and stack sizes are not applied | — |
| `MaterialPicker(selection:)` | three swatch tiles (Frosted, Glossy, Solid) on readable glass at e1 with a hairline, the selected one ringed in `accent`; each shows a pane of its own material over a wallpaper, lifted to e2 at the user's Depth | `.mat` |
| `LabeledSlider(title, value:, in:, valueText:, ends:)`, `LabeledToggle(title, detail:, isOn:)` | SectionHeader + CoxSlider + end labels / CoxToggleStyle with an optional detail line | `.appear .lbl`, `.row2` |
| `ChangedFileRow(file, isSelected:, actions:)`, `CheckpointRow(checkpoint, isSelected:, actions:)` | the shared `InspectorRow`: DS§3.7 glyph (`pencil`/`doc.text`/`trash` by change, `clock`), path with its directory in `text.secondary` and the file name kept on truncation / label, DiffStat / time in `text.secondary`, then the `RowAction` icon buttons (tooltip = title) while hovered or selected; selected on `accent.soft` at e1 | `.fr` (inspector rows) |
| `SettingRow(source:, content:)`, `SettingRow(title, detail:, source:, control:)`, `SettingLabel(title, detail:)` | a LabeledToggle / LabeledSlider, or a SettingLabel beside any control; then a Badge of the source layer (`SettingSource`: default, user, project, claude-settings, env, flag). A layer above the user's config (project, claude-settings, env, flag) makes the row read-only: the control is disabled and a lock precedes the badge. LabeledToggle names its setting with the same SettingLabel; a row that names a thing (an MCP server, a check) sets its title semibold (`namesItem`, the mockup's `<b>`) | `.group .gr` |
| `SettingField(_ value, prompt:, isSecure:, commit:)` | text or `SecureField` in an `insetWell` on `fill.primary`, `size.sidebarWidth` wide; typing stays local until Return commits it; a secure field never shows what is stored and empties once sent. Its well is `settingWell(width:)`, which `KeySheet`'s field shares | text well |
| `SettingPopUp(label, selection:, options:, title:)` | the selection's title in `font.control` and a micro `chevron.down` on `surface.window`, `radius.s`, a hairline, e1, `size.buttonHeightSmall` high; a menu with the selection checked. `SettingsStore` gives an enum with more than three options, or a tier's model from the catalog, this instead of CoxSegmented (T37.45.2) | `.sel` |
| `ChecklistRow(title, detail:, status:, symbol:, action:, perform:)` | status symbol in its colour (passed `checkmark` in `status.success`, warning, missing `xmark.octagon` in `status.danger`, a step to take in `accent`) in one `size.iconTile` column, SettingLabel, then the fix button (`CoxButtonStyle` small: primary for a step, secondary for a fix); SettingRow's insets | onboarding `.group .gr` |
| `ProjectDropZone(isTargeted:, send:)` | the Open a project `ChecklistRow` step (`folder` symbol, primary Choose Folder…) in a `.dashedBorder` on `radius.panel`, padded `space.m`; the border turns `accent` while a drag hovers (`Motion.durationFast`). A drop of exactly one local directory reports `openFolder(url)`, the intent the app sends for the folder its picker chose; a file, a web link or several items are refused (DT§5.8, T37.45.5) | `separator`, `accent`, `radius.panel`, `space.m` | onboarding dashed box |

### 6.4 Organisms

| Organism | Built from | CSS |
| --- | --- | --- |
| `ShellPane(.window/.sidebar/.column/.inspector)` | glassPane, hairline, elevation: e5 window, e2 side panes, flat column | `.window`, `.sidebar`, `.col`, `.insp` |
| `Sidebar` | ShellPane, SessionFilter, SectionHeader + CountBadge, project disclosure, SessionRow, footer (New session, provider StatusDot); an expired "Needs you" item's row is not a button and shows its title in `text.secondary`, not a faded label (§8); a remote host's group (T52.21) heads its rows with a host badge — `server.rack` and the ssh alias in `font.control`, `text.secondary`, in a `fill.primary` well at `radius.s` — and, once disconnected, a `status.danger` StatusDot, "Disconnected" in `font.caption` and a plain small Reconnect button; its rows are then read-only | `.sidebar` |
| `SessionToolbar` | Breadcrumb, then the plugins' `status.left`/`status.right` segments (PluginWidgetView, one line each, at most 24 code-face cells wide, truncated; a `ViewThatFits` drops them first when the bar is narrow; T52.17), ModelCapsule, ModeSegmented, CostCapsule, StopButton, icon CapsuleStyle buttons (Appearance ⌘⌥A, inspector ⌃⌘I, sidebar ⌃⌘S while hidden; tooltips name the keys), the Bypass strip under the bar | `.toolbar` |
| `ToolCard(content, isExpanded:)` | ToolHeader + one detail: the edit's DiffHunkViews or a TerminalTail. A running call shows its tail under a flat header; a finished one folds the detail behind the chevron (open state is the card's own); opened, a readable face at e2 with a hairline rim, `radius.l`. Public with the value types it takes (T37.23) | `.tool`, `.tool.exp` |
| `ApprovalCard(content, act:)`, `ApprovalCard(content, act:, edit:)` | header (warning symbol, title, RiskChip), the command in a `surface.code` well, the reasons (why, which subagent) in caption, then Allow / Allow for session / Edit… (when given `edit` and an input) / Deny as regular-height CoxButtonStyle and "Session grant: …" in footnote at the trailing end (under the buttons when it does not fit), on `fill.primary` under a hairline; Edit… puts the input as JSON in the well with Run edited (only while it parses) and Cancel (T37.27.6); a readable face at e1 with a `status.warning` leading edge, `radius.xxl`. Decided, it shrinks to a NoticeRow: "Allowed by you · for session". CoxTranscript's `DecisionCard` fills it from the block and sends the choice as an `Intent` (T37.27) | `.appr` |
| `QuestionCard(content, answer:)` | the same frame with an `accent` edge: who asks, the question in `font.transcript`, one CoxButtonStyle button per option, then a field and Answer on the action row; answered, a NoticeRow "You answered: …" (T37.27) | `.appr` (question) |
| `DecisionBar(content, choose:)` | the approval or question a turn waits on, pinned above the Composer while its card stays in the transcript: a symbol (`exclamationmark.triangle` in `status.warning`, `questionmark.circle` in `accent`), "Waiting for you: " and the command in semibold mono, or "Question from cox: " and the question, on one line cut in the middle; then Allow / For session / Deny, or a question's answers while they fit, as small CoxButtonStyle; `status.warning.soft` or `accent.soft` over a readable face, `radius.xl`, a hairline. ⌘⏎ allows and ⌘⌫ denies from anywhere in the window, ahead of the editor's own keys. CoxTranscript's `SessionComposer` pins the session's first waiting block and sends the choice as the card's `Intent` (T37.27.5) | `.pinned` |
| `AssistantMessage` | markdown runs, InlineCode, CodeBlockView | `.asst` |
| `TurnView` | UserBubble, ThinkingDisclosure, ToolCard, AssistantMessage, TurnMeta. Not a view of its own under A87: a turn is the run of blocks it owns in `TranscriptView`'s one text, the user message and the thought styled text ranges, the tool calls ToolCards (T37.23) | `.turn` |
| `TranscriptView(store:crossBlockSelection:approval:)` | one TextKit 2 text (`CoxTranscriptText`) of the store's blocks, kept in step by the patches the store applies; tool, tool-group and task blocks are ToolCards hosted as one character each, approvals and questions the `approval` slot; prose in `font.transcript`, code in `font.mono.code`, readable text colours only. Package `CoxTranscript`, where CoxUI, the text view and the store meet (T37.23) | `.scroll` |
| `Composer(state:, send:)` | text editor (`font.transcript`, `font.mono.code` in shell mode) with its hint, CompletionList floating above, a row of attachment Thumbnails with an `xmark.circle.fill` remove badge, a row of the paperclip as an icon-only ComposerChip and ComposerChip (the permission mode with its ⇧⇥ KeyCap, which cycles it; the model and effort; the think toggle beside it, which sends the next turn to the think tier with `confirm_think` and turns itself off once that turn is sent (A103, `toggleThink`); shell with the "share output" CoxToggleStyle, mentions, queued) and a round `accent` send button (`fill.secondary` while disabled); the hint in `text.placeholder` (A112); readable `surface.window` glass at e3, `radius.pane`. ⏎ sends or picks, ⇧⏎ breaks the line, ⌘⏎ sends now, ↑ ↓ ⇥ ⎋ drive the rows, ⌫ on an empty shell line leaves shell mode, ⇧⇥ or a click on the mode chip asks for the next mode (`cycleMode`; the core names it and moves the chip), dropped files attach; every key and click is a `Composer.Intent`. TokenMeter sits before Send and opens TokenPopover standing on the composer's top edge (`toggleTokens`) | `.composer` |
| `TokenPopover(state:)` | heading and phase (`accent` while streaming), `font.metric` tok/s beside a Sparkline, the rate line (avg, first token, peak), a turn / session KeyValueGrid, the context heading over a StackedBar and its `font.legend` legend with `radius.swatch` swatches (shown once the core sends the parts), a note in `font.detail`; readable `surface.popover` glass at e4, `size.tokenPopoverWidth` | `.tokpop` |
| `AppearancePopover(state:, send:)` | title and KeyCap, MaterialPicker, LabeledSlider ×3 (transparency; blur, or reflection for Glossy; Depth), LabeledToggle (tint), a note in footnote; readable `surface.popover` glass at e4. Solid disables transparency and blur; Reduce Transparency disables all but Depth and the note says why. Reports one intent per `[desktop.appearance]` key; the window draws from the same state | `.appear` |
| `Inspector` | ShellPane, title, tab strip (the selected tab lifted on `surface.window` on glass, a `fill.secondary` well on Solid); each tab's content (ChangedFileRow, CheckpointRow, KeyValueGrid) is a slot, scrolling in one inset body | `.insp` |
| `ChangesTab(state:, send:)` | the Changes tab (DT§5.1): `InspectorSection`s (SectionHeader over flush rows, shared by every tab) of ChangedFileRow under a `Review` link with its KeyCap (⌘⇧R, which the app's menu answers), CheckpointRow, and the worktree's KeyValueGrid; an empty section is left out, an empty tab says so. A row click opens Review at the file; the rows' actions report review, revert and rewind (code only, A101) | `.ib`, `.ih`, `.fr` |
| `RewindTimeline(state:, send:)` | the rewind timeline (DT§3, DT§5.4): one `InspectorSection` `Rewind` of CheckpointRows oldest first, the selected one (a gutter mark's) lifted; each row's actions are the scopes — Restore code (`doc.text`), Restore conversation (`text.bubble`), Restore code and conversation (`arrow.uturn.backward`) — reported as one `rewind(checkpoint:code:conversation:)`; empty, it says so. CoxModel's `SessionStore.rewind(checkpoint:code:conversation:)` sends it as `Intent.rewind` (T37.28.1) | `.ih`, `.fr` |
| `RewindMenu(state:, send:)` | one prompt's rewind scopes (Figma frame 14): a `SectionHeader` "Rewind to before turn N", then Code and conversation (`arrow.uturn.backward`, "N files restored" when known), Code only (`doc.text`, "keep the chat"), Conversation only (`text.bubble`, "keep the files"), a hairline, and Fork a new session here (`arrow.triangle.branch`); the highlighted row on `accent`; readable popover glass at e4, `size.rewindMenuWidth`, `radius.xl`. Reports `rewind(turn:code:conversation:)` or `fork(turn:)`; CoxModel's `SessionStore.rewind(toTurn:code:conversation:)` and `fork(beforeTurn:)` send them, `ChangesRewindPreview` counts the files changed at or after the turn from `Changes.turns` (T37.46). In the transcript a hovered prompt's marked `TurnGutter` opens it in a popover (CoxTranscript's `PromptRewindMenu`, T37.48) | `.popover`, `.popover .it` |
| `WelcomeHero(state:, send:)` | an empty session's transcript column (Figma frame 22): `AppIcon`, `space.welcome`, "What should we do in <project>?" in `font.title.welcome`, the workspace summary in `font.body` `text.secondary`, then the suggestions `space.xxxl` below as a three-column grid of hairline cards (`radius.xl`, title in `font.title.card` over `font.caption`), each reporting `suggest(prompt:)`; centred in `size.readingWidth`, `size.welcomeTop` from the top. CoxTranscript's `SessionWelcome` fills it from CoxClient's `WelcomeService`, which CoxCore answers with cox-app's `welcome` (T37.49) and drafts a suggestion into the composer | `.reading`, `.appicon`, `.card` |
| `ReviewPane(state:, send:)` | Review (DT§5.4, T37.28.2): in the transcript column's place, the changed files `size.reviewFileListWidth` wide grouped by the turn that changed each last (`InspectorSection` per turn of ChangedFileRows, the open one lifted) with a RewindTimeline under them, a trailing hairline, then the open file's hunks as DiffHunkViews clipped to `radius.l` with a hairline rim; with no hunks it says nothing is left (a code-only rewind) or nothing is open. A row click reports `open(path:)`, the timeline's scopes pass through. CoxModel's `SessionStore.review(path:)` reads the net diff (A101); the Changes tab's plain `rewind(checkpoint:)` restores code only. With `revertsHunks` each hunk header offers "Revert hunk": a confirmation dialog, skipped by ⌥-click, then `revertHunk(index)`; `SessionStore.revert(hunk:in:)` sends the core's hunk number and the diff's digest, and a stale revert is refused by the core's Notice (T51.21) | `.ih`, `.fr`, `.diff` |
| `PlanTab(state:)` | the Plan tab (DT§5.1): one `InspectorSection` (`Plan · done of all`) of the model's `todo` list in its order — a box per step (`square` in `text.secondary` pending, `square.inset.filled` in `accent` in progress, `checkmark.square.fill` in `status.success` done), then the step's text in `font.compact`, wrapping; a done step is struck through in `text.secondary`. VoiceOver reads the text with the state. Read-only; an empty tab says so. The app fills it from `SessionClient.plan()` (T37.29.2) | `.ib`, `.ih`, `.todo` |
| `ContextTab(state:, send:)` | the Context & Cost tab's context block (DT§5.1, mockup 10): an `InspectorSection` headed by the context (`Context · 76.4k`) with its share of the window trailing, over the StackedBar, a legend Grid (swatch in the part's `context.*` colour, the part, its tokens trailing in `text.secondary`; a `Free` row on a `fill.secondary` swatch while the window is known) and `Compact now` (`CoxButtonStyle(.secondary, size: .small)`, reported as `compact`, disabled while a turn runs, A105); then a `Cache hit` SectionHeader with the turn's or the session's figure, as `[desktop.context] cache_hit` picks (A104, per turn by default); then `Cost by turn`, an `InspectorSection` over a KeyValueGrid (`In`, `Out`, `Cache r/w`, `$`) with a row per turn (`1 · code`), each subagent (`explore`) as a detail row under its turn and `Session` last; then the project's spend today and this week as a caption footnote in `text.secondary`; before the core sent any of it the tab says so. CoxModel's `ContextTabState` fills it from the meter's `UsageView` through the shared `ContextSplit` mapping, and `SessionStore.compactNow()` sends `Intent.compact(focus: nil)` (T37.29.3.1); `SessionStore.costHistory()` reads cox-app's `TurnCosts` from the ledger (T37.29.3.2) with the project footnote (T37.29.3.3); `SessionStore.contextTab(cacheHit: SettingsStore.cacheHitScope)` picks the figure, and `turnRunning` is the meter's turn not yet done (T37.29.3.5) | `.ih`, `.bar`, `.legend`, `.pb` |
| `TasksTab(state:, send:)` | the Tasks tab (DT§5.1): one `InspectorSection` (`Subagents & background · n`) of `InspectorRow`s — the kind's glyph (`person.2` subagent, `terminal` shell), the label, the tier Badge, the cost in tabular footnote `text.secondary` once the task is done, then the ToolHeader's spinner, check or cross; an empty tab says so. A row click, or its `Open transcript` (subagent) or `Open output` (shell) action, reports the task's id to open it | `.ib`, `.ih`, `.card` |
| `InfoTab(state:)` | the Info tab (DT§5.1): an `InspectorSection` `Session` whose KeyValueGrid lists the session id, folder, worktree with its branch as a detail row, and rollout file, then `Config`: each layer that set a key with its count, its file as a detail row under it; paths start at `~`; an empty tab says so | `.ib`, `.ih`, `.tokpop .grid` |
| `SettingsSidebar(pages:, selection:, userFile:, projectFile:, filter:, search:, select:)` | ShellPane(.sidebar); a "Search settings" `SessionFilter` (Esc clears it; `pages` arrive narrowed by label and config key in CoxModel, T37.45.1), then an `InspectorRow` per `SettingsPage` (General … Advanced, DT§5.7) led by its §3.7 symbol on an `IconTile` in `tile.settings.<page>` colours, the selected one lifted on `accent` in `text.onAccent` (the mockup's `.set-side .it.on`); a search match in a page title or a `SettingLabel` is bold on `accent.soft` (bold alone on `accent`); footer: each config file cut in the middle with its layer Badge | `.set-side` |
| `SettingsGroupBox(title, content:)` | a `font.title.group` title in `text.secondary` (none for a `nil` title, as the first-run window's boxes) over the rows on `fill.primary` at `radius.xl`, a hairline between rows | `.gtitle`, `.group` |
| `TerminalPaneChrome(state:, send:, content:)` | the terminal pane under the column (mockup 24, T51.6): a header on `fill.primary` under a top hairline, `font.caption` in `text.secondary` — a tab per shell (`terminal` symbol and CoxModel's `zsh — <branch>` title), the shown one on `surface.window` in `text.primary` at e1, `radius.s`; then `+` and the trailing `` ⌃` toggle `` hint — over the `content` slot on `surface.terminal`, where the app puts CoxPlatform's SwiftTerm `TerminalPane`. A tab's context menu closes it | `.panehead`, `.term` |
| `ConnectHostSheet(state:, host:, send:)` | File › Connect to Host… (T52.21): a `font.body` semibold title, a `font.caption` line in `text.secondary` saying no key and no ssh agent leave the Mac, the alias field in `font.monoInline` beside `server.rack` in a `fill.primary` well at `radius.m`, `size.buttonHeight` tall; the core's failure line in `status.danger`; Cancel (secondary, Esc) and Connect (primary, Return, disabled while empty or connecting); `space.xl` padding, `size.popoverWidth` wide | — |
| `PluginPanel(items)` | the plugin `panel` slot above the composer (PL§8, T52.17): per shown panel, a header naming the plugin on `fill.primary` under a top hairline, `font.caption` in `text.secondary`, over its PluginWidgetView on `surface.window`, at most eight `font.monoCode` lines tall and scrolling past that. The app shows an `overlay` slot's tree in a sheet that Esc closes | — |
| `BrowserPaneChrome(state:, send:, content:)` | the browser pane beside the transcript (mockup 25, T51.10): a `Size.toolbarHeight` bar with a back chevron in `text.secondary`; the address in a sunken `fill.primary` well (`font.caption`, `radius.m`) led by a `Spinner` while the page loads or a lock for `https`; and a reload icon capsule. Below, the `content` slot in a `radius.xl` well with a hairline rim, where the app puts WebKit's `WebView` over CoxPlatform's `BrowserController.page`. Return in the field reports what was typed; Rust checks it | web column of 25 |
| `MenuBarPanel(state:, send:)` | the menu-bar extra's panel (mockup 26, T51.13), 360 pt wide: a "Needs you" `SectionHeader` over one row per waiting inbox item led by a waiting `StatusDot` — an approval shows its title over the command in `font.monoInline` with small primary Allow and secondary Deny buttons, a question its title over "asks: …" and opens its session on click; a "Running" section of rows led by a `Spinner`, the title over activity · time · cost in `font.footnote`, each opening its session; `Hairline` rules between sections; then "Today" with the day's figures, New session… ⌥⌘N and Open cox ⌘O. With nothing waiting or running it says "Nothing needs you". Allow for the session and editing a command stay in the app (DT§5.6). No glass of its own: it sits on the extra's window | `.popover` of 26 |
| `CommandPalette(state:, send:)`, `.commandPalette(_:send:)` | ⌘K's palette (mockup 12, DT§5.5, T37.44.13): a `magnifyingglass` and the query in `font.title.palette` with an `esc` KeyCap over a Hairline, then a SectionHeader per group (Actions, Sessions, Commands & files) over its rows — the symbol (or `/` glyph) in a `size.paletteIcon` `fill.secondary` well at `radius.s`, the title in `font.transcript` with the characters the query matched bold, the keys or `project · when` in footnote `text.secondary` trailing; the selected row `text.onAccent` on an `accent` face at `radius.l`, with no well; "No matches" in footnote when nothing is left. `size.paletteWidth` wide on readable `surface.popover` glass at `radius.pane`, hairline, e4. The modifier lays it two toolbar heights down over a `shadow.scrim` that a click dismisses. Rows, their order and the matched characters are `cox_app::palette::rank`'s | `.palette`, `.scrim` |

### 6.5 The glass main screen, decomposed

`MainScreen` = `ShellPane(.window)` holding `Sidebar` + `SessionToolbar` + `ShellPane(.column)`
(`TranscriptView` + `Composer`) + `Inspector`, with `AppearancePopover` or `TokenPopover` as popovers.
It holds no styling of its own; it takes `MainScreenState` and reports `MainScreenIntent`.

`SettingsScreen` = `ShellPane(.window)` holding `SettingsSidebar` + `ShellPane(.column)` with the page's
title (`font.transcript.h1`: the mockup's 20 pt `h1` has no token) over one
`SettingsGroupBox` per config table of the selected page: the provider's `KeyRow` — whether the Keychain
holds a key, never the key, and Add key (primary) or Change key (secondary) opening `KeySheet`, a secure
field whose value goes only to `.storeKey` — then a `SettingRow` per setting (LabeledToggle, LabeledSlider,
CoxSegmented, SettingPopUp or SettingField). It takes
`SettingsScreenState` and reports `SettingsScreenIntent`; `CoxModel`'s `SettingsStore.tables(in:)` holds
the titles, details and controls the app copies into that state. Each page's tile is a macOS system
colour, flat as in the mockup (A96): General `systemGray`, Models & Providers `systemPurple`,
Permissions `systemOrange`, Sandbox `systemGreen`, Budget `systemTeal`, MCP Servers `systemBlue`,
Plugins `systemIndigo`, Appearance `systemPink`, Advanced `systemBrown`; the values are SwiftUI's
colours resolved on macOS 27.0 per scheme and contrast (checked 2026-09-28). The MCP page opens with a
`Logins` `SettingsGroupBox`: per server a `TitledSetting` with its login line and a small
`CoxButtonStyle` Log in (primary) or Log out (secondary) button, from `SettingsStore.logins`. The General
page ends with a `Shortcuts` `SettingsGroupBox` (T51.15): per global hotkey a `TitledSetting` whose
control is the app's recorder, passed in as `SettingsScreen(state:recorder:send:)`'s slot because CoxUI
does not link the hotkey library; with no recorder the box is left out. A page
whose keys the project file tried to weaken opens with a `Dropped from the project` `SettingsGroupBox`:
per value a `TitledSetting` with the key, the guard's reason and a warning `Badge` (`999 → 5`), from
`SettingsStore.dropped(in:)`.

`OnboardingScreen` = `ShellPane(.window)` holding the `ProjectDropZone` step and a `SettingsGroupBox`
with a `ChecklistRow` per check (DT§5.8), neither headed, as in the mockup. It takes `OnboardingScreenState` (the rows `cox-app`'s
`checklist` returns: provider key, git, sandbox, shell environment, each with what is missing and an
optional fix) and reports `OnboardingScreenIntent` (`chooseFolder`, `openFolder(url)` for a dropped folder or the one
the picker chose, `openSettings`, `retry`). The
mockup's welcome hero needs a title token and the app icon, which do not exist yet.

## 7. Data shown in the token meter

- ↑ **sent** = input + cache read + cache write; ↓ **received** = output, including thinking. Session
  totals come from the cost ledger (one `usage` row per request); per-turn values are the same rows
  filtered by turn.
- **tok/s** during streaming is estimated in `cox-app` from output deltas (`cox-tokens` estimate over a
  rolling window) and replaced by the exact figure when the request's usage arrives. The UI never
  computes it.
- The context bar draws the core's context breakdown (A98: `Event::ContextBreakdown`, formatted by
  `cox-app` into the share of the window and the system, tools, instruction files and history
  parts), the same split the TUI's status line and `/context` overlay show (T37.25.3).

## 8. Accessibility

- Every interactive element has a label; icon-only buttons have a tooltip with their shortcut.
- Text contrast is at least 4.5:1 against its surface in every material and appearance; the snapshot
  suite checks the Frosted renders. For Frosted and Glossy the surface is the glass laid over the
  window fill — each pane's tint at the material's window opacity over opaque `surface.window` — the
  worst case cox can predict, since the wallpaper behind the window is unknown; the snapshot
  wallpaper is for looks only (A112). CoxUI's `ContrastTests` computes each pair it names on that
  backdrop for all three materials in both appearances.
- VoiceOver rotors: Approvals, Tool calls, Errors. The token meter reads as "218 thousand tokens sent,
  9.8 thousand received, 71 tokens per second".
- Honour Reduce Transparency, Reduce Motion and Increase Contrast (§1.6). `high-contrast.mjs` derives
  the Increase Contrast palettes `color.*-hc.json` (A89) and checks them in `just desktop-tokens`: text
  at least 7:1 on its surface, borders solid and at least 3:1, glass more opaque by
  `material.highContrast.glassKeep`, the number CoxUI's materials use too (§3.5). A token that stands
  for a macOS system colour pins the system's own Increase Contrast variant in
  `$extensions.cox.highContrast` (A96); the check still holds it to its ratio, moving the glyph on it
  instead.

## 9. Rules for agents generating SwiftUI

Before writing a view:

1. Find the element in §6 (search by the mockup's CSS class). If it exists, use it. If it nearly
   exists, add a variant to it. Only if nothing fits, add a new row to §6 in the same change.
2. Build bottom-up: atoms, then molecules, then organisms. A screen file only composes.

While writing:

- Use generated tokens only: `Color(.<role>)`, `Space.<step>`, `Radius.<step>`, `Size.<name>`,
  `.textStyle(.<token>)`, `.elevation(.<level>)`, `Motion.<token>`. No `Color(red:…)`, `.padding(12)`,
  `.font(.system(size:…))`, `.shadow(…)`, `.cornerRadius(…)` or `withAnimation(.easeIn(duration:…))`
  with literals.
- Subviews are `struct`s, not computed properties or `@ViewBuilder` functions, so SwiftUI can skip them.
  Keep `body` short; extract when a view does two things.
- Inputs are value types from `CoxModel`; a component never imports `CoxCore` (the FFI) and never owns
  business state. Local UI state (hover, disclosure open) is `@State` inside the component.
- Lists use stable ids from Rust; row state is `Equatable`.
- One component per file, named after the component.

Before finishing:

- `#Preview` for every variant × light/dark × Solid/Frosted, using `Previews/PreviewState`.
- Snapshot tests (swift-snapshot-testing 1.19.6, checked on GitHub 2026-09-28) for the same matrix.
  A missing reference is recorded and fails once; `SNAPSHOT_TESTING_RECORD=all swift test` re-records
  after an intended change, and a second run must pass.
- SwiftLint (0.65.1, checked on GitHub 2026-09-28) passes, including the custom rules that reject
  literal colours, sizes, fonts, radii, shadows and durations outside `Tokens/` and `Foundations/`.
- If you added or changed a token, the drift test passes and `tokens/tokens.css` is regenerated.
