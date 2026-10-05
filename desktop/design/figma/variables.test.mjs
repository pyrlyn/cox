// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Tests for the Figma token scripts (T37.44.1): every token reaches Figma in every mode, each script
// fits `use_figma`'s limit, and running the scripts twice changes nothing — checked against an
// in-memory stand-in for the Plugin API calls the scripts make.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { COLOR_MODES, FIGMA_FONT, chunks, collections, loadTokens, sizeScope, styles, walk } from './variables.mjs';

const tokens = await loadTokens();
const AsyncFunction = (async () => {}).constructor;
const name = (path) => path.filter((p) => p !== '$root').join('/');

function fakeFigma({ fonts = ['Inter', 'Roboto Mono'], rendered = fonts } = {}) {
  let next = 1;
  const id = (kind) => `${kind}:${next++}`;
  const cols = [];
  const vars = [];
  const texts = [];
  const effects = [];
  const removable = (list, item) => () => list.splice(list.indexOf(item), 1);
  return {
    state: { cols, vars, texts, effects },
    loadFontAsync: async ({ family }) => {
      if (!fonts.includes(family)) throw new Error(`no font ${family}`);
    },
    createText() {
      const t = { characters: '', remove() {} };
      Object.defineProperty(t, 'hasMissingFont', { get: () => !rendered.includes(t.fontName.family) });
      return t;
    },
    getLocalTextStylesAsync: async () => [...texts],
    getLocalEffectStylesAsync: async () => [...effects],
    createTextStyle() {
      const s = { id: id('text'), name: '', bound: {}, setBoundVariable: (field, v) => (s.bound[field] = v.id) };
      s.remove = removable(texts, s);
      texts.push(s);
      return s;
    },
    createEffectStyle() {
      const s = { id: id('effect'), name: '' };
      s.remove = removable(effects, s);
      effects.push(s);
      return s;
    },
    variables: {
      getLocalVariableCollectionsAsync: async () => [...cols],
      getLocalVariablesAsync: async () => [...vars],
      createVariableCollection(collection) {
        const c = {
          id: id('col'),
          name: collection,
          modes: [{ modeId: id('mode'), name: 'Mode 1' }],
          renameMode: (modeId, to) => (c.modes.find((m) => m.modeId === modeId).name = to),
          addMode: (to) => {
            const modeId = id('mode');
            c.modes.push({ modeId, name: to });
            return modeId;
          },
          get variableIds() {
            return vars.filter((v) => v.variableCollectionId === c.id).map((v) => v.id);
          },
        };
        cols.push(c);
        return c;
      },
      createVariable(variable, collection, resolvedType) {
        const v = {
          id: id('var'),
          name: variable,
          variableCollectionId: collection.id,
          resolvedType,
          valuesByMode: {},
          codeSyntax: {},
          setValueForMode: (modeId, value) => (v.valuesByMode[modeId] = value),
          setVariableCodeSyntax: (platform, syntax) => (v.codeSyntax[platform] = syntax),
        };
        v.remove = removable(vars, v);
        vars.push(v);
        return v;
      },
    },
  };
}

const run = async (figma) => {
  const reports = [];
  for (const code of chunks(tokens)) reports.push(...(await new AsyncFunction('figma', code)(figma)));
  return reports;
};

test('a_size_scope_anchors_hairline_quoteBar_and_Ring', () => {
  assert.deepEqual(sizeScope('hairline'), ['STROKE_FLOAT', 'WIDTH_HEIGHT']);
  assert.deepEqual(sizeScope('quoteBar'), ['STROKE_FLOAT', 'WIDTH_HEIGHT']);
  assert.deepEqual(sizeScope('statusDotRing'), ['STROKE_FLOAT', 'WIDTH_HEIGHT']);
  assert.deepEqual(sizeScope('paneGap'), ['GAP']);
  // The unanchored pattern treated any name containing "hairline" as a stroke.
  assert.deepEqual(sizeScope('hairlineWidth'), ['WIDTH_HEIGHT']);
  assert.deepEqual(sizeScope('quoteBarInner'), ['WIDTH_HEIGHT']);
});

