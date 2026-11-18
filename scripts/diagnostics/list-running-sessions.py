#!/usr/bin/env python3
"""列出状态非终态的会话，并对照正文末条状态（只读）。

非终态 = 不在 completed / error / terminated / archived 里。
客户端据行状态渲染「运行中」转圈，与正文无关。
"""
import glob
import json
import os
import sqlite3

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")
PROJECTS = os.path.join(HOME, ".workbuddy", "projects")
SETTLED = {"completed", "error", "terminated", "archived"}


def body_map():
    return {os.path.basename(p)[:-6]: p for p in glob.glob(os.path.join(PROJECTS, "*", "*.jsonl"))}


def tail_status(path):
    if not path:
        return "(无正文)"
    last = None
    with open(path, encoding="utf-8", errors="ignore") as fh:
        for line in fh:
            if '"assistant"' not in line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if rec.get("type") == "message" and rec.get("role") == "assistant":
                last = rec
    if last is None:
        return "(无 assistant)"
    return str(last.get("status"))


def main():
    bodies = body_map()
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    rows = con.execute(
        "SELECT id, title, status, updated_at FROM sessions ORDER BY updated_at DESC"
    ).fetchall()

    print(f"会话 {len(rows)} 条\n=== 非终态行 ===")
    n = 0
    for sid, title, status, _ in rows:
        if (status or "").strip().lower() in SETTLED:
            continue
        n += 1
        print(f"  {sid[:8]}  行状态={status!r:<10} 正文末条={tail_status(bodies.get(sid)):<14} {(title or '(空)')[:30]}")
    print(f"\n非终态合计: {n}")

    from collections import Counter
    print("行状态分布:", dict(Counter((r[2] or "(null)") for r in rows)))


if __name__ == "__main__":
    main()
