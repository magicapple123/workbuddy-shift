#!/usr/bin/env python3
"""检查 session_usage 是否有未闭合的记录（只读）。"""
import os
import sqlite3

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")
SPIN = ["96fff7de", "f905da15", "65e1f127"]


def main():
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    cols = [r[1] for r in con.execute("PRAGMA table_info(session_usage)")]
    print("session_usage 列:", cols)
    print("总行数:", con.execute("SELECT COUNT(*) FROM session_usage").fetchone()[0])
    for prefix in SPIN:
        rows = con.execute(
            "SELECT * FROM session_usage WHERE session_id LIKE ?", (prefix + "%",)
        ).fetchall()
        print(f"\n{prefix}: {len(rows)} 行")
        for r in rows:
            print("  ", dict(zip(cols, r)))


if __name__ == "__main__":
    main()
