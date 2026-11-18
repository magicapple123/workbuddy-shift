#!/usr/bin/env python3
"""打印 sessions 表全部列名及非默认值分布，找可能的运行标记（只读）。"""
import os
import sqlite3

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")
INTEREST = ("run", "live", "active", "busy", "gen", "stream", "pending", "exec", "state", "agent")


def main():
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    cols = [r[1] for r in con.execute("PRAGMA table_info(sessions)")]
    print(f"sessions 共 {len(cols)} 列：")
    print("  " + ", ".join(cols))
    print("\n可疑列（名字含运行语义）:")
    hits = [c for c in cols if any(k in c.lower() for k in INTEREST)]
    print("  " + (", ".join(hits) if hits else "(无)"))

    for c in hits:
        vals = con.execute(
            f"SELECT {c}, COUNT(*) FROM sessions GROUP BY {c} ORDER BY 2 DESC LIMIT 5"
        ).fetchall()
        print(f"\n  {c} 取值分布:")
        for v, n in vals:
            print(f"    {v!r}: {n}")


if __name__ == "__main__":
    main()
