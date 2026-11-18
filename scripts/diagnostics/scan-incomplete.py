#!/usr/bin/env python3
"""扫描「正文末条为 incomplete」的会话（客户端会误显示为正在执行）。

只读，不修改任何文件。
"""
import glob
import json
import os
import sqlite3

HOME = os.path.expanduser("~")
PROJECTS = os.path.join(HOME, ".workbuddy", "projects")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")


def index_bodies():
    files = {}
    for path in glob.glob(os.path.join(PROJECTS, "*", "*.jsonl")):
        files[os.path.basename(path)[:-6]] = path
    return files


def last_assistant_status(path):
    try:
        with open(path, "rb") as fh:
            fh.seek(0, os.SEEK_END)
            size = fh.tell()
            fh.seek(max(0, size - 200000))
            text = fh.read().decode("utf-8", "ignore")
        for line in reversed([x for x in text.splitlines() if x.strip()]):
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if rec.get("type") == "message" and rec.get("role") == "assistant":
                content = rec.get("content") or [{}]
                return rec.get("status"), str(content[0].get("text", ""))[:24]
    except OSError:
        return None, None
    return None, None


def main():
    files = index_bodies()
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    rows = con.execute("SELECT id, title, status FROM sessions").fetchall()

    missing = [row for row in rows if row[0] not in files]
    bad = []
    for sid, title, status in rows:
        path = files.get(sid)
        if not path:
            continue
        st, snippet = last_assistant_status(path)
        if st == "incomplete":
            bad.append((sid, title or "(无标题)", status, snippet))

    print(f"会话总数 {len(rows)} | 找不到正文 {len(missing)}")
    print(f"末条为 incomplete 的会话：{len(bad)}\n")
    for sid, title, status, snippet in bad:
        print(f"  {sid[:8]}  db={status:<10} 「{title[:26]}」  末条='{snippet}'")
    if missing:
        print("\n正文缺失的会话：")
        for sid, title, status in missing[:8]:
            print(f"  {sid[:8]}  db={status:<10} 「{(title or '')[:26]}」")


if __name__ == "__main__":
    main()
