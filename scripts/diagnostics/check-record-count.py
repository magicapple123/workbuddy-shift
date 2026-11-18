#!/usr/bin/env python3
"""核对 sessions 表里的记录数与正文实际行数是否一致，并检查结构性异常。

只读。
"""
import glob
import json
import os
import sqlite3

HOME = os.path.expanduser("~")
PROJECTS = os.path.join(HOME, ".workbuddy", "projects")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")


def body_index():
    return {os.path.basename(p)[:-6]: p for p in glob.glob(os.path.join(PROJECTS, "*", "*.jsonl"))}


def main():
    files = body_index()
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    cols = [r[1] for r in con.execute("PRAGMA table_info(sessions)")]
    has_count = "record_count" in cols or "message_count" in cols
    print("sessions 是否有计数列:", has_count, "| 列名候选:", [c for c in cols if "count" in c])

    rows = con.execute("SELECT id, title, status FROM sessions").fetchall()
    mismatch = []
    missing = []
    dup_last = []

    for sid, title, status in rows:
        path = files.get(sid)
        if not path:
            missing.append((sid, title, status))
            continue
        with open(path, encoding="utf-8", errors="ignore") as fh:
            lines = [x for x in fh.read().splitlines() if x.strip()]
        # 最后一条记录是否是 assistant 且 incomplete
        last = None
        for line in reversed(lines):
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if rec.get("type") == "message" and rec.get("role") == "assistant":
                last = rec
                break
        if last and last.get("status") == "incomplete":
            dup_last.append((sid, title, status, len(lines)))

    print(f"\n会话 {len(rows)} | 找不到正文 {len(missing)} | 末条仍 incomplete {len(dup_last)}")
    for sid, title, status in missing[:5]:
        print(f"   缺正文: {sid[:8]} db={status} 「{(title or '')[:24]}」")
    for sid, title, status, n in dup_last[:8]:
        print(f"   末条incomplete: {sid[:8]} db={status} 行数={n} 「{(title or '')[:24]}」")

    # 记录数一致性（若存在计数列）
    for col in ("record_count", "message_count"):
        if col in cols:
            bad = 0
            for sid, cnt in con.execute(f"SELECT id, {col} FROM sessions WHERE {col} IS NOT NULL"):
                path = files.get(sid)
                if not path:
                    continue
                with open(path, encoding="utf-8", errors="ignore") as fh:
                    n = len([x for x in fh.read().splitlines() if x.strip()])
                if n != cnt:
                    bad += 1
                    if bad <= 5:
                        print(f"   计数不一致 {sid[:8]}: 库={cnt} 正文={n}")
            print(f"   {col} 不一致的会话数: {bad}")


if __name__ == "__main__":
    main()
