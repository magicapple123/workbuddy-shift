#!/usr/bin/env python3
"""修复跨账号复制导致的会话项目归属错位。

背景：WorkBuddy 用 `sessions.space_id` 指向 `spaces` 表来标记会话属于哪个项目。
早期版本的工具在跨账号复制时没有重映射 `space_id`，导致复制出的会话
指向源账号的 space（悬空）或为空，客户端解析不到就落进「最近」。

本脚本按会话的 `cwd` 精确匹配目标账号自己的同名项目目录，回填 `space_id`。

用法：
    python repair-session-space.py              # 干跑，只打印将要改什么
    python repair-session-space.py --apply      # 实际写库（自动备份）
    python repair-session-space.py --apply --db <路径>

要求：**关闭 WorkBuddy 客户端与切换工具后再执行 --apply**，避免写入被覆盖。
"""

import argparse
import os
import shutil
import sqlite3
import sys
import uuid
import time

DEFAULT_DB = os.path.expanduser(r"~\.workbuddy\workbuddy.db")


def norm(path):
    """路径归一化：分隔符统一、去尾部分隔符、忽略大小写。"""
    return (path or "").replace("\\", "/").rstrip("/").lower()


def analyze(con, create_missing=False):
    """返回：可回填列表、需新建项目的列表、无法匹配列表。

    create_missing=True 时，对「账号下有会话、但该账号没有对应项目」的情况，
    按同一目录在其它账号里的项目元信息生成一条新 spaces 记录（与工具复制时的
    行为一致），否则这些会话只能保持不动。
    """
    spaces = {}
    templates = {}
    for row in con.execute(
        "SELECT id, user_id, name, create_folder_path, device_id, source_kind, status, transport "
        "FROM spaces WHERE deleted_at = -1"
    ):
        sid, uid, name, folder, dev, kind, status, transport = row
        spaces.setdefault(uid, []).append({"id": sid, "name": name, "folder": folder})
        key = norm(folder)
        if key and key not in templates:
            templates[key] = {
                "name": name,
                "folder": folder,
                "device_id": dev,
                "source_kind": kind,
                "status": status or "active",
                "transport": transport or "local",
            }

    fixable, missing, cross, other = [], [], [], []
    rows = con.execute(
        """
        SELECT s.id, s.user_id, s.cwd, s.space_id, s.title, sp.user_id AS owner
        FROM sessions s
        LEFT JOIN spaces sp ON sp.id = s.space_id
        WHERE s.space_id IS NULL OR sp.user_id <> s.user_id
        """
    ).fetchall()

    for sid, uid, cwd, space_id, title, owner in rows:
        target = None
        cwd_n = norm(cwd)
        for sp in spaces.get(uid, []):
            folder_n = norm(sp["folder"])
            if not folder_n:
                continue
            if cwd_n == folder_n or cwd_n.startswith(folder_n + "/"):
                target = sp
                break
        item = {
            "id": sid,
            "user": uid,
            "cwd": cwd,
            "title": title,
            "old": space_id,
            "new": target["id"] if target else None,
            "project": target["name"] if target else None,
            "cross": bool(space_id) and owner != uid,
        }
        if target:
            fixable.append(item)
            if item["cross"]:
                cross.append(item)
            continue

        # 没有现成项目：看能不能按 cwd 在其它账号里找到同目录项目作为模板
        tpl = templates.get(cwd_n) or next(
            (t for k, t in templates.items() if cwd_n.startswith(k + "/")), None
        )
        if create_missing and tpl:
            item["project"] = tpl["name"]
            item["template"] = tpl
            missing.append(item)
        else:
            other.append(item)
    return fixable, missing, cross, other


