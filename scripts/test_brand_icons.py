# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

"""Tests for `brand_icons`: the app icons derived from brand/logo/cox-mark.svg."""

import json
import unittest

from brand_icons import GLYPH, ICON, IMAGESET, MARK, BrandError, outputs, split_mark

MARK_SVG = (
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">'
    '<rect width="64" height="64" rx="12" fill="#0F1A14"/>'
    '<rect x="40" y="31" width="6" height="6" fill="#A8E06C"/>'
    "</svg>"
)


class SplitMark(unittest.TestCase):
    def test_the_background_tile_becomes_the_fill_and_the_rest_stays(self):
        glyph, fill = split_mark(MARK_SVG)
        self.assertEqual(fill, "#0F1A14")
        self.assertNotIn("#0F1A14", glyph)
        self.assertIn('fill="#A8E06C"', glyph)
        self.assertIn('viewBox="0 0 64 64"', glyph)

    def test_the_glyph_fills_the_icon_canvas(self):
        glyph, _ = split_mark(MARK_SVG)
        self.assertIn('width="1024"', glyph)
        self.assertIn('height="1024"', glyph)

    def test_a_mark_whose_first_rect_is_not_the_full_tile_is_refused(self):
        partial = MARK_SVG.replace('width="64" height="64"', 'width="60" height="64"')
        with self.assertRaises(BrandError):
            split_mark(partial)

    def test_a_tile_fill_that_is_not_a_hex_colour_is_refused(self):
        with self.assertRaises(BrandError):
            split_mark(MARK_SVG.replace('fill="#0F1A14"', 'fill="url(#g)"'))


class Outputs(unittest.TestCase):
    def test_the_icon_fill_is_the_tile_colour_and_its_layer_is_the_glyph(self):
        files = outputs(MARK_SVG)
        icon = json.loads(files[ICON / "icon.json"])
        self.assertEqual(icon["fill"]["solid"], "srgb:0.05882,0.10196,0.07843,1.00000")
        self.assertEqual(icon["groups"][0]["layers"][0]["image-name"], GLYPH)
        self.assertIn(ICON / "Assets" / GLYPH, files)

    def test_the_imageset_carries_the_mark_verbatim_as_a_vector(self):
        files = outputs(MARK_SVG)
        self.assertEqual(files[IMAGESET / MARK.name], MARK_SVG)
        imageset = json.loads(files[IMAGESET / "Contents.json"])
        self.assertTrue(imageset["properties"]["preserves-vector-representation"])
        self.assertEqual(imageset["images"][0]["filename"], MARK.name)


if __name__ == "__main__":
    unittest.main()
