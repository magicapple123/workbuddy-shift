#!/usr/bin/env python3
"""完整打印会话正文最后 2 条记录（只读）。"""
import glob
import json
import os
import sys

HOME = os.path.expanduser("~")
PROJECTS = os.path.join(HOME, ".workbuddy", "projects")


def main():
    prefix = sys.argv[1] if len(sys.argv) > 1 else ""
    paths = glob.glob(os.path.join(PROJECTS, "*", prefix + "*.jsonl"))
    if not paths:
        print("未找到")
        return 1
    lines = [ln for ln in open(paths[0], encoding="utf-8", errors="ignore").read().splitlines() if ln.strip()]
    for ln in lines[-2:]:
        try:
            print(json.dumps(json.loads(ln), ensure_ascii=False, indent=2)[:1800])
        except json.JSONDecodeError:
            print(ln[:600])
        print("-" * 60)
    return 0


if __name__ == "__main__":
    sys.exit(main())