def backup(db_path):
    stamp = time.strftime("%Y%m%d-%H%M%S")
    dst = f"{db_path}.bak-{stamp}"
    shutil.copy2(db_path, dst)
    return dst


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", default=DEFAULT_DB)
    ap.add_argument("--apply", action="store_true", help="实际写库（默认只干跑）")
    ap.add_argument(
        "--create-spaces",
        action="store_true",
        help="为「有会话但账号里没有该项目」的情况新建项目记录（默认不建）",
    )
    args = ap.parse_args()

    if not os.path.exists(args.db):
        print(f"找不到数据库：{args.db}")
        return 2

    con = sqlite3.connect(args.db)
    fixable, missing, cross, other = analyze(con, args.create_spaces)

    print(f"数据库：{args.db}")
    print(f"可回填到已有项目：{len(fixable)} 条（其中跨账号引用 {len(cross)} 条）")
    if args.create_spaces:
        print(f"需先新建项目再回填：{len(missing)} 条")
    else:
        print(f"账号内无该项目、可加 --create-spaces 处理：{len(missing)} 条")
    print(f"无法匹配、保持不动：{len(other)} 条\n")

    by_project = {}
    for item in fixable:
        by_project.setdefault(item["project"], []).append(item)
    for name, items in by_project.items():
        print(f"  → 项目「{name}」：{len(items)} 条")
        for item in items[:3]:
            label = "跨账号" if item["cross"] else "空归属"
            print(f"      [{label}] {item['id'][:8]}  {item['cwd']}  「{(item['title'] or '')[:24]}」")
        if len(items) > 3:
            print(f"      …… 其余 {len(items) - 3} 条")

    if missing:
        print(f"\n  需新建项目后回填的 {len(missing)} 条：")
        by_user = {}
        for item in missing:
            by_user.setdefault((item["user"], item["project"]), []).append(item)
        for (uid, name), items in by_user.items():
            print(f"      账号 {uid[:8]} → 新建项目「{name}」：{len(items)} 条")

    if other:
        print(f"\n  无法匹配的 {len(other)} 条（保持原状）：")
        for item in other[:3]:
            print(f"      {item['id'][:8]}  {item['cwd']}")
        if len(other) > 3:
            print(f"      …… 其余 {len(other) - 3} 条")

    if not args.apply:
        print("\n[干跑] 未写入。确认后加 --apply 执行（会自动备份）。")
        return 0

    bak = backup(args.db)
    print(f"\n已备份：{bak}")

    now = int(time.time() * 1000)
    changed = 0

    # 第一段：为缺失项目的账号建项目（每个 (账号, 项目) 只建一条）
    created = {}
    with con:
        for item in missing:
            key = (item["user"], item["project"])
            if key not in created:
                sid = str(uuid.uuid4())
                tpl = item["template"]
                con.execute(
                    "INSERT INTO spaces (id, user_id, device_id, name, create_folder_path, "
                    "pinned, status, transport, deleted_at, created_at, updated_at, source_kind) "
                    "VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                    (
                        sid,
                        item["user"],
                        tpl["device_id"],
                        tpl["name"],
                        tpl["folder"],
                        0,
                        tpl["status"],
                        tpl["transport"],
                        -1,
                        now,
                        now,
                        tpl["source_kind"],
                    ),
                )
                created[key] = sid
            item["new"] = created[key]

    print(f"已新建项目 {len(created)} 条。")

    with con:
        for item in fixable + missing:
            cur = con.execute(
                "UPDATE sessions SET space_id = ?, updated_at = ? WHERE id = ? AND user_id = ?",
                (item["new"], now, item["id"], item["user"]),
            )
            changed += cur.rowcount
    con.close()

    print(f"已回填 {changed} 条。")

    verify = sqlite3.connect(f"file:{args.db}?mode=ro", uri=True)
    bad = verify.execute(
        """
        SELECT COUNT(*) FROM sessions s JOIN spaces sp ON sp.id = s.space_id
        WHERE sp.user_id <> s.user_id
        """
    ).fetchone()[0]
    left = verify.execute("SELECT COUNT(*) FROM sessions WHERE space_id IS NULL").fetchone()[0]
    print(f"校验：跨账号引用 {bad} 条，仍未归属 {left} 条（含本来无项目的会话）。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
