#!/usr/bin/env python3
"""只读：把两张图标统一缩放后并排拼成一张对比图，并打印各自的内容占比。

为什么需要：肉眼判断「图标看着小」很容易被圆角和底色骗到，真正的差别是
**不透明内容的包围盒占比**。这个脚本给出数字，同时产出一张可直接看的对比图。

用法：
    python scripts/compare-icons.py <左图> <右图> [--size 128]
输出：
    target/icon-compare.png
"""

import importlib.util
import struct
import sys
import zlib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "target/icon-compare.png"


def load_tray_helpers():
    spec = importlib.util.spec_from_file_location("wb_gen", REPO / "scripts/gen-tray-icon.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.decode_png, module.downscale_premultiplied


def bbox_ratio(size, px):
    minx, miny, maxx, maxy = size, size, -1, -1
    for y in range(size):
        for x in range(size):
            if px[(y * size + x) * 4 + 3] > 16:
                minx, miny = min(minx, x), min(miny, y)
                maxx, maxy = max(maxx, x), max(maxy, y)
    if maxx < 0:
        return 0.0, 0.0
    return (maxx - minx + 1) / size, (maxy - miny + 1) / size


def encode(size_w, size_h, rgb):
    raw = bytearray()
    for y in range(size_h):
        raw.append(0)
        raw += rgb[y * size_w * 3:(y + 1) * size_w * 3]

    def chunk(tag, payload):
        body = tag + payload
        return struct.pack(">I", len(payload)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size_w, size_h, 8, 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def main() -> None:
    left, right = Path(sys.argv[1]), Path(sys.argv[2])
    size = 128
    if "--size" in sys.argv:
        size = int(sys.argv[sys.argv.index("--size") + 1])

    decode_png, downscale = load_tray_helpers()
    tiles = []
    for path in (left, right):
        w, h, px = decode_png(path)
        small = downscale(w, h, px, size)
        rw, rh = bbox_ratio(size, small)
        print(f"{path.name}: 内容占宽 {rw:.0%} 占高 {rh:.0%}（{size}×{size} 归一化后）")
        tiles.append(small)

    pad, gap = 16, 24
    canvas_w = pad * 2 + size * 2 + gap
    canvas_h = pad * 2 + size
    rgb = bytearray()
    for y in range(canvas_h):
        for x in range(canvas_w):
            r = g = b = 240
            for index, tile in enumerate(tiles):
                ox = pad + index * (size + gap)
                if ox <= x < ox + size and pad <= y < pad + size:
                    i = ((y - pad) * size + (x - ox)) * 4
                    a = tile[i + 3] / 255.0
                    r = int(tile[i] * a + r * (1 - a))
                    g = int(tile[i + 1] * a + g * (1 - a))
                    b = int(tile[i + 2] * a + b * (1 - a))
                    break
            rgb += bytes((r, g, b))

    OUT.parent.mkdir(exist_ok=True)
    OUT.write_bytes(encode(canvas_w, canvas_h, rgb))
    print(f"对比图：{OUT.relative_to(REPO)}（左 {left.name} / 右 {right.name}）")


if __name__ == "__main__":
    main()
