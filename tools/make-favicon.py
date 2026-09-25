#!/usr/bin/env python3
"""Generate the pico-numpad favicon set.

The mark is the device itself: a grid of keycaps with the bottom-right key
(`+`, the one you long-press for the host-slot menu) picked out in the site's
accent colour.

The grid is art-directed per size, because a 4x4 grid of 3px keys with 1px
gaps aliases into a checkerboard at 16px -- sixteen separate elements is more
than a 16px tile can carry. 3x3 survives it. So:

    16px      3x3   (legible; nothing else tested was)
    32px+     4x4   (legible, and true to the actual 16-key pad)

Renders:
  web/favicon.svg                    scalable master, 4x4
  web/favicon-16.png                 3x3
  web/favicon-32.png
  web/favicon-48.png
  web/apple-touch-icon.png   180px, inset for the iOS mask safe zone
  web/icon-192.png
  web/icon-512.png
  web/favicon.ico             16 + 32, for legacy requests

Usage:  python3 tools/make-favicon.py
Requires: rsvg-convert (brew install librsvg) and ImageMagick for the .ico.
"""

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "web"

# Palette, matching the CSS custom properties in web/index.html.
BG = "#0d1013"      # --bg
KEY = "#7e8fa0"     # keycap; lifted well off the background so it survives downscaling
ACCENT = "#4cc2ff"  # --accent

BOX = 64


def grid_svg(px: int, cols: int, key: int, gap: int) -> str:
    """A `cols` x `cols` keycap grid, drawn in a 64-unit box and scaled to `px`.

    The bottom-right key carries the accent -- that is the `+` key on the real
    pad, the one you long-press for the host-slot menu. Index is computed from
    `cols` rather than hardcoded, so it lands bottom-right for 3x3 and 4x4 both.
    """
    accent_cell = cols * cols - 1
    side = cols * key + (cols - 1) * gap
    start = (BOX - side) / 2
    cells = []
    for r in range(cols):
        for c in range(cols):
            i = r * cols + c
            fill = ACCENT if i == accent_cell else KEY
            cells.append(
                f'  <rect x="{start + c * (key + gap):.2f}" y="{start + r * (key + gap):.2f}" '
                f'width="{key}" height="{key}" rx="2" fill="{fill}"/>'
            )
    return "\n".join(
        [
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{px}" height="{px}" '
            f'viewBox="0 0 {BOX} {BOX}">',
            f'  <rect width="{BOX}" height="{BOX}" rx="13" fill="{BG}"/>',
            *cells,
            "</svg>",
        ]
    )


def four_by_four(px: int, pad_fraction: float = 0.0) -> str:
    """The full 16-key mark. `pad_fraction` insets it for maskable icons, where
    the platform crops the outer ~10% on each edge."""
    inset = BOX * pad_fraction
    span = BOX - 2 * inset
    s = span / BOX
    return grid_svg(px, 4, 12 * s, 4 * s)


def three_by_three(px: int, pad_fraction: float = 0.0) -> str:
    inset = BOX * pad_fraction
    span = BOX - 2 * inset
    s = span / BOX
    return grid_svg(px, 3, 16 * s, 4 * s)


def raster(svg_text: str, out: Path, size: int) -> None:
    res = subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size)],
        input=svg_text.encode(),
        capture_output=True,
    )
    if res.returncode != 0:
        sys.exit(f"rsvg-convert failed for {out}: {res.stderr.decode()}")
    out.write_bytes(res.stdout)
    print(f"  {str(out.relative_to(ROOT)):<28} {size:>4}x{size:<4} {len(res.stdout):>7} bytes")


def build_ico(out: Path, sources: list[Path]) -> None:
    for cmd in (["magick"], ["convert"]):
        try:
            res = subprocess.run([*cmd, *[str(s) for s in sources], str(out)], capture_output=True)
        except FileNotFoundError:
            continue
        if res.returncode == 0:
            print(f"  {str(out.relative_to(ROOT)):<28} {'16+32':>9} {out.stat().st_size:>7} bytes")
            return
    sys.exit("ico build failed: neither `magick` nor `convert` succeeded")


def main() -> None:
    WEB.mkdir(parents=True, exist_ok=True)

    # Scalable master. No inset: browsers rendering SVG favicons do not crop.
    svg_master = four_by_four(BOX)
    (WEB / "favicon.svg").write_text(svg_master + "\n")
    print(f"  web/favicon.svg  {'scalable':>9} {(WEB / 'favicon.svg').stat().st_size:>7} bytes")

    raster(three_by_three(16), WEB / "favicon-16.png", 16)
    raster(four_by_four(32), WEB / "favicon-32.png", 32)
    raster(four_by_four(48), WEB / "favicon-48.png", 48)
    # iOS masks the icon and crops the outer edge, so the grid needs a safe zone.
    raster(four_by_four(180, pad_fraction=0.12), WEB / "apple-touch-icon.png", 180)
    raster(four_by_four(192, pad_fraction=0.08), WEB / "icon-192.png", 192)
    raster(four_by_four(512, pad_fraction=0.08), WEB / "icon-512.png", 512)

    build_ico(WEB / "favicon.ico", [WEB / "favicon-16.png", WEB / "favicon-32.png"])


if __name__ == "__main__":
    main()
