#!/usr/bin/env python3
"""统计 addon_selection.welcomeMode / interactionMode 的取值分布，找运行态标记（只读）。"""
import json
import os
import sqlite3
from collections import Counter

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")


def main():
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    rows = con.execute("SELECT id, title, status, addon_selection FROM sessions").fetchall()
    modes = Counter()
    by_mode = {}
    for sid, title, status, raw in rows:
        mode = None
        if raw:
            try:
                data = json.loads(raw)
                if isinstance(data, dict):
                    mode = data.get("welcomeMode")
            except json.JSONDecodeError:
                pass
        modes[mode] += 1
        by_mode.setdefault(mode, []).append((sid, title, status))

    print("welcomeMode 分布:", dict(modes))
    for mode, items in by_mode.items():
        if mode in (None, "landing", "welcome"):
            continue
        print(f"\n=== welcomeMode={mode!r} 共 {len(items)} 条 ===")
        for sid, title, status in items[:12]:
            print(f"  {sid[:8]} {status:<9} {(title or '(空)')[:36]}")


if __name__ == "__main__":
    main()
