// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The High Contrast palettes (A89, T37.17.1): derives `tokens/color.light-hc.json` and
// `tokens/color.dark-hc.json` from the light and dark palettes by one rule, then reads the written
// files back and checks every pair. Separate from the Style Dictionary build because it writes that
// build's input; `npm run build` (`just desktop-tokens`) runs it first, so CI's drift job re-derives
// and re-checks. `node high-contrast.mjs --check` checks the committed files without writing.
//
// The rule: text is at least 7:1 on every surface it sits on; hairlines and borders are solid and at
// least 3:1, as are the other graphics a person must tell apart; glass keeps a quarter of its
// transparency — `material.highContrast.glassKeep` in `tokens/base.json`, the number CoxUI's
// `Appearance` applies to the window and pane opacity (A100), read here so both follow one token.
// A colour that already passes is kept; one that fails moves the least distance towards black or
// white (away from its surface) that passes. The specular sweep's strength and the window's own
// opacity are `material.*` numbers, not colours, so no palette file can switch them; the sweep's
// colour, `glass.specular`, is kept, and `Appearance` drops the sweep under Increase Contrast.
import { readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const tokensDir = join(dirname(fileURLToPath(import.meta.url)), 'tokens');
const MODES = { 'light-hc': 'light', 'dark-hc': 'dark' };
const TEXT = 7;
const GRAPHIC = 3;
const STEP = 0.005;

// Glass: translucent surfaces. Each sits over the opaque window fill of its palette, as the palette
// describes them; what shows through the window itself is the material's opacity, not a colour.
// `surface.terminal` is opaque (A129 (8)), so it is no glass and keeps its value; text is still
// checked on it below. It is no page either: only terminal text sits on it.
const GLASS = ['surface.sidebar', 'surface.capsule', 'surface.popover', 'glass.fill'];
const PAGE = ['surface.window', 'surface.code', ...GLASS];
const WORDS = [
  'text.primary', 'text.secondary', 'text.tertiary', 'text.placeholder', 'accent', 'status.success',
  'status.warning', 'status.danger', 'status.plan', 'meter.sent', 'meter.received',
];
const CODE = ['syntax.keyword', 'syntax.string', 'syntax.number', 'syntax.function', 'syntax.comment', 'syntax.type'];
const onCode = (...fills) => ['surface.code', ...fills.map((f) => `${f}@surface.code`)];
const TILES = ['neutral', 'edit', 'shell', 'search', 'write', 'app'];
const PAGES = ['general', 'models', 'permissions', 'sandbox', 'budget', 'mcp', 'plugins', 'appearance', 'advanced'];

// Which colour sits on which. `fill@surface` is a translucent fill over that surface. `move: 'bg'`
// derives the background instead: a tile's white glyph is already as light as it goes.
const RULES = [
  {
    fg: WORDS,
    bg: [...PAGE, 'fill.primary@surface.window', 'accent.soft@surface.window', 'accent.soft@surface.sidebar',
      'accent.selected@surface.sidebar', 'status.warningSoft@surface.window', 'status.dangerSoft@surface.window'],
    min: TEXT,
  },
  // Code and diff lines; a changed line's number is `text.primary` on its gutter (DS§6).
  { fg: [...CODE, 'text.primary'], bg: onCode('diff.add', 'diff.del'), min: TEXT },
  { fg: ['text.primary'], bg: onCode('diff.addGutter', 'diff.delGutter'), min: TEXT },
  { fg: ['text.terminal', 'text.terminalOk'], bg: ['surface.terminal'], min: TEXT },
  // A badge's label on its own tint: the project badge, each RiskChip level (DS§6.2).
  ...['role.project', 'risk.low', 'risk.medium', 'risk.high'].map((fg) => ({
    fg: [fg], bg: [...PAGE, `${fg}Soft@surface.window`], min: TEXT,
  })),
  // A label on a filled face: the primary button, the Bypass segment, the count badge.
  { fg: ['text.onAccent'], bg: ['accent', 'status.danger', 'status.warning'], min: TEXT },
  { fg: ['separator', 'surface.capsuleBorder', 'glass.border'], bg: PAGE, min: GRAPHIC, solid: true },
  // A quote's bar is a graphic a person reads the quote's depth from (A97).
  { fg: ['quote.bar'], bg: PAGE, min: GRAPHIC, solid: true },
  { fg: ['context.system', 'context.tools', 'context.instructions', 'context.history'], bg: PAGE, min: GRAPHIC },
  ...TILES.map((t) => ({ fg: [`tile.${t}.glyph`], bg: [`tile.${t}.top`, `tile.${t}.bottom`], min: GRAPHIC, move: 'bg' })),
  // A Settings page tile's face is pinned (below), so its glyph moves instead.
  ...PAGES.map((p) => ({ fg: [`tile.settings.${p}.glyph`], bg: ['top', 'bottom'].map((e) => `tile.settings.${p}.${e}`), min: GRAPHIC })),
];
// Tints and depth with nothing to read on them; A89 sets no ratio, so they keep their value.
const KEPT = ['fill.secondary', 'status.successSoft', 'shadow.tint', 'shadow.scrim', 'glass.highlight', 'glass.specular'];

const fail = (msg) => {
  throw new Error(`high contrast: ${msg}`);
};
// A96: a colour that stands for a macOS system colour carries the system's own Increase Contrast
// variant, so High Contrast follows the system instead of the rule; the rule still checks it.
const pinned = (token) => token.$extensions?.cox?.highContrast;

const walk = (group, path = [], out = new Map()) => {
  for (const [key, node] of Object.entries(group)) {
    if (key.startsWith('$') && key !== '$root') continue;
    const at = key === '$root' ? path : [...path, key];
    if (node?.$type === 'color') out.set(at.join('.'), node);
    else if (node && typeof node === 'object') walk(node, at, out);
  }
  return out;
};

const rgba = (token, where) => {
  const { hex, alpha = 1 } = token.$value;
  if (!/^#[0-9a-f]{6}$/i.test(hex ?? '')) fail(`${where}: expected a #rrggbb hex`);
  return { rgb: [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)), alpha };
};
const over = (top, under) => top.rgb.map((c, i) => Math.round(c * top.alpha + under[i] * (1 - top.alpha)));
const mix = (rgb, to, t) => rgb.map((c) => Math.round(c + (to - c) * t));
const linear = (c) => (c / 255 <= 0.04045 ? c / 255 / 12.92 : ((c / 255 + 0.055) / 1.055) ** 2.4);
const luminance = (rgb) => 0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2]);
const ratio = (a, b) => {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};
const hex = (rgb) => `#${rgb.map((c) => c.toString(16).padStart(2, '0')).join('')}`;

