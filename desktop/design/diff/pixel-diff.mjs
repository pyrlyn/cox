// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Pixel diff of a CoxUI snapshot against its Figma or mockup frame (A119, DS§2).
// sharp decodes and resamples (a 2x frame against a 1x snapshot needs a real downscale, not
// nearest-neighbour), pixelmatch decides which pixels differ; only the grouping of differing
// pixels into regions is ours, because neither library reports where the drift is.
import { mkdirSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import pixelmatch from 'pixelmatch';
import sharp from 'sharp';

const HERE = dirname(fileURLToPath(import.meta.url));

// `crop` ({left, top, width, height}) cuts a region first, e.g. the window without the wallpaper.
const rgba = (input, crop, size) => {
  const img = (crop ? sharp(input).extract(crop) : sharp(input)).ensureAlpha();
  return (size ? img.resize(size.width, size.height, { fit: 'fill', kernel: 'lanczos3' }) : img)
    .raw()
    .toBuffer({ resolveWithObject: true });
};

// Groups differing pixels into regions: cells of `cell`×`cell` px that hold a differing pixel,
// joined when they touch (8-neighbours), so one changed label is one region and not a hundred
// glyph fragments. Each box is the exact extent of the differing pixels inside its region.
export function regions(mask, width, height, cell = 8) {
  const cols = Math.ceil(width / cell);
  const rows = Math.ceil(height / cell);
  const box = new Array(cols * rows).fill(null);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      if (!mask[y * width + x]) continue;
      const i = Math.floor(y / cell) * cols + Math.floor(x / cell);
      const b = (box[i] ??= { x0: x, y0: y, x1: x, y1: y, pixels: 0 });
      b.x0 = Math.min(b.x0, x); b.y0 = Math.min(b.y0, y);
      b.x1 = Math.max(b.x1, x); b.y1 = Math.max(b.y1, y);
      b.pixels++;
    }
  }
  const seen = new Uint8Array(cols * rows);
  const out = [];
  for (let start = 0; start < box.length; start++) {
    if (!box[start] || seen[start]) continue;
    const r = { ...box[start], pixels: 0 };
    const stack = [start];
    seen[start] = 1;
    while (stack.length) {
      const i = stack.pop();
      const b = box[i];
      r.x0 = Math.min(r.x0, b.x0); r.y0 = Math.min(r.y0, b.y0);
      r.x1 = Math.max(r.x1, b.x1); r.y1 = Math.max(r.y1, b.y1);
      r.pixels += b.pixels;
      const cx = i % cols, cy = (i - cx) / cols;
      for (let dy = -1; dy <= 1; dy++) {
        for (let dx = -1; dx <= 1; dx++) {
          const nx = cx + dx, ny = cy + dy, n = ny * cols + nx;
          if (nx < 0 || ny < 0 || nx >= cols || ny >= rows || seen[n] || !box[n]) continue;
          seen[n] = 1;
          stack.push(n);
        }
      }
    }
    out.push({ x: r.x0, y: r.y0, width: r.x1 - r.x0 + 1, height: r.y1 - r.y0 + 1, pixels: r.pixels });
  }
  return out.sort((a, b) => b.width * b.height - a.width * a.height || b.pixels - a.pixels);
}

// Compares `snapshot` with `frame` at the snapshot's size (or the frame's, `scaleTo: 'frame'`),
// each cut to its crop first, writes the pixelmatch diff image to `out` and returns the share and
// the largest regions.
export async function diffImages(snapshot, frame, {
  out, scaleTo = 'snapshot', snapshotCrop, frameCrop, threshold = 0.1, cell = 8, top = 5,
} = {}) {
  const [snapMeta, frameMeta] = await Promise.all([snapshot, frame].map(async (input, i) => {
    const crop = i ? frameCrop : snapshotCrop;
    return crop ?? (await sharp(input).metadata());
  }));
  const { width, height } = scaleTo === 'frame' ? frameMeta : snapMeta;
  const [a, b] = await Promise.all([
    rgba(snapshot, snapshotCrop, scaleTo === 'frame' ? { width, height } : null),
    rgba(frame, frameCrop, scaleTo === 'frame' ? null : { width, height }),
  ]);
  const diff = Buffer.alloc(width * height * 4);
  const differing = pixelmatch(a.data, b.data, diff, width, height, { threshold });
  const mask = Buffer.alloc(width * height * 4);
  pixelmatch(a.data, b.data, mask, width, height, { threshold, diffMask: true });
  const hit = new Uint8Array(width * height);
  for (let i = 0; i < hit.length; i++) hit[i] = mask[i * 4 + 3] ? 1 : 0;
  if (out) {
    mkdirSync(dirname(out), { recursive: true });
    await sharp(diff, { raw: { width, height, channels: 4 } }).png().toFile(out);
  }
  const aspect = (m) => m.width / m.height;
  return {
    width, height, out,
    snapshot: { width: snapMeta.width, height: snapMeta.height },
    frame: { width: frameMeta.width, height: frameMeta.height },
    aspectDrift: Math.abs(aspect(snapMeta) / aspect(frameMeta) - 1),
    differing,
    share: differing / (width * height),
    regions: regions(hit, width, height, cell).slice(0, top),
  };
}

