#!/usr/bin/env python3
"""检查 automations / automation_runs / cloud_sync_ledger 的进行中记录（只读）。"""
import os
import sqlite3

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")


def show(con, table, limit=6):
    print(f"\n=== {table} ===")
    cols = [r[1] for r in con.execute(f"PRAGMA table_info('{table}')")]
    print("列:", cols)
    total = con.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
    print("行数:", total)
    for row in con.execute(f"SELECT * FROM {table} LIMIT {limit}"):
        print("  ", dict(zip(cols, row)))


def main():
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    for t in ("automations", "automation_runs", "automation_runtime_state", "cloud_sync_ledger",
              "session_turn_interruptions"):
        try:
            show(con, t)
        except sqlite3.Error as exc:
            print(f"\n=== {t} === 读取失败: {exc}")


if __name__ == "__main__":
    main()
