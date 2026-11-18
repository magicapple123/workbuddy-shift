#!/usr/bin/env python3
"""检查指定会话正文是否含 _meta 字段，并打印末条原始文本（只读）。"""
import glob
import json
import os
import sys

HOME = os.path.expanduser("~")
PROJECTS = os.path.join(HOME, ".workbuddy", "projects")


def main():
    prefixes = sys.argv[1:]
    for prefix in prefixes:
        paths = glob.glob(os.path.join(PROJECTS, "*", prefix + "*.jsonl"))
        if not paths:
            print(f"{prefix}: 未找到正文")
            continue
        path = paths[0]
        with open(path, encoding="utf-8", errors="ignore") as fh:
            lines = [ln for ln in fh.read().splitlines() if ln.strip()]
        meta = sum(1 for ln in lines if "_meta" in ln)
        print(f"\n{prefix}: {len(lines)} 行，含 _meta {meta} 行")
        print(f"  末条: {lines[-1][:260]}")
        for ln in lines:
            if "_meta" in ln:
                try:
                    rec = json.loads(ln)
                    print(f"  含 _meta 的记录 status={rec.get('status')!r} role={rec.get('role')!r}")
                except json.JSONDecodeError:
                    print("  含 _meta 但 JSON 解析失败")


if __name__ == "__main__":
    main()
