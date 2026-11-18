#!/usr/bin/env python3
"""把吉祥物剪影做成托盘模板素材。

链路：
    assets/mascot/mascot-mask.png（1024，透明底黑剪影）
        → src-tauri/icons/tray-icon-template.png（256，本脚本产出，黑色 + alpha）
        → src-tauri/icons/tray-icon-mono-{black,white}.rgba（Windows 托盘，32）
        → src-tauri/icons/tray-icon-template.rgba（macOS 模板，36）
    后两步由 `scripts/gen-tray-icon.py --mono ...` 完成。

为什么保留中间 PNG 而不是直接读 mascot：托盘素材要求「正方形、内容居中、
四周留白固定」，而 mascot 的包围盒是 907×765（宽高比 1.19）。直接整幅下采样
在 32px 下内容只占 28px 宽、24px 高，看起来比原来的方头猫瘦一圈。这里先按
包围盒裁切、再按「宽度占 88%」重新居中，得到的密度与旧素材一致（旧素材 86%）。

用法：
    python scripts/gen-tray-template.py
"""

import importlib.util
import struct
import zlib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SRC = REPO / "assets/mascot/mascot-mask.png"
OUT = REPO / "src-tauri/icons/tray-icon-template.png"
SIZE = 256
# 内容宽度占画布的比例。托盘位只有 16 px，留白会被放大成"没有图标"的观感——
# 官方 WorkBuddy 的托盘图标是满幅方形，我们的剪影如果只占 88% 会明显缩水。
# 0.98 是"贴满但不切边"的上限（再大会切掉抗锯齿边缘）。
WIDTH_RATIO = 0.98
INK = (17, 17, 19)  # 与 gen-tray-icon.py 的 MONO_RGB["black"] 同一墨色


def load_decoder():
    """复用 gen-tray-icon.py 的 PNG 解码，避免两份实现。"""
    path = REPO / "scripts/gen-tray-icon.py"
    spec = importlib.util.spec_from_file_location("wb_gen_tray_icon", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.decode_png


def bbox(w, h, px, threshold=12):
    minx, miny, maxx, maxy = w, h, -1, -1
    for y in range(h):
        for x in range(w):
            if px[(y * w + x) * 4 + 3] > threshold:
                if x < minx:
                    minx = x
                if x > maxx:
                    maxx = x
                if y < miny:
                    miny = y
                if y > maxy:
                    maxy = y
    if maxx < 0:
        raise SystemExit("源图没有可见像素")
    return minx, miny, maxx, maxy


def fit_square(w, px, box):
    """裁到包围盒 → 按目标宽度等比缩放 → 居中贴到正方形画布。"""
    minx, miny, maxx, maxy = box
    bw, bh = maxx - minx + 1, maxy - miny + 1
    target_w = int(SIZE * WIDTH_RATIO)
    scale = target_w / bw
    target_h = max(1, round(bh * scale))
    offx = (SIZE - target_w) // 2
    offy = (SIZE - target_h) // 2

    out = bytearray(SIZE * SIZE * 4)
    for ty in range(target_h):
        sy0 = miny + int(ty / scale)
        sy1 = miny + max(int(ty / scale) + 1, int((ty + 1) / scale))
        for tx in range(target_w):
            sx0 = minx + int(tx / scale)
            sx1 = minx + max(int(tx / scale) + 1, int((tx + 1) / scale))
            total, count = 0, 0
            for sy in range(sy0, min(sy1, miny + bh)):
                row = sy * w * 4
                for sx in range(sx0, min(sx1, minx + bw)):
                    total += px[row + sx * 4 + 3]
                    count += 1
            alpha = total // count if count else 0
            # 源剪影内部残留 254 这类过渡值，会让托盘图标显示为「半透明」。
            # 单色素材只有「有墨/无墨」两种语义，接近实心的一律拉满。
            if alpha >= 250:
                alpha = 255
            o = ((offy + ty) * SIZE + offx + tx) * 4
            out[o], out[o + 1], out[o + 2] = INK
            out[o + 3] = alpha
    return out


def encode_png(px):
    """最小 PNG 编码（RGBA8、非隔行、filter 0）。"""
    raw = bytearray()
    stride = SIZE * 4
    for y in range(SIZE):
        raw.append(0)
        raw += px[y * stride:(y + 1) * stride]

    def chunk(tag, payload):
        body = tag + payload
        return struct.pack(">I", len(payload)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def main() -> None:
    decode_png = load_decoder()
    w, h, px = decode_png(SRC)
    box = bbox(w, h, px)
    fitted = fit_square(w, px, box)
    OUT.write_bytes(encode_png(fitted))
    opaque = sum(1 for i in range(3, len(fitted), 4) if fitted[i] > 200)
    print(f"源 {w}×{h}，包围盒 {box[2] - box[0] + 1}×{box[3] - box[1] + 1}"
          f" → {OUT.relative_to(REPO)}（{SIZE}×{SIZE}，"
          f"内容占宽 {WIDTH_RATIO:.0%}，实心像素 {opaque:,}）")


if __name__ == "__main__":
    main()
