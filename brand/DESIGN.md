# cox brandbook

Only what is specific to cox. Everything else is the [Pyrlyn base layer](https://github.com/pyrlyn/brand/blob/v0.3.0/base/DESIGN.md) from `@pyrlyn/brand` (pinned in `package.json`).
Tokens: [`tokens.json`](tokens.json) (extends `@pyrlyn/brand/base/tokens.json`), built to
`dist/tokens.css` by `node build.mjs`. Source: `pyrlyn/cox` at `03825e72`, `docs/brand/` (DESIGN.md,
tokens.css, logos) and `docs/assets/landing-v1/` (PNG icons).

## Idea

The coxswain that steers: a terminal TUI pane with a chartreuse cursor. Terminal chrome (TUI
frames, status lines, pane borders) over generic glass cards.

## Colour

- Light ("paper terminal") is the default; dark ("classic green-on-ink") is first-class.
- Accent: forest `#3D8B3A` on light, chartreuse `#A8E06C` on dark. As text (links) use `accent-fg`:
  `#2F6F2C` on light (`#3D8B3A` is 3.92:1 on `bg`), chartreuse on dark. Terminal text `#C8F08A`.
- Mark tile and code background: forest ink `#0F1A14`.
- Status colours are defined (`success`, `warn`, `danger`) plus `--cox-info`. Text uses the `*-fg`
  variants (light `#2F6F2C`, `#7F5D07`, `#A13A34`; the light fills are below 4.5:1 as text on `surface-2`).
- Distinct from ketch: deeper forest + chartreuse, never bright teal/mint. No purple AI glow.
- Role mapping from the cox names: `bg-elevated`→`surface`, `accent-soft`→`accent-muted`,
  `warning`→`warn`, `shadow`→`shadow-e1`. cox's own `accent-muted` (solid `#6BBF4A`) is
  `--cox-accent-leaf`. Kept as `--cox-*`: `surface-1`, `border-hairline`, `accent-hover`,
  `chartreuse`, `info`, `code-bg`, `code-fg`, `glass-fill`, `glass-border`, `accent-glow`.
- `on-accent` (text on an accent fill): `#060A08` on light (4.70:1 on `#3D8B3A`; white is 4.24:1 and
  forest `#0F1A14` 4.20:1, both below AA), forest `#0F1A14` on dark (11.50:1 on chartreuse). The light
  `accent-hover` `#2F6F2C` is darker, so a small dark label drops to 3.25:1 on hover: lighten on hover
  (e.g. `accent-leaf` `#6BBF4A`) or keep hover labels large.

## Type and shape

cox uses IBM Plex Mono for UI (as the base) and IBM Plex Sans for docs body only; its own scale is
12/14/16/20/28/40 and radii 8/12. Those are cox site values and are not part of the shared base.

## Logo

| File (`logo/`) | Use |
|---|---|
| `cox-mark.svg` | Mark 64×64: forest tile, pane frame, chartreuse cursor |
| `cox-wordmark.svg` | Mark + `cox` in IBM Plex Mono 600 (live text) |
| `cox-favicon.svg` | Favicon |
| `png/cox-favicon-{32x32,64x64}.png`, `png/cox-icon-favicon-256.png` | Rendered from `cox-favicon.svg` by `just brand-icons` |
| `png/cox-apple-touch-icon-180.png`, `png/cox-icon-logo-{512,1024}.png` | Rendered from `cox-mark.svg` by `just brand-icons` |

Don't revive the helm wheel: the mark is a terminal pane, not nautical.
