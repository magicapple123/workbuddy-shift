#!/usr/bin/env python3
"""把「正文末条 assistant 消息为 incomplete」的会话标记为已完成。

背景：会话正文（projects/<workspace>/<cid>.jsonl）的最后一条 assistant 消息
若停在 status="incomplete"（被用户中断、429 限流、500 报错等），WorkBuddy 客户端
会把该会话显示成「思考中」，处理时间持续累加——即使它早已结束。数据库里
sessions.status 可能是 completed/error，与正文不一致。

本脚本只改末条消息的 status（incomplete → completed），正文内容一字不动，
并在 _meta 里留一个标记便于追溯。

用法：
    python fix-incomplete-turns.py              # 干跑
    python fix-incomplete-turns.py --apply      # 执行（逐个文件先备份）

要求：执行前关闭 WorkBuddy 客户端。
"""

import argparse
import glob
import json
import os
import shutil
import sqlite3
import sys
import time

HOME = os.path.expanduser("~")
PROJECTS = os.path.join(HOME, ".workbuddy", "projects")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")


def index_bodies():
    return {os.path.basename(p)[:-6]: p for p in glob.glob(os.path.join(PROJECTS, "*", "*.jsonl"))}


def find_last_assistant(lines):
    for idx in range(len(lines) - 1, -1, -1):
        if not lines[idx].strip():
            continue
        try:
            rec = json.loads(lines[idx])
        except json.JSONDecodeError:
            continue
        if rec.get("type") == "message" and rec.get("role") == "assistant":
            return idx, rec
    return None, None


def scan():
    files = index_bodies()
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    rows = con.execute("SELECT id, title, status FROM sessions").fetchall()

    hits = []
    for sid, title, status in rows:
        # 真正在跑的会话不能碰：它们末条 incomplete 是正常流式状态
        if status in ("working", "active"):
            continue
        path = files.get(sid)
        if not path:
            continue
        try:
            with open(path, encoding="utf-8", errors="ignore") as fh:
                lines = fh.read().splitlines()
        except OSError:
            continue
        idx, rec = find_last_assistant(lines)
        if not rec or rec.get("status") != "incomplete":
            continue
        content = rec.get("content") or [{}]
        text = str(content[0].get("text", ""))[:28]
        hits.append({"id": sid, "title": title or "(无标题)", "db": status,
                     "path": path, "line": idx, "text": text})
    return hits


def apply(hits):
    stamp = time.strftime("%Y%m%d-%H%M%S")
    backup_dir = os.path.join(HOME, ".workbuddy", f"incomplete-backup-{stamp}")
    os.makedirs(backup_dir, exist_ok=True)
    print(f"备份目录：{backup_dir}")

    changed = 0
    for item in hits:
        path = item["path"]
        shutil.copy2(path, os.path.join(backup_dir, os.path.basename(path)))
        with open(path, encoding="utf-8", errors="ignore") as fh:
            lines = fh.read().splitlines()
        idx, rec = find_last_assistant(lines)
        if not rec:
            continue
        rec["status"] = "completed"
        meta = rec.get("_meta")
        if not isinstance(meta, dict):
            meta = {}
        meta["incompleteNormalized"] = {"at": stamp, "reason": "phantom running state fix"}
        rec["_meta"] = meta
        lines[idx] = json.dumps(rec, ensure_ascii=False)
        with open(path, "w", encoding="utf-8", newline="") as fh:
            fh.write("\n".join(lines) + "\n")
        changed += 1
    print(f"已修正 {changed} 个会话正文。")
    return backup_dir


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args()

    hits = scan()
    print(f"末条为 incomplete 的会话：{len(hits)}\n")
    for item in hits:
        print(f"  {item['id'][:8]}  db={item['db']:<10} 「{item['title'][:26]}」  末条='{item['text']}'")

    if not args.apply:
        print("\n[干跑] 未修改任何文件。确认后加 --apply（会先逐个备份）。")
        return 0

    if not hits:
        return 0
    apply(hits)
    print("\n复核：")
    left = scan()
    print(f"  仍为 incomplete 的会话：{len(left)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