export function report(r) {
  const pct = (v) => `${(v * 100).toFixed(2)}%`;
  const lines = [
    `compared at ${r.width}×${r.height} (snapshot ${r.snapshot.width}×${r.snapshot.height}, frame ${r.frame.width}×${r.frame.height})`,
  ];
  if (r.aspectDrift > 0.01) lines.push(`warning: aspect ratios differ by ${pct(r.aspectDrift)}; scaling stretches one image`);
  lines.push(`differing: ${pct(r.share)} (${r.differing} of ${r.width * r.height} px)`);
  lines.push(r.regions.length ? 'largest regions (x,y width×height, differing px):' : 'no differing regions');
  r.regions.forEach((g, i) => lines.push(`  ${i + 1}. ${g.x},${g.y} ${g.width}×${g.height}  ${g.pixels} px`));
  if (r.out) lines.push(`diff image: ${r.out}`);
  return lines.join('\n');
}

const USAGE = 'usage: npm run diff -- <snapshot.png> <frame.png> [--out diff.png] [--scale-to snapshot|frame]'
  + ' [--crop-snapshot x,y,w,h] [--crop-frame x,y,w,h] [--threshold 0.1] [--cell 8] [--top 5] [--json]';

const parseCrop = (v) => {
  if (v === undefined) return undefined;
  const n = v.split(',').map(Number);
  if (n.length !== 4 || !n.every(Number.isInteger) || n[0] < 0 || n[1] < 0 || n[2] <= 0 || n[3] <= 0) {
    throw new Error(`bad crop "${v}": want x,y,w,h in whole pixels\n${USAGE}`);
  }
  return { left: n[0], top: n[1], width: n[2], height: n[3] };
};

async function main() {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      out: { type: 'string' },
      'scale-to': { type: 'string', default: 'snapshot' },
      'crop-snapshot': { type: 'string' },
      'crop-frame': { type: 'string' },
      threshold: { type: 'string', default: '0.1' },
      cell: { type: 'string', default: '8' },
      top: { type: 'string', default: '5' },
      json: { type: 'boolean', default: false },
    },
  });
  const threshold = Number(values.threshold);
  const cell = Number.parseInt(values.cell, 10);
  const top = Number.parseInt(values.top, 10);
  if (positionals.length !== 2 || !['snapshot', 'frame'].includes(values['scale-to'])
    || !(threshold >= 0 && threshold <= 1) || !(cell > 0) || !(top > 0)) {
    throw new Error(USAGE);
  }
  // `npm run` moves to this package; paths stay relative to where the command was typed.
  const cwd = process.env.INIT_CWD ?? process.cwd();
  const [snapshot, frame] = positionals.map((p) => resolve(cwd, p));
  const out = values.out ? resolve(cwd, values.out) : join(HERE, 'out', `${basename(snapshot, '.png')}.diff.png`);
  const r = await diffImages(snapshot, frame, {
    out, scaleTo: values['scale-to'], threshold, cell, top,
    snapshotCrop: parseCrop(values['crop-snapshot']), frameCrop: parseCrop(values['crop-frame']),
  });
  console.log(values.json ? JSON.stringify(r, null, 2) : report(r));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((e) => {
    console.error(e.message);
    process.exitCode = 1;
  });
}
