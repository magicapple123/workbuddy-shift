#!/usr/bin/env python3
"""检查每条会话末条 assistant 消息是否带 providerData.error（只读）。

用法: python scripts/diagnostics/check-provider-error.py [标题关键词 ...]
不传参数则检查全部会话。
"""
import glob
import json
import os
import sqlite3
import sys

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")
PROJECTS = os.path.join(HOME, ".workbuddy", "projects")


def last_assistant(path):
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
    return last


def main():
    keys = sys.argv[1:]
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    sql = "SELECT id, title, status FROM sessions"
    params = []
    if keys:
        sql += " WHERE " + " OR ".join("title LIKE ?" for _ in keys)
        params = [f"%{k}%" for k in keys]
    rows = con.execute(sql, params).fetchall()

    bodies = {}
    for p in glob.glob(os.path.join(PROJECTS, "*", "*.jsonl")):
        bodies[os.path.basename(p)[:-6]] = p

    print(f"{'会话':<26}{'行状态':<10}{'末条status':<12}{'skipRun':<9}{'error':<7}标题")
    for r in rows:
        path = bodies.get(r["id"])
        if not path:
            continue
        rec = last_assistant(path)
        if rec is None:
            print(f"{r['id'][:8]:<26}{str(r['status']):<10}{'(无 assistant)':<12}{'-':<9}{'-':<7}{(r['title'] or '')[:26]}")
            continue
        pd = rec.get("providerData") or {}
        err = pd.get("error")
        print(
            f"{r['id'][:8]:<26}{str(r['status']):<10}{str(rec.get('status')):<12}"
            f"{str(pd.get('skipRun')):<9}{('yes' if err else 'no'):<7}{(r['title'] or '')[:26]}"
        )


if __name__ == "__main__":
    main()
