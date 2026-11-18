#!/usr/bin/env python3
"""汇总 teams 目录下每个团队的会话绑定与运行状态（只读）。"""
import json
import os

HOME = os.path.expanduser("~")
TEAMS = os.path.join(HOME, ".workbuddy", "teams")


def main():
    if not os.path.isdir(TEAMS):
        print("没有 teams 目录")
        return
    for name in sorted(os.listdir(TEAMS)):
        d = os.path.join(TEAMS, name)
        if not os.path.isdir(d):
            continue
        info = {"team": name}
        for f in ("config.json", "runtime.json"):
            p = os.path.join(d, f)
            if not os.path.exists(p):
                continue
            try:
                data = json.load(open(p, encoding="utf-8"))
            except (json.JSONDecodeError, OSError) as exc:
                info[f] = f"(读取失败 {exc})"
                continue
            if isinstance(data, dict):
                keep = {}
                for k, v in data.items():
                    kl = k.lower()
                    if any(t in kl for t in ("session", "status", "state", "run", "active", "updated")):
                        keep[k] = v if not isinstance(v, (dict, list)) else f"<{type(v).__name__}>"
                info[f] = keep
        print(json.dumps(info, ensure_ascii=False), "\n")


if __name__ == "__main__":
    main()