// The opaque colour a background shows: its layers composited, bottom up, over the window fill.
const shown = (spec, colors) =>
  spec
    .split('@')
    .reverse()
    .reduce((under, p) => over(colors.get(p) ?? fail(`unknown colour ${p}`), under), colors.get('surface.window').rgb);

const pairs = () => RULES.flatMap((r) => r.fg.flatMap((fg) => r.bg.map((bg) => ({ ...r, fg, bg }))));
const contrast = (fg, bg, colors) => {
  const under = shown(bg, colors);
  return ratio(over(fg, under), under);
};

// Moves `name` towards black or white, whichever stands further from every colour it is compared
// with, by the smallest step that makes every one of `checks` pass.
const nudge = (name, colors, checks, awayFrom) => {
  const start = colors.get(name);
  const reach = (end) => Math.min(...awayFrom.map((c) => ratio([end, end, end], c)));
  const end = reach(255) > reach(0) ? 255 : 0;
  for (let t = 0; t <= 1 + 1e-9; t += STEP) {
    colors.set(name, { ...start, rgb: mix(start.rgb, end, t) });
    if (checks.every((p) => contrast(colors.get(p.fg), p.bg, colors) >= p.min)) return;
  }
  fail(`${name} cannot reach its ratio`);
};

const derive = (source) => {
  const colors = new Map([...walk(source.color)].map(([p, t]) => [p, rgba(pinned(t) ? { $value: { hex: pinned(t) } } : t, p)]));
  for (const g of GLASS) {
    const c = colors.get(g);
    colors.set(g, { ...c, alpha: Number((1 - (1 - c.alpha) * GLASS_KEEP).toFixed(3)) });
  }
  const all = pairs();
  for (const r of RULES.filter((r) => r.solid)) {
    for (const fg of r.fg) colors.set(fg, { rgb: over(colors.get(fg), colors.get('surface.window').rgb), alpha: 1 });
  }
  for (const r of RULES.filter((r) => r.move === 'bg')) {
    for (const bg of r.bg) nudge(bg, colors, all.filter((p) => p.bg === bg), r.fg.map((f) => colors.get(f).rgb));
  }
  const movers = [...new Set(RULES.filter((r) => r.move !== 'bg').flatMap((r) => r.fg))];
  for (const fg of movers) {
    const checks = all.filter((p) => p.fg === fg);
    nudge(fg, colors, checks, checks.map((p) => shown(p.bg, colors)));
  }
  return colors;
};

