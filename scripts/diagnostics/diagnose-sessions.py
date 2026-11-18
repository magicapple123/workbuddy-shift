#!/usr/bin/env python3
"""诊断指定标题的会话：行状态、正文末条、中断记录、用量记录（只读）。

用法: python scripts/diagnostics/diagnose-sessions.py 关键词1 关键词2 ...
"""
import glob
import json
import os
import sqlite3
import sys

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")
PROJECTS = os.path.join(HOME, ".workbuddy", "projects")


def bodies():
    return {os.path.basename(p)[:-6]: p for p in glob.glob(os.path.join(PROJECTS, "*", "*.jsonl"))}


def tail_records(path, n=3):
    if not path:
        return []
    out = []
    with open(path, encoding="utf-8", errors="ignore") as fh:
        for line in fh:
            if not line.strip():
                continue
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    return out[-n:]


def main():
    keys = sys.argv[1:]
    if not keys:
        print("用法: python scripts/diagnostics/diagnose-sessions.py 关键词")
        return 1
    body = bodies()
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row

    cols = [r[1] for r in con.execute("PRAGMA table_info(sessions)")]
    for key in keys:
        print(f"\n{'=' * 60}\n关键词: {key}")
        rows = con.execute(
            "SELECT * FROM sessions WHERE title LIKE ?", (f"%{key}%",)
        ).fetchall()
        if not rows:
            print("  未找到")
            continue
        for r in rows:
            sid = r["id"]
            print(f"  id={sid[:8]} status={r['status']!r} updated={r['updated_at']}")
            for c in cols:
                if c in ("session_settings", "title", "id", "status", "updated_at", "created_at"):
                    continue
                v = r[c]
                if v not in (None, "", 0):
                    print(f"    {c} = {str(v)[:70]}")
            print(f"    正文: {body.get(sid, '(缺失)')}")
            for rec in tail_records(body.get(sid)):
                brief = {k: rec[k] for k in ("type", "role", "status") if k in rec}
                text = ""
                if isinstance(rec.get("content"), list) and rec["content"]:
                    text = str(rec["content"][0].get("text", ""))[:40]
                print(f"    末条: {brief} {text!r}")
            ic = con.execute(
                "SELECT COUNT(*) FROM session_turn_interruptions WHERE session_id=?", (sid,)
            ).fetchone()[0]
            print(f"    session_turn_interruptions: {ic} 行")
    return 0


if __name__ == "__main__":
    sys.exit(main())
