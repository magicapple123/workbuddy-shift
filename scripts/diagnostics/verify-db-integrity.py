#!/usr/bin/env python3
"""删除孤儿会话后的完整性核对（只读）。"""
import glob
import os
import sqlite3

HOME = os.path.expanduser("~")
DB = os.path.join(HOME, ".workbuddy", "workbuddy.db")

con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
print("integrity_check:", con.execute("PRAGMA integrity_check").fetchone()[0])
print("sessions:", con.execute("SELECT COUNT(*) FROM sessions").fetchone()[0])
for table in ("spaces", "workspaces", "automations", "session_usage"):
    print(f"{table}:", con.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0])
print("正文文件:", len(glob.glob(os.path.join(HOME, ".workbuddy", "projects", "*", "*.jsonl"))))
