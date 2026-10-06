# Desktop mockups

HTML mockups of the macOS client (`docs/design/desktop.md`, `../DESIGN.md`). One page holds every
screen; the URL hash picks it (`mockups.html#01-main-session-streaming`, `#__list` lists them).

Render screens to 2x PNGs (headless Chrome, 1520×980 window) into `screens/`, which is not committed:

```bash
./render.sh 28-main-glass-frosted 29-main-glass-glossy 30-main-glass-tokens 31-main-glass-dark-frosted 32-main-glass-dark-glossy
```

Screens 01–27 are the solid light/dark set; 28–30 are the glass main screen (frosted, glossy, token
popover) and 31–32 the same screen in dark glass (frosted, glossy). The colours come from `../tokens/tokens.css`, which `just desktop-tokens` generates from
`../tokens/`; the page gives them short names and keeps only mockup-only values (wallpaper, window
shadow, glass materials) inline.

Note (T60.6, A138): the mockups still draw the model capsule and the Ask/Plan/Auto control in the toolbar. In the
app they are the composer's model chip (opens the model popover over it) and mode chip (opens a menu of Ask, Plan,
Auto and Bypass); the toolbar keeps neither. `docs/design/desktop.md` DT§5.1 and DT§5.3 and `../DESIGN.md` §6.4 are
the source for the placement.
