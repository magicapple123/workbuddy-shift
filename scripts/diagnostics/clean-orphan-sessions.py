#!/usr/bin/env python3
"""删除「库里有行、本地无正文」的孤儿会话。

孤儿 = sessions 表有记录，但 ~/.workbuddy/projects/<workspace>/<cid>.jsonl 不存在。
客户端点开这类会话读不到历史，会弹 HISTORY_LOAD_FAILED。

安全措施：
- 先整库备份（sqlite backup API，含 WAL 内容），备份路径打印出来
- 只删「正文文件确实不存在」的行，任何一条有正文就整体跳过
- 同时清理 session_usage / session_turn_interruptions 里指向它的行
- 默认干跑，--apply 才写库

用法：
  python scripts/diagnostics/clean-orphan-sessions.py            # 干跑，列出待删清单
  python scripts/diagnostics/clean-orphan-sessions.py --apply    # 备份后执行
"""
import argparse
import glob
import json
import os
import sqlite3
import sys
from datetime import datetime

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")
PROJECTS = os.path.join(HOME, ".workbuddy", "projects")
BACKUP_DIR = os.path.join(HOME, ".workbuddy", "orphan-cleanup-backup")


def body_ids():
    return {os.path.basename(p)[:-6] for p in glob.glob(os.path.join(PROJECTS, "*", "*.jsonl"))}


def find_orphans(con):
    have = body_ids()
    rows = con.execute("SELECT id, title, status, cwd, created_at FROM sessions").fetchall()
    return [r for r in rows if r[0] not in have]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--apply", action="store_true", help="真正执行删除（默认只干跑）")
    args = ap.parse_args()

    if not os.path.exists(DB):
        print(f"数据库不存在：{DB}")
        return 1

    con = sqlite3.connect(DB, timeout=10)
    con.execute("PRAGMA busy_timeout = 10000")

    orphans = find_orphans(con)
    print(f"会话总数 {con.execute('SELECT COUNT(*) FROM sessions').fetchone()[0]} | 孤儿 {len(orphans)}\n")
    if not orphans:
        print("没有需要清理的记录。")
        return 0

    for sid, title, status, cwd, created in orphans:
        when = datetime.fromtimestamp((created or 0) / 1000).strftime("%Y-%m-%d %H:%M")
        print(f"  {sid}  {status:<9} 建于 {when}  {(title or '(空标题)')[:34]}")

    # 兜底复核：任何一条只要能找到正文，就说明判定有误，整体中止
    have = body_ids()
    bad = [o[0] for o in orphans if o[0] in have]
    if bad:
        print(f"\n中止：这些 id 实际存在正文，判定异常 → {bad}")
        return 2

    if not args.apply:
        print("\n干跑结束，未修改。加 --apply 执行。")
        return 0

    os.makedirs(BACKUP_DIR, exist_ok=True)
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    backup = os.path.join(BACKUP_DIR, f"workbuddy-{stamp}.db")
    with sqlite3.connect(backup) as dst:
        con.backup(dst)
    print(f"\n备份：{backup}（{os.path.getsize(backup) / 1048576:.1f} MB）")

    ids = [o[0] for o in orphans]
    marks = ",".join("?" * len(ids))
    try:
        con.execute("BEGIN IMMEDIATE")
        usage = con.execute(f"DELETE FROM session_usage WHERE session_id IN ({marks})", ids).rowcount
        turns = con.execute(
            f"DELETE FROM session_turn_interruptions WHERE session_id IN ({marks})", ids
        ).rowcount
        removed = con.execute(f"DELETE FROM sessions WHERE id IN ({marks})", ids).rowcount
        con.commit()
    except Exception as exc:  # noqa: BLE001 - 出错必须整体回滚并报告
        con.rollback()
        print(f"执行失败已回滚：{exc}")
        return 3

    print(f"已删除 sessions {removed} 行、session_usage {usage} 行、session_turn_interruptions {turns} 行")

    left = con.execute("SELECT COUNT(*) FROM sessions").fetchone()[0]
    remain = find_orphans(con)
    print(f"复核：sessions 剩 {left} 行，孤儿剩 {len(remain)} 条")
    return 0


if __name__ == "__main__":
    sys.exit(main())
