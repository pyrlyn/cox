// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Figma side of the desktop tokens (A114, T37.44.1): turns `tokens/*.json` into Figma Plugin API
// scripts that the Figma MCP tool `use_figma` runs against the `cox desktop` file. Separate from
// `style-dictionary.config.mjs` because Figma is a mirror, not a build output: nothing in the
// repository reads it back, and it runs only when a person re-syncs the file.
//
// Each script is self-contained and idempotent: it finds collections, modes, variables and styles by
// name, updates them in place, creates what is missing and removes what the tokens no longer have, so
// re-running it after a token edit is the whole re-sync. `use_figma` caps a script at 50,000
// characters, so the jobs are packed into as few scripts as fit under `LIMIT`.
//
//   node figma/variables.mjs             print every script, each after a `// ==== n/N ====` line
//   node figma/variables.mjs --out DIR   write them to DIR/01.js, DIR/02.js, …
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
export const TOKENS_DIR = join(here, '../tokens');
export const LIMIT = 45000;

// Figma mode name per `color.<mode>.json`, in the order the modes are created (Light is the default).
export const COLOR_MODES = { light: 'Light', dark: 'Dark', 'light-hc': 'Light HC', 'dark-hc': 'Dark HC' };
// Collection per base group; the Swift type is the iOS code syntax prefix (DS§2).
// Every alternative is anchored: `hairline` inside a longer name is not a stroke.
export function sizeScope(name) {
  if (/(?:hairline|quoteBar|Ring)$/.test(name)) return ['STROKE_FLOAT', 'WIDTH_HEIGHT'];
  if (name === 'paneGap') return ['GAP'];
  return ['WIDTH_HEIGHT'];
}

const BASE_GROUPS = {
  space: { collection: 'Spacing', swift: 'Space', scopes: () => ['GAP'] },
  radius: { collection: 'Radius', swift: 'Radius', scopes: () => ['CORNER_RADIUS'] },
  size: {
    collection: 'Size',
    swift: 'Size',
    scopes: sizeScope,
  },
  material: { collection: 'Material', swift: 'MaterialToken', scopes: (name) => (/blur/i.test(name) ? ['EFFECT_FLOAT'] : []) },
  motion: { collection: 'Motion', swift: 'Motion', scopes: () => [] },
  font: { collection: 'Type', swift: 'FontToken' },
};
// A125: Figma draws each code family with a stand-in it renders, because `use_figma` lists SF Pro but
// does not render it and does not list SF Mono. The product and the HTML renders keep SF, so metrics
// are compared there, not in Figma. `styles` holds the family's style names for weights 100…900; DS§3.2
// reads 650 as semibold, so a weight rounds down. A font Figma still lacks is skipped and reported.
export const FIGMA_FONT = {
  'SF Pro Text': { family: 'Inter', styles: ['Thin', 'Extra Light', 'Light', 'Regular', 'Medium', 'Semi Bold', 'Bold', 'Extra Bold', 'Black'] },
  'SF Mono': { family: 'Roboto Mono', styles: ['Thin', 'ExtraLight', 'Light', 'Regular', 'Medium', 'SemiBold', 'Bold'] },
};
const standIn = (family, where) => FIGMA_FONT[family] ?? fail(`${where}: no Figma stand-in for ${family}`);

const fail = (msg) => {
  throw new Error(`figma tokens: ${msg}`);
};
const parts = (path) => path.filter((p) => p !== '$root');
const camel = (path) => parts(path).map((p, i) => (i ? p[0].toUpperCase() + p.slice(1) : p)).join('');
const round = (n) => Math.round(n * 1000) / 1000;

/** Every token under `node` as `[path, token]`, `$root` kept in the path. */
export function* walk(node, path = []) {
  if (node && typeof node === 'object' && '$value' in node) {
    yield [path, node];
    return;
  }
  for (const [key, child] of Object.entries(node ?? {})) {
    if (!key.startsWith('$') || key === '$root') yield* walk(child, [...path, key]);
  }
}

export async function loadTokens(dir = TOKENS_DIR) {
  const read = async (file) => JSON.parse(await readFile(join(dir, file), 'utf8'));
  const colors = {};
  for (const mode of Object.keys(COLOR_MODES)) colors[mode] = await read(`color.${mode}.json`);
  return { base: await read('base.json'), colors };
}

