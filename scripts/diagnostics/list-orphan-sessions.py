#!/usr/bin/env python3
"""列出「库里有会话行、本地无正文文件」的孤儿记录，并判断是否近期产生。只读。"""
import glob
import os
import sqlite3
import time
from datetime import datetime

HOME = os.path.expanduser("~")
PROJECTS = os.path.join(HOME, ".workbuddy", "projects")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")


def main():
    have = {os.path.basename(p)[:-6] for p in glob.glob(os.path.join(PROJECTS, "*", "*.jsonl"))}
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row

    rows = con.execute("""SELECT id, title, status, cwd, created_at, updated_at, space_id, transport,
                                 conversation_origin, visibility
                          FROM sessions ORDER BY updated_at DESC""").fetchall()

    orphans = [r for r in rows if r["id"] not in have]
    print(f"会话 {len(rows)} | 孤儿记录 {len(orphans)}\n")
    now = time.time() * 1000
    for r in orphans:
        age = (now - (r["updated_at"] or now)) / 3600000
        created = datetime.fromtimestamp((r["created_at"] or 0) / 1000).strftime("%Y-%m-%d %H:%M")
        print(f"  {r['id'][:8]}  status={r['status']:<9} {age:6.1f}h前  建于 {created}")
        print(f"     标题: {(r['title'] or '(空)')[:40]}")
        print(f"     cwd={r['cwd']}  transport={r['transport']}  origin={r['conversation_origin']}")

    # 反向：有正文但库里没记录
    ids = {r["id"] for r in rows}
    extra = [p for p in glob.glob(os.path.join(PROJECTS, "*", "*.jsonl"))
             if os.path.basename(p)[:-6] not in ids]
    print(f"\n有正文但库里无记录的文件: {len(extra)}")
    for p in extra[:5]:
        print("   ", os.path.basename(p))


if __name__ == "__main__":
    main()
