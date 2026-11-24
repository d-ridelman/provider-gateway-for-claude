"""Draws the app icon in the family of the author's brand book:
an iron tile as in the brand book's app icon, and on it the product's mark in its static
colour - PGC's bridge between two nodes (client and provider), the mark of the previous icon, laid on the 64 grid.

The static colour is fjord #7fa8e8: not the brand's ember, not the author's bronze, not red or
amber, and not used inside the window (there the mark takes the accent).

Every ICO frame is drawn at its own size (4x supersampled), not shrunk from one big image:
40 px and up get the full icon (the mark in its frame, scaled 0.75 with an offset of 8, as the
brand's app icon); 32 px and below get a bolder mark without the frame and a hairline rim, so the
tile does not vanish on a dark taskbar.

Usage: python tools/make_icon.py <out-dir>
Writes icon.ico (16-256 px), icon.png (512 px) and tray.png (32 px, the tray's own small icon:
Tauri would otherwise hand the tray the 256 px frame to shrink).
"""
import sys
from pathlib import Path

from PIL import Image, ImageDraw

SS = 4                       # supersampling
MARK = (127, 168, 232)       # fjord: PGC's static mark colour
TOP = (28, 35, 44)           # #1c232c
BOTTOM = (11, 13, 16)        # #0b0d10, iron.bg
TILE_RADIUS = 14 / 64

# The mark: PGC's own bridge from the previous icon, the same proportions moved to the 64 grid
# (1024 / 16): an arch over a deck between two ring nodes, client and provider.
NODES = ((18.75, 37.5), (45.25, 37.5))
NODE_R, CORE_R = 5.75, 2.75
ARCH_RADIUS, STROKE = 13.25, 4.0


def tile(px: int, rim: bool) -> Image.Image:
    """The iron tile, rounded like the brand's app icon, with its faint top-down gradient."""
    grad = Image.new("RGBA", (px, px))
    draw = ImageDraw.Draw(grad)
    for y in range(px):
        t = y / max(px - 1, 1)
        draw.line([(0, y), (px, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(TOP, BOTTOM)) + (255,))
    mask = Image.new("L", (px, px), 0)
    radius = round(px * TILE_RADIUS)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, px - 1, px - 1], radius=radius, fill=255)
    out = Image.new("RGBA", (px, px), (0, 0, 0, 0))
    out.paste(grad, (0, 0), mask)
    if rim:
        ImageDraw.Draw(out).rounded_rectangle([0, 0, px - 1, px - 1], radius=radius, outline=MARK + (150,), width=SS)
    return out


def mark(draw: ImageDraw.ImageDraw, scale: float, ox: float, oy: float, min_stroke: float = 0, cores: bool = True) -> None:
    """The mark: grid units -> pixels through scale and offset; strokes never thinner than min_stroke (px).
    Without cores (the smallest sizes) the nodes are solid: a 1 px hole only blurs."""
    p = lambda x, y: (ox + x * scale, oy + y * scale)
    fill = MARK + (255,)
    w = max(STROKE * scale, min_stroke)
    (lx, y), (rx, _) = p(*NODES[0]), p(*NODES[1])
    r = ARCH_RADIUS * scale
    cx = (lx + rx) / 2
    draw.arc([cx - r - w / 2, y - r - w / 2, cx + r + w / 2, y + r + w / 2], 180, 360, fill=fill, width=round(w))
    draw.rounded_rectangle([lx - 10 / 16 * scale, y - w / 2, rx + 10 / 16 * scale, y + w / 2], radius=w / 2, fill=fill)
    node = max(NODE_R * scale, w * 1.3)
    for x in (lx, rx):
        draw.ellipse([x - node, y - node, x + node, y + node], fill=fill)
        if cores:
            core = CORE_R * scale
            draw.ellipse([x - core, y - core, x + core, y + core], fill=BOTTOM + (255,))


def frame(draw: ImageDraw.ImageDraw, scale: float, ox: float, oy: float) -> None:
    """The mark's frame (as the brand book's tile): a rounded ring from 3 to 61 on the grid, 2.6 thick."""
    a, b, t = ox + 3 * scale, ox + 61 * scale, 2.6 * scale
    draw.rounded_rectangle([a, oy + 3 * scale, b, oy + 61 * scale], radius=5 * scale, outline=MARK + (255,), width=round(t))


def render(size: int) -> Image.Image:
    px = size * SS
    small = size <= 32
    img = tile(px, rim=small)
    draw = ImageDraw.Draw(img)
    if small:
        # Glyph box 13..51 x 22..43.25 on the grid fills 80 % of the tile, centred.
        scale = 0.80 * px / 38
        mark(draw, scale, px / 2 - 32 * scale, px / 2 - 32.6 * scale, min_stroke=1.5 * SS, cores=size >= 24)
    else:
        # The brand's app icon: the framed mark at 0.75 with an offset of 8 (on a 64 tile).
        unit = px / 64
        scale, off = 0.75 * unit, 8 * unit
        frame(draw, scale, off, off)
        mark(draw, scale, off, off)
    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    out_dir = Path(sys.argv[1])
    out_dir.mkdir(parents=True, exist_ok=True)
    render(512).save(out_dir / "icon.png")
    render(32).save(out_dir / "tray.png")
    sizes = (256, 128, 64, 48, 40, 32, 24, 20, 16)
    frames = [render(s) for s in sizes]
    frames[0].save(out_dir / "icon.ico", sizes=[(s, s) for s in sizes], append_images=frames[1:])
    # Self-check: every size is in the ICO and is its own drawing (not the 256 frame shrunk).
    with Image.open(out_dir / "icon.ico") as ico:
        got = sorted(ico.info["sizes"])
        assert got == sorted((s, s) for s in sizes), got
        ico.size = (16, 16)
        assert ico.copy().convert("RGBA").tobytes() == frames[-1].tobytes(), "16 px frame is not the drawn one"
    print("written", out_dir)


if __name__ == "__main__":
    main()
