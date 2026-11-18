#!/usr/bin/env python3
"""诊断会话的云同步状态（只读）。

客户端在会话标题右侧的转圈图标有两种来源：本地运行中、云端同步中。
本脚本把两张表里与同步相关的字段都打出来，用来区分。
"""
import glob
import os
import sqlite3

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")
KEYS = ("推送GitHub", "ResumeForge 剩余", "排查切换账号")


def main():
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    cols = [r[1] for r in con.execute("PRAGMA table_info(sessions)")]
    sync_cols = [c for c in cols if any(k in c.lower() for k in ("cloud", "sync", "state", "activity"))]
    print("sessions 同步相关列:", sync_cols)

    for key in KEYS:
        rows = con.execute("SELECT * FROM sessions WHERE title LIKE ?", (f"%{key}%",)).fetchall()
        for r in rows:
            sid = r["id"]
            print(f"\n{key}  id={sid[:8]} status={r['status']!r}")
            for c in sync_cols:
                v = r[c]
                if v not in (None, "", 0):
                    print(f"   {c} = {str(v)[:90]}")
            ledger = con.execute(
                "SELECT * FROM cloud_sync_ledger WHERE session_id = ?", (sid,)
            ).fetchall()
            print(f"   cloud_sync_ledger: {len(ledger)} 行")
            for row in ledger:
                print("     ", {k: row[k] for k in row.keys()})

    print("\n=== cloud_sync_ledger 概览 ===")
    total = con.execute("SELECT COUNT(*) FROM cloud_sync_ledger").fetchone()[0]
    print("总行数:", total)
    lcols = [r[1] for r in con.execute("PRAGMA table_info(cloud_sync_ledger)")]
    print("列:", lcols)
    for row in con.execute("SELECT * FROM cloud_sync_ledger LIMIT 5"):
        print("  ", {k: row[k] for k in row.keys()})


if __name__ == "__main__":
    main()
