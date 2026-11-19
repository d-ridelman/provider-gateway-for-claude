"""Draws the app icon: a cream bridge between two nodes on a warm clay tile.

Warm palette in the spirit of Claude's, but its own mark (a bridge, not a starburst),
so it is not mistaken for the Claude logo.

Usage: python tools/make_icon.py <out-dir>
Writes icon.ico (16-256 px) and icon.png (512 px) into <out-dir>.
"""
import sys
from pathlib import Path

from PIL import Image, ImageDraw

SIZE = 1024
TOP = (222, 128, 92)     # light clay
BOTTOM = (176, 82, 56)   # deep terracotta
WHITE = (250, 244, 235, 255)  # warm cream
NODE_CORE = (186, 90, 62, 255)


def tile() -> Image.Image:
    gradient = Image.new("RGBA", (SIZE, SIZE))
    draw = ImageDraw.Draw(gradient)
    for y in range(SIZE):
        t = y / (SIZE - 1)
        color = tuple(round(a + (b - a) * t) for a, b in zip(TOP, BOTTOM))
        draw.line([(0, y), (SIZE, y)], fill=color + (255,))
    mask = Image.new("L", (SIZE, SIZE), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, SIZE - 1, SIZE - 1], radius=230, fill=255)
    out = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    out.paste(gradient, (0, 0), mask)
    return out


def glyph(img: Image.Image) -> None:
    draw = ImageDraw.Draw(img)
    # Centred vertically: the arch top (base - half) and the node bottoms balance around 512.
    left, right, base = (300, 600), (724, 600), 600
    width = 64
    # Arch: the gateway between two endpoints.
    half = (right[0] - left[0]) // 2
    draw.arc([left[0], base - half, right[0], base + half], start=180, end=360, fill=WHITE, width=width)
    # Deck under the arch.
    draw.rounded_rectangle([left[0] - 10, base - width // 2, right[0] + 10, base + width // 2],
                           radius=width // 2, fill=WHITE)
    # Two nodes (client and provider).
    for cx, cy in (left, right):
        r = 92
        draw.ellipse([cx - r, cy - r, cx + r, cy + r], fill=WHITE)
        inner = 44
        draw.ellipse([cx - inner, cy - inner, cx + inner, cy + inner], fill=NODE_CORE)


def main() -> None:
    out_dir = Path(sys.argv[1])
    out_dir.mkdir(parents=True, exist_ok=True)
    img = tile()
    glyph(img)
    img.resize((512, 512), Image.LANCZOS).save(out_dir / "icon.png")
    sizes = [(s, s) for s in (16, 20, 24, 32, 40, 48, 64, 128, 256)]
    img.resize((256, 256), Image.LANCZOS).save(out_dir / "icon.ico", sizes=sizes)
    print("written", out_dir)


if __name__ == "__main__":
    main()
