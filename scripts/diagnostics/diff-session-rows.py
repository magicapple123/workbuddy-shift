#!/usr/bin/env python3
"""对比「转圈会话」与「正常会话」的全部列，找出差异字段（只读）。

用法: python scripts/diagnostics/diff-session-rows.py
"""
import os
import sqlite3

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")

# 截图里转圈的三条（按最新一条取）与不转圈的对照组
SPIN = ["96fff7de", "f905da15"]
QUIET = ["f1a3ebfe", "7e13d67d"]  # 已知完成的普通会话


def fetch(con, prefix):
    return con.execute("SELECT * FROM sessions WHERE id LIKE ?", (prefix + "%",)).fetchone()


def main():
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    cols = [r[1] for r in con.execute("PRAGMA table_info(sessions)")]

    def row_map(r):
        return dict(zip(cols, r)) if r else None

    spin = [row_map(fetch(con, p)) for p in SPIN]
    quiet = [row_map(fetch(con, p)) for p in QUIET]
    spin = [r for r in spin if r]
    quiet = [r for r in quiet if r]
    if not spin or not quiet:
        print("样本不足")
        return

    print(f"转圈样本 {len(spin)} 条，对照 {len(quiet)} 条\n")
    print(f"{'列':<32}{'转圈':<34}{'正常'}")
    for c in cols:
        sv = {str(r.get(c)) for r in spin}
        qv = {str(r.get(c)) for r in quiet}
        if sv == qv:
            continue
        s = " | ".join(sorted(sv))[:32]
        q = " | ".join(sorted(qv))[:32]
        print(f"{c:<32}{s:<34}{q}")


if __name__ == "__main__":
    main()
