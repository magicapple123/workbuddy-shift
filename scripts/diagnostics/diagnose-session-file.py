#!/usr/bin/env python3
"""只读诊断：某个会话的正文文件是否可被解析，以及数据库里的行是否对得上。

用法：
    python scripts/diagnostics/diagnose-session-file.py <会话 id 前缀或全量>

为什么会用到：客户端提示 HISTORY_LOAD_FAILED（"历史文件存在异常，会话暂不可用"），
但当前对话仍能继续——通常是正文 .jsonl 里有一行被写坏（进程被杀时留下半行），
历史加载器整份放弃，而内存里的会话还在跑。这个脚本只读，不修任何东西。
"""

import glob
import json
import os
import sqlite3
import sys


def main() -> None:
    sid = sys.argv[1] if len(sys.argv) > 1 else "fe277719"
    root = os.path.expanduser("~/.workbuddy")
    files = sorted(glob.glob(os.path.join(root, "projects", "*", f"{sid}*.jsonl")))
    print(f"正文文件 {len(files)} 个")
    for path in files:
        size = os.path.getsize(path)
        bad = []
        total = 0
        last_ts = None
        with open(path, "rb") as fh:
            for lineno, raw in enumerate(fh, 1):
                if not raw.strip():
                    continue
                total += 1
                try:
                    obj = json.loads(raw)
                    if isinstance(obj, dict):
                        last_ts = obj.get("timestamp") or last_ts
                except Exception as exc:  # noqa: BLE001 - 诊断脚本，任何解析失败都要报
                    bad.append((lineno, len(raw), str(exc)[:70], raw[:100]))
        print(f"  {os.path.basename(path)}")
        print(f"    体积 {size / 1048576:.2f} MB / {total} 行 / 坏行 {len(bad)}")
        print(f"    末条时间戳 {last_ts}")
        for lineno, length, err, head in bad[-3:]:
            print(f"    坏行 {lineno}: 长度 {length} 字节 — {err}")
            print(f"      {head!r}")

    db = os.path.join(root, "workbuddy.db")
    if os.path.exists(db):
        con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        con.row_factory = sqlite3.Row
        rows = con.execute(
            "SELECT id, title, status, cwd, created_at, updated_at FROM sessions WHERE id LIKE ?",
            (f"{sid}%",),
        ).fetchall()
        print(f"\n数据库行 {len(rows)} 条")
        for row in rows:
            print(f"  {row['id']}  status={row['status']}  updated={row['updated_at']}")
            print(f"    title={row['title']!r}  cwd={row['cwd']}")


if __name__ == "__main__":
    main()