// The HC file is the source file with each colour's value replaced, descriptions kept.
const render = (source, colors, from) => {
  const out = structuredClone(source);
  out.$description = `Generated by desktop/design/high-contrast.mjs from color.${from}.json (A89). Do not edit.`;
  for (const [path, token] of walk(out.color)) {
    const { rgb, alpha } = colors.get(path);
    delete token.$extensions;
    token.$value = {
      colorSpace: 'srgb',
      components: rgb.map((c) => Number((c / 255).toFixed(4))),
      hex: hex(rgb),
      ...(alpha < 1 ? { alpha } : {}),
    };
  }
  return `${JSON.stringify(out, null, 2)}\n`;
};

// Checks the written files on their own: every colour is covered, every pair passes, borders are
// solid, glass is more opaque and nothing the rule does not name has changed.
const check = (source, hc, mode) => {
  const base = walk(source.color);
  const colors = new Map([...walk(hc.color)].map(([p, t]) => [p, rgba(t, `${mode} ${p}`)]));
  const named = new Set([...GLASS, ...KEPT, ...RULES.flatMap((r) => [...r.fg, ...r.bg.flatMap((b) => b.split('@'))])]);
  for (const path of base.keys()) {
    if (!named.has(path)) fail(`${mode} ${path}: no High Contrast rule names it`);
    if (!colors.has(path)) fail(`${mode} ${path}: missing`);
    const pin = pinned(base.get(path));
    if (pin && hex(colors.get(path).rgb) !== pin.toLowerCase()) fail(`${mode} ${path}: not the pinned ${pin}`);
  }
  for (const g of GLASS) {
    if (!(colors.get(g).alpha > rgba(base.get(g), g).alpha)) fail(`${mode} ${g}: glass is not more opaque`);
  }
  for (const k of KEPT) {
    if (JSON.stringify(rgba(base.get(k), k)) !== JSON.stringify(colors.get(k))) fail(`${mode} ${k}: changed`);
  }
  let least;
  for (const p of pairs()) {
    if (p.solid && colors.get(p.fg).alpha !== 1) fail(`${mode} ${p.fg}: a border must be solid`);
    const r = contrast(colors.get(p.fg), p.bg, colors);
    if (r < p.min) fail(`${mode} ${p.fg} on ${p.bg}: ${r.toFixed(2)}:1 is below ${p.min}:1`);
    if (!least || r / p.min < least.r / least.min) least = { ...p, r };
  }
  return `${mode}: ${pairs().length} pairs pass; closest ${least.fg} on ${least.bg} at ${least.r.toFixed(2)}:1`;
};

const load = async (mode) => JSON.parse(await readFile(join(tokensDir, `color.${mode}.json`), 'utf8'));
const GLASS_KEEP = JSON.parse(await readFile(join(tokensDir, 'base.json'), 'utf8')).material?.highContrast?.glassKeep?.$value;
if (!(typeof GLASS_KEEP === 'number' && GLASS_KEEP >= 0 && GLASS_KEEP < 1)) {
  fail('material.highContrast.glassKeep in base.json must be a number in 0…1 (1 excluded)');
}

for (const [mode, from] of Object.entries(MODES)) {
  const source = await load(from);
  if (!process.argv.includes('--check')) {
    await writeFile(join(tokensDir, `color.${mode}.json`), render(source, derive(source), from));
  }
  console.log(check(source, await load(mode), mode));
}
