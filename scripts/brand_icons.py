#!/usr/bin/env python3
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

"""`just brand-icons`: the macOS app's icons, derived from brand/logo/cox-mark.svg.

brand/ is the one source of the mark, so the app never carries a hand-edited copy:
this script writes

- desktop/macos/App/AppIcon.icon: the Icon Composer document Xcode compiles into the
  app icon. The mark's background tile becomes the document's solid fill and the rest
  of the mark its one layer, so macOS draws its own squircle, grid and Liquid Glass
  instead of shrinking a pre-shaped tile onto a grey plate.
- CoxUI's Brand.xcassets/CoxMark.imageset: the mark itself, kept as a vector, for the
  views that show it (`AppIcon`).

`--check` writes nothing and fails when a generated file differs, for CI.
Tests: `python3 -m unittest discover -s scripts -p 'test_brand_icons.py'`.
"""

from __future__ import annotations

import json
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

SVG_NS = "http://www.w3.org/2000/svg"
MARK = Path("brand/logo/cox-mark.svg")
ICON = Path("desktop/macos/App/AppIcon.icon")
GLYPH = "cox-glyph.svg"
CANVAS = 1024
IMAGESET = Path("desktop/macos/Packages/CoxUI/Sources/CoxUI/Brand.xcassets/CoxMark.imageset")


class BrandError(Exception):
    pass


def split_mark(svg: str) -> tuple[str, str]:
    """The mark without its background tile, and the tile's fill colour."""
    ET.register_namespace("", SVG_NS)
    root = ET.fromstring(svg)
    tile = root.find(f"{{{SVG_NS}}}rect")
    box = root.get("viewBox", "").split()
    # The tile is the first rect and covers the whole viewBox; anything else means the
    # mark changed shape and the split below would drop artwork.
    if tile is None or len(box) != 4 or [tile.get("width"), tile.get("height")] != box[2:]:
        raise BrandError(f"{MARK}: the first <rect> is not a full-size background tile")
    fill = tile.get("fill", "")
    if len(fill) != 7 or not fill.startswith("#"):
        raise BrandError(f"{MARK}: the background tile's fill {fill!r} is not #RRGGBB")
    root.remove(tile)
    # Icon Composer sizes a layer by its width and height in points, not its viewBox;
    # without them the 64-unit mark lands as a speck in the 1024-point canvas.
    root.set("width", str(CANVAS))
    root.set("height", str(CANVAS))
    return ET.tostring(root, encoding="unicode") + "\n", fill


def srgb(hex_colour: str) -> str:
    r, g, b = (int(hex_colour[i : i + 2], 16) / 255 for i in (1, 3, 5))
    return f"srgb:{r:.5f},{g:.5f},{b:.5f},1.00000"


def xcode_json(value: object) -> str:
    # Xcode's own spacing, so opening the catalog in Xcode leaves no diff.
    return json.dumps(value, indent=2, separators=(",", " : ")) + "\n"


def outputs(mark: str) -> dict[Path, str]:
    glyph, fill = split_mark(mark)
    icon = {
        "fill": {"solid": srgb(fill)},
        "groups": [{"layers": [{"image-name": GLYPH, "name": "cox-glyph"}]}],
        "supported-platforms": {"squares": ["macOS"]},
    }
    imageset = {
        "images": [{"filename": MARK.name, "idiom": "universal"}],
        "info": {"author": "xcode", "version": 1},
        "properties": {"preserves-vector-representation": True},
    }
    catalog = {"info": {"author": "xcode", "version": 1}}
    return {
        ICON / "icon.json": xcode_json(icon),
        ICON / "Assets" / GLYPH: glyph,
        IMAGESET.parent / "Contents.json": xcode_json(catalog),
        IMAGESET / "Contents.json": xcode_json(imageset),
        IMAGESET / MARK.name: mark,
    }


def main(argv: list[str]) -> int:
    root = Path(__file__).resolve().parent.parent
    check = argv == ["--check"]
    if argv and not check:
        print("usage: brand_icons.py [--check]", file=sys.stderr)
        return 2
    try:
        files = outputs((root / MARK).read_text(encoding="utf-8"))
    except (OSError, ET.ParseError, BrandError) as err:
        print(f"brand_icons: {err}", file=sys.stderr)
        return 1
    stale = []
    for rel, text in files.items():
        path = root / rel
        current = path.read_text(encoding="utf-8") if path.is_file() else None
        if current == text:
            continue
        stale.append(rel)
        if not check:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
    if check and stale:
        for rel in stale:
            print(f"brand_icons: {rel} is out of date; run `just brand-icons`", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