const rgba = (value, where) => {
  if (value?.colorSpace !== 'srgb' || !/^#[0-9a-f]{6}$/i.test(value.hex ?? '')) fail(`${where}: expected an srgb #rrggbb colour`);
  const [r, g, b] = [1, 3, 5].map((i) => round(parseInt(value.hex.slice(i, i + 2), 16) / 255));
  return { r, g, b, a: value.alpha ?? 1 };
};
const px = (dim, where) => (dim?.unit === 'px' && typeof dim.value === 'number' ? dim.value : fail(`${where}: expected a px dimension`));

function colorCollection({ colors }) {
  const modes = Object.values(COLOR_MODES);
  const byName = new Map();
  for (const [file, mode] of Object.entries(COLOR_MODES)) {
    for (const [path, token] of walk(colors[file])) {
      const where = `color.${file}:${path.join('.')}`;
      if (token.$type !== 'color') fail(`${where}: expected $type color`);
      const name = parts(path).join('/');
      if (!byName.has(name)) {
        const role = parts(path.slice(1));
        byName.set(name, {
          name,
          type: 'COLOR',
          values: {},
          scopes: colorScopes(role),
          description: token.$description ?? '',
          code: { WEB: `var(--c-${role.join('-')})`, iOS: `Color(.${camel(path.slice(1))})` },
        });
      }
      byName.get(name).values[mode] = rgba(token.$value, where);
    }
  }
  for (const v of byName.values()) {
    const missing = modes.filter((m) => !v.values[m]);
    if (missing.length) fail(`colour ${v.name} has no value for ${missing.join(', ')}`);
  }
  return { collection: 'Color', modes, variables: [...byName.values()] };
}

function colorScopes([group, role]) {
  if (group === 'text') return ['TEXT_FILL'];
  if (group === 'shadow') return ['EFFECT_COLOR'];
  if (group === 'separator' || role === 'capsuleBorder') return ['STROKE_COLOR', 'SHAPE_FILL'];
  return ['FRAME_FILL', 'SHAPE_FILL', 'TEXT_FILL', 'STROKE_COLOR'];
}

// A typography token becomes five variables Figma can bind on a text style; line height and
// tracking are stored in px because Figma binds them as px.
function typeVariables(path, token, where) {
  const v = token.$value;
  const size = px(v.fontSize, where);
  const family = v.fontFamily?.[0] ?? fail(`${where}: no fontFamily`);
  const tracking = v.letterSpacing === undefined ? 0 : v.letterSpacing.unit === 'em' ? v.letterSpacing.value : fail(`${where}: letterSpacing must be em`);
  const name = parts(path).join('/');
  const code = { iOS: `FontToken.${camel(path.slice(1))}` };
  const one = (suffix, type, value, scopes, description = '') => ({ name: `${name}/${suffix}`, type, values: { Value: value }, scopes, description, code });
  return [
    one('family', 'STRING', standIn(family, where).family, ['FONT_FAMILY'], `Code: ${v.fontFamily.join(', ')}`),
    one('size', 'FLOAT', size, ['FONT_SIZE']),
    one('weight', 'FLOAT', v.fontWeight, ['FONT_WEIGHT']),
    one('lineHeight', 'FLOAT', round(size * v.lineHeight), ['LINE_HEIGHT'], `${v.lineHeight} × size`),
    one('letterSpacing', 'FLOAT', round(size * tracking), ['LETTER_SPACING'], `${tracking} em`),
  ];
}

function baseValue(path, token, where) {
  const v = token.$value;
  switch (token.$type) {
    case 'dimension':
      return ['FLOAT', px(v, where)];
    case 'number':
      return typeof v === 'number' ? ['FLOAT', v] : fail(`${where}: expected a number`);
    case 'duration':
      // Figma has no duration unit on a plain variable; ms matches the token and DS§3.6.
      return ['FLOAT', { ms: 1, s: 1000 }[v?.unit] * v.value || fail(`${where}: expected ms or s`)];
    case 'cubicBezier':
      return Array.isArray(v) && v.length === 4 ? ['STRING', `cubic-bezier(${v.join(', ')})`] : fail(`${where}: expected four control values`);
    default:
      return fail(`${where}: no Figma variable for $type ${token.$type}`);
  }
}

/** The variable collections: Color with four modes, then one single-mode collection per base group. */
export function collections(tokens) {
  const out = new Map();
  for (const [path, token] of walk(tokens.base)) {
    const where = `base:${path.join('.')}`;
    if (token.$type === 'shadow') continue;
    const group = BASE_GROUPS[path[0]] ?? fail(`${where}: no Figma collection for group ${path[0]}`);
    if (!out.has(group.collection)) out.set(group.collection, { collection: group.collection, modes: ['Value'], variables: [] });
    const vars = out.get(group.collection).variables;
    if (token.$type === 'typography') {
      vars.push(...typeVariables(path, token, where));
      continue;
    }
    const [type, value] = baseValue(path, token, where);
    const code = { iOS: `${group.swift}.${camel(path.slice(1))}` };
    if (token.$type === 'dimension') code.WEB = `var(--${parts(path).join('-')})`;
    vars.push({ name: parts(path).join('/'), type, values: { Value: value }, scopes: group.scopes(path.at(-1)), description: token.$description ?? '', code });
  }
  return [colorCollection(tokens), ...out.values()];
}

/** Text styles (bound to the Type variables) and effect styles, one per typography and shadow token. */
export function styles({ base }) {
  const text = [];
  const effect = [];
  for (const [path, token] of walk(base)) {
    const where = `base:${path.join('.')}`;
    const name = parts(path).join('/');
    if (token.$type === 'typography') {
      const v = token.$value;
      const font = standIn(v.fontFamily[0], where);
      const style = font.styles[Math.floor(v.fontWeight / 100) - 1] ?? fail(`${where}: weight ${v.fontWeight}`);
      text.push({ name, family: font.family, style, description: token.$description ?? '' });
    } else if (token.$type === 'shadow') {
      const layers = (Array.isArray(token.$value) ? token.$value : [token.$value]).map((l) => ({
        type: l.inset === true ? 'INNER_SHADOW' : 'DROP_SHADOW',
        color: rgba(l.color, where),
        offset: { x: px(l.offsetX, where), y: px(l.offsetY, where) },
        radius: px(l.blur, where),
        spread: px(l.spread, where),
        visible: true,
        blendMode: 'NORMAL',
        // CSS never paints a box-shadow under its own translucent box; Figma does unless told not to.
        ...(l.inset === true ? {} : { showShadowBehindNode: false }),
      }));
      effect.push({ name, effects: layers, description: token.$description ?? '' });
    }
  }
  return { text, effect };
}

// The code every script shares. `jobs` is the DATA array: variable collections (maybe one slice of
// a collection, with `prune` only on its last slice) and at most one styles job.
const RUNTIME = String.raw`
const rgba = (c) => { const [hex, a] = c.split('/'); const n = (i) => parseInt(hex.slice(i, i + 2), 16) / 255; return { r: n(1), g: n(3), b: n(5), a: a === undefined ? 1 : Number(a) }; };
const report = [];
const allCollections = await figma.variables.getLocalVariableCollectionsAsync();
const allVariables = await figma.variables.getLocalVariablesAsync();
const byName = new Map(allVariables.map((v) => [v.name, v]));
for (const job of DATA) {
  if (job.styles) {
    const r = { styles: 'text+effect', text: 0, effect: 0, bound: 0, removed: 0, missingFonts: [], unrenderedFonts: [] };
    const texts = await figma.getLocalTextStylesAsync();
    for (const s of job.styles.text) {
      const font = { family: s.family, style: s.style };
      try { await figma.loadFontAsync(font); } catch (e) { r.missingFonts.push(s.name + ': ' + s.family + ' ' + s.style); continue; }
      // A font can load yet not render (listed from a desktop but absent where the file renders): probe it.
      const probe = figma.createText();
      probe.fontName = font;
      probe.characters = 'Ag';
      if (probe.hasMissingFont) r.unrenderedFonts.push(s.name + ': ' + s.family + ' ' + s.style);
      probe.remove();
      const st = texts.find((t) => t.name === s.name) ?? figma.createTextStyle();
      st.name = s.name;
      st.description = s.description;
      st.fontName = font;
      for (const [field, suffix] of [['fontSize', 'size'], ['lineHeight', 'lineHeight'], ['letterSpacing', 'letterSpacing'], ['fontFamily', 'family']]) {
        const v = byName.get(s.name + '/' + suffix);
        if (!v) throw new Error('missing variable ' + s.name + '/' + suffix + ': run the Type collection first');
        st.setBoundVariable(field, v);
        r.bound++;
      }
      r.text++;
    }
    const keepText = new Set(job.styles.text.map((s) => s.name));
    for (const t of texts) if (t.name.startsWith(job.styles.textPrefix) && !keepText.has(t.name)) { t.remove(); r.removed++; }
    const effects = await figma.getLocalEffectStylesAsync();
    for (const s of job.styles.effect) {
      const st = effects.find((e) => e.name === s.name) ?? figma.createEffectStyle();
      st.name = s.name;
      st.description = s.description;
      st.effects = s.effects.map((e) => ({ ...e, color: rgba(e.color) }));
      r.effect++;
    }
    const keepEffect = new Set(job.styles.effect.map((s) => s.name));
    for (const e of effects) if (e.name.startsWith(job.styles.effectPrefix) && !keepEffect.has(e.name)) { e.remove(); r.removed++; }
    report.push(r);
    continue;
  }
  let col = allCollections.find((c) => c.name === job.collection);
  if (!col) {
    col = figma.variables.createVariableCollection(job.collection);
    col.renameMode(col.modes[0].modeId, job.modes[0]);
    allCollections.push(col);
  }
  const modeIds = {};
  for (const m of job.modes) modeIds[m] = (col.modes.find((x) => x.name === m) ?? { modeId: col.addMode(m) }).modeId;
  const r = { collection: col.name, id: col.id, modes: col.modes.map((m) => m.name), created: 0, updated: 0, removed: 0 };
  for (const [name, type, values, scope, description, web, ios] of job.variables) {
    let v = byName.get(name);
    if (v && (v.variableCollectionId !== col.id || v.resolvedType !== type)) { v.remove(); v = undefined; }
    if (v) r.updated++;
    else { v = figma.variables.createVariable(name, col, type); byName.set(name, v); r.created++; }
    job.modes.forEach((m, i) => v.setValueForMode(modeIds[m], type === 'COLOR' ? rgba(values[i]) : values[i]));
    v.scopes = job.scopes[scope];
    v.description = description;
    if (web) v.setVariableCodeSyntax('WEB', web);
    v.setVariableCodeSyntax('iOS', ios);
  }
  if (job.prune) {
    const keep = new Set([...job.prune, ...job.variables.map((d) => d[0])]);
    for (const v of await figma.variables.getLocalVariablesAsync()) if (v.variableCollectionId === col.id && !keep.has(v.name)) { v.remove(); r.removed++; }
  }
  r.total = col.variableIds.length;
  report.push(r);
}
return report;
`;

// The scripts carry the model in a compact form, since every character counts against the limit:
// colours as #rrggbb[/alpha], a variable as [name, type, values in mode order, scope index, description,
// web syntax, iOS syntax] with the scope lists shared per job.
const hex = ({ r, g, b, a }) => `#${[r, g, b].map((n) => Math.round(n * 255).toString(16).padStart(2, '0')).join('')}${a < 1 ? `/${a}` : ''}`;
const wire = (job) => {
  if (job.styles) {
    const effect = job.styles.effect.map((s) => ({ ...s, effects: s.effects.map((e) => ({ ...e, color: hex(e.color) })) }));
    return { styles: { ...job.styles, effect } };
  }
  const scopes = [...new Set(job.variables.map((v) => JSON.stringify(v.scopes)))];
  const variables = job.variables.map((v) => [
    v.name,
    v.type,
    job.modes.map((m) => (v.type === 'COLOR' ? hex(v.values[m]) : v.values[m])),
    scopes.indexOf(JSON.stringify(v.scopes)),
    v.description,
    v.code.WEB ?? '',
    v.code.iOS,
  ]);
  return { collection: job.collection, modes: job.modes, scopes: scopes.map((x) => JSON.parse(x)), variables, ...(job.prune ? { prune: job.prune } : {}) };
};
const script = (jobs) => `// Generated by desktop/design/figma/variables.mjs from desktop/design/tokens. Run with use_figma.\nconst DATA = ${JSON.stringify(jobs.map(wire))};\n${RUNTIME}`;

/** Every job, in run order: collections sliced to fit `limit`, then the styles job. */
export function jobs(tokens, limit = LIMIT) {
  const out = [];
  for (const c of collections(tokens)) {
    const slices = [[]];
    for (const v of c.variables) {
      const next = [...slices.at(-1), v];
      if (slices.at(-1).length && script([{ ...c, variables: next }]).length > limit / 2) slices.push([v]);
      else slices[slices.length - 1] = next;
    }
    slices.forEach((variables, i) => {
      const job = { collection: c.collection, modes: c.modes, variables };
      if (i === slices.length - 1) job.prune = slices.slice(0, -1).flat().map((v) => v.name);
      out.push(job);
    });
  }
  const { text, effect } = styles(tokens);
  out.push({ styles: { text, effect, textPrefix: 'font/', effectPrefix: 'elevation/' } });
  return out;
}

/** The scripts to run in order: jobs packed greedily, each script at most `limit` characters. */
export function chunks(tokens, limit = LIMIT) {
  const packed = [[]];
  for (const job of jobs(tokens, limit)) {
    if (packed.at(-1).length && script([...packed.at(-1), job]).length > limit) packed.push([job]);
    else packed.at(-1).push(job);
  }
  return packed.map((group) => {
    const code = script(group);
    if (code.length > limit) fail(`one job is ${code.length} characters, over the ${limit} limit`);
    return code;
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const all = chunks(await loadTokens());
  const at = process.argv.indexOf('--out');
  if (at > 0) {
    const dir = process.argv[at + 1] ?? fail('--out needs a directory');
    await mkdir(dir, { recursive: true });
    for (const [i, code] of all.entries()) await writeFile(join(dir, `${String(i + 1).padStart(2, '0')}.js`), code);
  } else {
    for (const [i, code] of all.entries()) process.stdout.write(`// ==== ${i + 1}/${all.length} ====\n${code}\n`);
  }
}
