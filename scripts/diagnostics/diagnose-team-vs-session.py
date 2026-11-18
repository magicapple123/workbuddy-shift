#!/usr/bin/env python3
"""团队运行态与主会话状态对照表（只读）。

用途：判断侧边栏转圈是否来自「团队仍处于 active」。
"""
import json
import os
import sqlite3

HOME = os.path.expanduser("~")
TEAMS = os.path.join(HOME, ".workbuddy", "teams")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")


def main():
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row

    rows = []
    for name in sorted(os.listdir(TEAMS)):
        d = os.path.join(TEAMS, name)
        run = os.path.join(d, "runtime.json")
        if not os.path.isfile(run):
            continue
        try:
            data = json.load(open(run, encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            continue
        sid = data.get("leadSessionId")
        members = data.get("members") or []
        member_states = {}
        for m in members:
            s = m.get("status")
            member_states[s] = member_states.get(s, 0) + 1
        sess = None
        if sid:
            sess = con.execute("SELECT id, title, status FROM sessions WHERE id = ?", (sid,)).fetchone()
        rows.append({
            "team": name,
            "team_status": data.get("status"),
            "members": member_states,
            "session": (sess["title"][:26] if sess else "(会话不存在)"),
            "row_status": (sess["status"] if sess else "-"),
            "sid": (sid or "")[:8],
        })

    print(f"{'团队':<34}{'团队状态':<9}{'成员':<22}{'会话行状态':<11}标题")
    for r in rows:
        print(f"{r['team']:<34}{str(r['team_status']):<9}{str(r['members']):<22}{str(r['row_status']):<11}{r['session']}")

    active = [r for r in rows if r["team_status"] == "active"]
    print(f"\n团队总数 {len(rows)} | active {len(active)}")
    missing = [r for r in rows if r["session"] == "(会话不存在)"]
    print(f"主会话已不存在的团队 {len(missing)}: {[r['team'] for r in missing]}")


if __name__ == "__main__":
    main()
