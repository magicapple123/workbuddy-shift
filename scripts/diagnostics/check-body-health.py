#!/usr/bin/env python3
"""检查会话正文的健康度：行尾、非法行、状态取值、必备字段。

只读，不修改任何文件。
"""
import glob
import json
import os
from collections import Counter

HOME = os.path.expanduser("~")
PROJECTS = os.path.join(HOME, ".workbuddy", "projects")
BACKUP = max(glob.glob(os.path.join(HOME, ".workbuddy", "incomplete-backup-*")), key=os.path.getmtime)


def scan_file(path):
    with open(path, "rb") as fh:
        raw = fh.read()
    crlf = raw.count(b"\r\n")
    lf = raw.count(b"\n") - crlf
    text = raw.decode("utf-8", "ignore")
    lines = text.splitlines()
    bad = 0
    for line in lines:
        if not line.strip():
            continue
        try:
            json.loads(line)
        except json.JSONDecodeError:
            bad += 1
    return {"bytes": len(raw), "crlf": crlf, "lf": lf, "lines": len(lines),
            "trailing_nl": raw.endswith(b"\n"), "bad_json": bad}


def main():
    files = glob.glob(os.path.join(PROJECTS, "*", "*.jsonl"))
    print(f"正文文件 {len(files)} 个 | 修复备份 {os.path.basename(BACKUP)}\n")

    print("=== 行尾风格分布 ===")
    endings = Counter()
    for p in files:
        info = scan_file(p)
        key = "CRLF" if info["crlf"] and not info["lf"] else ("LF" if info["lf"] and not info["crlf"] else "混合")
        endings[key] += 1
    print("  ", dict(endings))

    print("\n=== 非法 JSON 行 ===")
    bads = [(p, scan_file(p)["bad_json"]) for p in files]
    bads = [x for x in bads if x[1]]
    print("  有问题的文件:", len(bads))
    for p, n in bads[:5]:
        print(f"   {os.path.basename(p)}: {n} 行")

    print("\n=== 被修复文件 vs 备份：行尾是否变了 ===")
    changed = 0
    for bak in glob.glob(os.path.join(BACKUP, "*.jsonl")):
        cur = glob.glob(os.path.join(PROJECTS, "*", os.path.basename(bak)))
        if not cur:
            continue
        b, c = scan_file(bak), scan_file(cur[0])
        if (b["crlf"], b["lf"]) != (c["crlf"], c["lf"]):
            changed += 1
            print(f"   {os.path.basename(bak)}: 备份 CRLF={b['crlf']} LF={b['lf']} → 现在 CRLF={c['crlf']} LF={c['lf']}")
    print(f"   行尾发生变化的文件数: {changed} / {len(glob.glob(os.path.join(BACKUP, '*.jsonl')))}")

    print("\n=== assistant 消息的 status 取值分布 ===")
    c = Counter()
    for p in files:
        with open(p, encoding="utf-8", errors="ignore") as fh:
            for line in fh:
                if '"assistant"' not in line:
                    continue
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if rec.get("role") == "assistant":
                    c[rec.get("status")] += 1
    print("  ", dict(c))


if __name__ == "__main__":
    main()
