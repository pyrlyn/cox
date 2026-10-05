// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Checks that the pixel diff finds a known drift where it is and scales a 2x frame to a 1x snapshot.
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import sharp from 'sharp';
import { diffImages, regions } from './pixel-diff.mjs';

const png = (width, height, paint = []) => {
  const data = Buffer.alloc(width * height * 4);
  for (let i = 0; i < data.length; i += 4) data.set([240, 240, 240, 255], i);
  for (const { x, y, w, h } of paint) {
    for (let yy = y; yy < y + h; yy++) {
      for (let xx = x; xx < x + w; xx++) data.set([20, 60, 200, 255], (yy * width + xx) * 4);
    }
  }
  return sharp(data, { raw: { width, height, channels: 4 } }).png().toBuffer();
};

test('two_images_that_differ_in_one_rectangle_report_exactly_that_rectangle', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'pixel-diff-'));
  try {
    const out = join(dir, 'diff.png');
    const r = await diffImages(await png(64, 48), await png(64, 48, [{ x: 13, y: 7, w: 21, h: 9 }]), { out });
    assert.equal(r.differing, 21 * 9);
    assert.equal(r.share, (21 * 9) / (64 * 48));
    assert.deepEqual(r.regions, [{ x: 13, y: 7, width: 21, height: 9, pixels: 21 * 9 }]);
    assert.ok(statSync(out).size > 0);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a_2x_frame_is_scaled_down_to_the_1x_snapshot_before_comparing', async () => {
  const rect = { x: 8, y: 8, w: 16, h: 8 };
  const double = { x: 16, y: 16, w: 32, h: 16 };
  const r = await diffImages(await png(64, 48, [rect]), await png(128, 96, [double]));
  assert.equal(r.width, 64);
  assert.equal(r.differing, 0);
  assert.equal(r.aspectDrift, 0);
});

test('separate_drifts_are_separate_regions_the_largest_first', () => {
  const width = 100, height = 60;
  const mask = new Uint8Array(width * height);
  const fill = (x, y, w, h) => {
    for (let yy = y; yy < y + h; yy++) for (let xx = x; xx < x + w; xx++) mask[yy * width + xx] = 1;
  };
  fill(2, 2, 5, 5);
  fill(60, 30, 30, 20);
  assert.deepEqual(regions(mask, width, height, 8), [
    { x: 60, y: 30, width: 30, height: 20, pixels: 600 },
    { x: 2, y: 2, width: 5, height: 5, pixels: 25 },
  ]);
});

test('glyph_sized_gaps_inside_one_changed_label_merge_into_one_region', () => {
  const width = 64, height = 16;
  const mask = new Uint8Array(width * height);
  for (const x of [4, 10, 16, 22]) for (let y = 4; y < 10; y++) mask[y * width + x] = 1;
  assert.deepEqual(regions(mask, width, height, 8), [{ x: 4, y: 4, width: 19, height: 6, pixels: 24 }]);
});

test('crops_cut_the_same_window_out_of_both_images_before_comparing', async () => {
  const snapshot = await png(40, 30, [{ x: 10, y: 10, w: 20, h: 10 }]);
  const frame = await png(100, 80, [{ x: 20, y: 20, w: 40, h: 20 }]);
  const r = await diffImages(snapshot, frame, {
    snapshotCrop: { left: 5, top: 5, width: 30, height: 20 },
    frameCrop: { left: 10, top: 10, width: 60, height: 40 },
  });
  assert.equal(r.width, 30);
  assert.equal(r.differing, 0);
});
