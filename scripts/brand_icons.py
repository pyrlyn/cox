#!/usr/bin/env python3
# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

"""`just brand-icons`: every icon and logo raster, derived from the SVGs in brand/logo/.

brand/logo/*.svg is the one source of the mark, so nothing carries a hand-edited or
hand-exported copy: this script writes

- brand/logo/png/*.png: the PNG exports, rendered by resvg (the mise.toml pin) at the
  sizes in `PNGS`.
- desktop/macos/App/AppIcon.icon: the Icon Composer document Xcode compiles into the
  app icon. The mark's background tile becomes the document's solid fill and the rest
  of the mark its one layer, so macOS draws its own squircle, grid and Liquid Glass
  instead of shrinking a pre-shaped tile onto a grey plate.
- CoxUI's Brand.xcassets/CoxMark.imageset: the mark itself, kept as a vector, for the
  views that show it (`AppIcon`).

`--check` writes nothing and fails when a generated file differs, for CI. The PNG bytes
are only stable for one resvg version on one architecture, so CI checks on the same
Apple Silicon runners the app is built on.
Tests: `python3 -m unittest discover -s scripts -p 'test_brand_icons.py'`.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
from collections.abc import Callable
from pathlib import Path

SVG_NS = "http://www.w3.org/2000/svg"
LOGO = Path("brand/logo")
MARK = LOGO / "cox-mark.svg"
FAVICON = LOGO / "cox-favicon.svg"
ICON = Path("desktop/macos/App/AppIcon.icon")
GLYPH = "cox-glyph.svg"
CANVAS = 1024
IMAGESET = Path("desktop/macos/Packages/CoxUI/Sources/CoxUI/Brand.xcassets/CoxMark.imageset")

# Output name -> (source SVG, square size in pixels). The favicon SVG is drawn for small
# sizes (thicker strokes on a 32-unit grid); everything larger uses the mark.
PNGS: dict[str, tuple[Path, int]] = {
    "cox-favicon-32x32.png": (FAVICON, 32),
    "cox-favicon-64x64.png": (FAVICON, 64),
    "cox-icon-favicon-256.png": (FAVICON, 256),
    "cox-apple-touch-icon-180.png": (MARK, 180),
    "cox-icon-logo-512.png": (MARK, 512),
    "cox-icon-logo-1024.png": (MARK, 1024),
}

Renderer = Callable[[Path, int, Path], None]


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
    """The text files derived from the mark, by repository path."""
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


def resvg(svg: Path, size: int, out: Path) -> None:
    exe = shutil.which("resvg")
    if exe is None:
        raise BrandError("resvg not found; run through `just brand-icons` (mise exec)")
    # Fonts are not loaded: no mark or favicon uses text, so a renderer that found a
    # system font would only make the output depend on the machine.
    cmd = [exe, "--skip-system-fonts", "-w", str(size), "-h", str(size), str(svg), str(out)]
    done = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if done.returncode != 0:
        raise BrandError(f"resvg {svg}: {done.stderr.strip()}")


def rasters(root: Path, render: Renderer, scratch: Path) -> dict[Path, bytes]:
    """Every PNG in `PNGS`, rendered into `scratch`, by repository path."""
    files = {}
    for name, (svg, size) in PNGS.items():
        out = scratch / name
        render(root / svg, size, out)
        files[LOGO / "png" / name] = out.read_bytes()
    return files


def sync(root: Path, files: dict[Path, bytes], check: bool) -> list[Path]:
    """Write every file that differs (unless `check`); return the ones that did."""
    stale = []
    for rel, data in files.items():
        path = root / rel
        if path.is_file() and path.read_bytes() == data:
            continue
        stale.append(rel)
        if not check:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
    return stale


def main(argv: list[str], render: Renderer = resvg) -> int:
    root = Path(__file__).resolve().parent.parent
    check = argv == ["--check"]
    if argv and not check:
        print("usage: brand_icons.py [--check]", file=sys.stderr)
        return 2
    try:
        texts = outputs((root / MARK).read_text(encoding="utf-8"))
        files = {rel: text.encode("utf-8") for rel, text in texts.items()}
        with tempfile.TemporaryDirectory() as scratch:
            files |= rasters(root, render, Path(scratch))
    except (OSError, ET.ParseError, BrandError) as err:
        print(f"brand_icons: {err}", file=sys.stderr)
        return 1
    stale = sync(root, files, check)
    if check and stale:
        for rel in stale:
            print(f"brand_icons: {rel} is out of date; run `just brand-icons`", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
