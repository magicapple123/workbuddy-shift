#!/usr/bin/env python3
"""Bake macOS-like rounded corners into Windows icon assets.

macOS applies a squircle mask to square app icons. Windows shows the bitmap as-is,
so icon.ico needs transparent corners.

Does not touch icon.png / icon.icns / 32x32.png / 128x128.png (macOS + Linux).
"""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1] / "src-tauri" / "icons"
RADIUS_RATIO = 0.2237


def rounded(im: Image.Image, radius_ratio: float = RADIUS_RATIO) -> Image.Image:
    im = im.convert("RGBA")
    w, h = im.size
    scale = 4
    big = im.resize((w * scale, h * scale), Image.Resampling.LANCZOS)
    bw, bh = big.size
    radius = int(min(bw, bh) * radius_ratio)
    mask = Image.new("L", (bw, bh), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, bw - 1, bh - 1), radius=radius, fill=255)
    mask = mask.resize((w, h), Image.Resampling.LANCZOS)
    out = im.copy()
    out.putalpha(mask)
    return out


def main() -> None:
    src = Image.open(ROOT / "icon.png").convert("RGBA")
    rounded_src = rounded(src)
    rounded_src.save(
        ROOT / "icon.ico",
        format="ICO",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )


if __name__ == "__main__":
    main()
