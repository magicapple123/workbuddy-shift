#!/usr/bin/env python3
"""按 id 前缀打印会话正文最后若干条记录（只读）。"""
import glob
import json
import os
import sqlite3
import sys

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")
PROJECTS = os.path.join(HOME, ".workbuddy", "projects")


def main():
    prefixes = sys.argv[1:]
    if not prefixes:
        print("用法: python scripts/diagnostics/show-session-tail.py <id前缀> [...]")
        return 1
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    for prefix in prefixes:
        row = con.execute(
            "SELECT id, title, status FROM sessions WHERE id LIKE ?", (prefix + "%",)
        ).fetchone()
        if not row:
            print(f"\n[{prefix}] 未找到")
            continue
        sid, title, status = row
        path = glob.glob(os.path.join(PROJECTS, "*", sid + ".jsonl"))
        print(f"\n[{sid[:8]}] status={status!r} title={title!r}")
        if not path:
            print("  正文缺失")
            continue
        recs = []
        with open(path[0], encoding="utf-8", errors="ignore") as fh:
            for line in fh:
                if line.strip():
                    try:
                        recs.append(json.loads(line))
                    except json.JSONDecodeError:
                        pass
        print(f"  共 {len(recs)} 条，末尾 5 条：")
        for rec in recs[-5:]:
            keys = {k: rec.get(k) for k in ("type", "role", "status", "subtype") if rec.get(k)}
            text = ""
            content = rec.get("content")
            if isinstance(content, list) and content:
                text = str(content[0].get("text", ""))[:34]
            print(f"    {keys} {text!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
