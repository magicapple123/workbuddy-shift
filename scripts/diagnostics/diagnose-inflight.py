#!/usr/bin/env python3
"""只看"进行中"的记录：automation_runs 的状态分布 + cloud_sync_ledger 结构（只读）。"""
import os
import sqlite3
from collections import Counter

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")


def cols(con, t):
    return [r[1] for r in con.execute(f"PRAGMA table_info('{t}')")]


def main():
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)

    print("=== automation_runs ===")
    c = cols(con, "automation_runs")
    print("列:", c)
    print("行数:", con.execute("SELECT COUNT(*) FROM automation_runs").fetchone()[0])
    if "status" in c:
        for status, n in con.execute("SELECT status, COUNT(*) FROM automation_runs GROUP BY status"):
            print(f"  status={status!r}: {n}")
    for row in con.execute(
        "SELECT * FROM automation_runs WHERE status NOT IN ('completed','failed','error','cancelled','skipped') LIMIT 5"
    ):
        print("  进行中:", dict(zip(c, row)))

    print("\n=== cloud_sync_ledger ===")
    c = cols(con, "cloud_sync_ledger")
    print("列:", c)
    print("行数:", con.execute("SELECT COUNT(*) FROM cloud_sync_ledger").fetchone()[0])
    for row in con.execute("SELECT * FROM cloud_sync_ledger LIMIT 3"):
        print("  ", dict(zip(c, row)))

    print("\n=== sessions 里 updated_at 最近的 5 条 ===")
    for row in con.execute(
        "SELECT id, title, status, updated_at FROM sessions ORDER BY updated_at DESC LIMIT 5"
    ):
        print("  ", row[0][:8], row[2], (row[1] or "")[:28])


if __name__ == "__main__":
    main()
