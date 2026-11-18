#!/usr/bin/env python3
"""解析 app.asar 头部，列出文件名里带 menu/locale/i18n 的条目（只读）。"""
import json
import struct
import sys

PATH = r"D:\WorkBuddy\resources\app.asar"


def main():
    with open(PATH, "rb") as fh:
        head = fh.read(16)
        if len(head) < 16:
            print("文件过短")
            return 1
        # asar: 4 字节 pickle 头、4 字节 header size、4 字节 payload size、4 字节 json 长度
        json_len = struct.unpack("<I", head[12:16])[0]
        raw = fh.read(json_len)
    try:
        tree = json.loads(raw.decode("utf-8", "ignore"))
    except json.JSONDecodeError as exc:
        print("header 解析失败:", exc)
        return 1

    hits = []

    def walk(node, prefix=""):
        for name, value in node.get("files", {}).items():
            p = f"{prefix}/{name}" if prefix else name
            if "files" in value:
                walk(value, p)
            else:
                low = p.lower()
                if any(k in low for k in ("menu", "locale", "i18n", "zh-cn", "zh_cn")):
                    hits.append(p)

    walk(tree)
    print(f"命中 {len(hits)} 个条目，前 20：")
    for h in hits[:20]:
        print("  ", h)
    return 0


if __name__ == "__main__":
    sys.exit(main())
