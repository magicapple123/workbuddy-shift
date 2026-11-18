#!/usr/bin/env python3
"""从 app.asar 中提取单个文件并打印含指定关键词的行（只读）。"""
import json
import struct
import sys

PATH = r"D:\WorkBuddy\resources\app.asar"


def main():
    target = sys.argv[1] if len(sys.argv) > 1 else "main/menu-i18n.js"
    keywords = sys.argv[2:] or ["反馈", "帮助", "关于"]

    with open(PATH, "rb") as fh:
        head = fh.read(16)
        json_len = struct.unpack("<I", head[12:16])[0]
        header_size = struct.unpack("<I", head[4:8])[0]
        tree = json.loads(fh.read(json_len).decode("utf-8", "ignore"))
        data_start = 8 + header_size

        node = tree
        for part in target.split("/"):
            node = node.get("files", {}).get(part, {})
            if not node:
                print(f"未找到 {target}")
                return 1

        offset = int(node["offset"])
        size = node["size"]
        fh.seek(data_start + offset)
        text = fh.read(size).decode("utf-8", "ignore")

    print(f"{target}: {size} 字节")
    shown = 0
    for line in text.splitlines():
        if any(k in line for k in keywords):
            print("  ", line.strip()[:160])
            shown += 1
            if shown >= 12:
                break
    if not shown:
        print("   未命中关键词")
    return 0


if __name__ == "__main__":
    sys.exit(main())
