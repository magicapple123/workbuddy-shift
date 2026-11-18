#!/usr/bin/env python3
"""打印指定会话的 session_settings / addon_selection JSON，找运行态标记（只读）。"""
import json
import os
import sqlite3

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")
KEYS = ("推送GitHub", "ResumeForge 剩余", "排查切换账号")
HINT = ("run", "stream", "busy", "pending", "status", "state", "exec", "turn")


def walk(obj, prefix=""):
    hits = []
    if isinstance(obj, dict):
        for k, v in obj.items():
            p = f"{prefix}.{k}" if prefix else k
            if any(h in k.lower() for h in HINT):
                hits.append((p, v if not isinstance(v, (dict, list)) else f"<{type(v).__name__} len={len(v)}>"))
            hits += walk(v, p)
    elif isinstance(obj, list):
        for i, v in enumerate(obj[:5]):
            hits += walk(v, f"{prefix}[{i}]")
    return hits


def main():
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    for key in KEYS:
        rows = con.execute(
            "SELECT id, title, status, session_settings, addon_selection, agent_last_synced "
            "FROM sessions WHERE title LIKE ?", (f"%{key}%",)
        ).fetchall()
        for r in rows:
            print(f"\n=== {key}  id={r['id'][:8]} status={r['status']!r} synced={r['agent_last_synced']!r}")
            for field in ("session_settings", "addon_selection"):
                raw = r[field]
                if not raw:
                    print(f"  {field}: (空)")
                    continue
                try:
                    data = json.loads(raw)
                except (json.JSONDecodeError, TypeError):
                    print(f"  {field}: (无法解析) {str(raw)[:80]}")
                    continue
                hits = walk(data)
                print(f"  {field} 键: {list(data)[:12] if isinstance(data, dict) else type(data)}")
                for p, v in hits[:12]:
                    print(f"    {p} = {v}")


if __name__ == "__main__":
    main()