test('every_token_becomes_a_variable_or_style_and_every_colour_has_all_four_modes', () => {
  const vars = new Map(collections(tokens).flatMap((c) => c.variables.map((v) => [v.name, { ...v, modes: c.modes }])));
  const { text, effect } = styles(tokens);
  for (const file of Object.keys(COLOR_MODES)) {
    for (const [path] of walk(tokens.colors[file])) {
      const v = vars.get(name(path));
      assert.ok(v, `no variable for ${file}:${path.join('.')}`);
      assert.deepEqual(Object.keys(v.values).sort(), Object.values(COLOR_MODES).sort(), name(path));
    }
  }
  for (const [path, token] of walk(tokens.base)) {
    if (token.$type === 'typography') {
      assert.ok(text.some((s) => s.name === name(path)), `no text style for ${name(path)}`);
      for (const part of ['family', 'size', 'weight', 'lineHeight', 'letterSpacing']) assert.ok(vars.has(`${name(path)}/${part}`), `${name(path)}/${part}`);
    } else if (token.$type === 'shadow') {
      assert.ok(effect.some((s) => s.name === name(path)), `no effect style for ${name(path)}`);
    } else {
      assert.ok(vars.has(name(path)), `no variable for ${name(path)}`);
    }
  }
  for (const v of vars.values()) assert.ok(!v.scopes.includes('ALL_SCOPES'), `${v.name} is in every picker`);
});

test('every_script_fits_use_figma_and_parses', () => {
  const all = chunks(tokens);
  assert.ok(all.length >= 1);
  for (const code of all) {
    assert.ok(code.length < 50000, `script is ${code.length} characters`);
    assert.doesNotThrow(() => new AsyncFunction('figma', code));
  }
});

test('a_second_run_updates_in_place_and_prunes_what_the_tokens_dropped', async () => {
  const figma = fakeFigma();
  const first = await run(figma);
  const count = figma.state.vars.length;
  const expected = collections(tokens).reduce((n, c) => n + c.variables.length, 0);
  assert.equal(count, expected);
  const color = figma.state.cols.find((c) => c.name === 'Color');
  assert.deepEqual(color.modes.map((m) => m.name), Object.values(COLOR_MODES));

  figma.variables.createVariable('space/gone', figma.state.cols.find((c) => c.name === 'Spacing'), 'FLOAT');
  const second = await run(figma);
  assert.equal(figma.state.vars.length, count);
  assert.equal(figma.state.cols.length, new Set(first.filter((r) => r.collection).map((r) => r.collection)).size);
  assert.equal(second.reduce((n, r) => n + (r.created ?? 0), 0), 0);
  assert.equal(second.find((r) => r.collection === 'Spacing').removed, 1);
  assert.equal(new Set(figma.state.texts.map((s) => s.name)).size, figma.state.texts.length);
});

test('every_text_style_uses_a_stand_in_style_figma_renders_mono_included', async () => {
  const { text } = styles(tokens);
  const stand = Object.values(FIGMA_FONT);
  for (const s of text) assert.ok(stand.some((f) => f.family === s.family && f.styles.includes(s.style)), `${s.name}: ${s.family} ${s.style}`);
  assert.ok(text.some((s) => s.family === 'Roboto Mono'), 'no mono text style');
  const figma = fakeFigma();
  const report = (await run(figma)).find((r) => r.styles);
  assert.deepEqual([report.missingFonts, report.unrenderedFonts], [[], []]);
  assert.deepEqual(figma.state.texts.map((s) => s.name), text.map((s) => s.name));
});

test('a_font_figma_lacks_skips_its_text_style_and_says_so', async () => {
  const figma = fakeFigma({ fonts: ['Inter'] });
  const report = (await run(figma)).find((r) => r.styles);
  const mono = styles(tokens).text.filter((s) => s.family === 'Roboto Mono').map((s) => s.name);
  assert.ok(mono.length > 0);
  assert.deepEqual(report.missingFonts.map((m) => m.split(':')[0]), mono);
  assert.ok(figma.state.texts.every((s) => !mono.includes(s.name) && s.bound.fontSize));
});

test('a_font_figma_lists_but_cannot_render_is_reported_and_kept', async () => {
  const figma = fakeFigma({ rendered: [] });
  const report = (await run(figma)).find((r) => r.styles);
  const all = styles(tokens).text.map((s) => s.name);
  assert.deepEqual(report.unrenderedFonts.map((m) => m.split(':')[0]), all);
  assert.deepEqual(figma.state.texts.map((s) => s.name), all);
});

test('a_drop_shadow_is_not_painted_under_its_own_box', () => {
  for (const { effects } of styles(tokens).effect) {
    for (const e of effects) assert.equal(e.showShadowBehindNode, e.type === 'DROP_SHADOW' ? false : undefined);
  }
});
