#!/usr/bin/env python3
"""校验同目录下 fix-incomplete-turns.py 的改动范围：只应改动末行，行数与正文不得变化。"""
import glob
import json
import os

HOME = os.path.expanduser("~")
bak = max(glob.glob(os.path.join(HOME, ".workbuddy", "incomplete-backup-*")), key=os.path.getmtime)
print("备份目录:", bak)

target = "775a1c1c"
cur = glob.glob(os.path.join(HOME, ".workbuddy", "projects", "*", target + "*.jsonl"))[0]
old = open(os.path.join(bak, os.path.basename(cur)), encoding="utf-8").read().splitlines()
new = open(cur, encoding="utf-8").read().splitlines()

print("行数:", len(old), "→", len(new), "| 一致:", len(old) == len(new))
diff = [i for i, (a, b) in enumerate(zip(old, new)) if a != b]
print("差异行号:", diff, "(应只有最后一行)")

rec = json.loads(new[-1])
content = rec.get("content") or [{}]
print("末条 status:", rec.get("status"))
print("末条正文:", str(content[0].get("text"))[:26])
print("留痕:", (rec.get("_meta") or {}).get("incompleteNormalized"))

bad = 0
for line in new[-300:]:
    if not line.strip():
        continue
    try:
        json.loads(line)
    except json.JSONDecodeError:
        bad += 1
print("末尾 300 行解析失败:", bad)
