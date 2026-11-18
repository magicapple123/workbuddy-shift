#!/usr/bin/env python3
"""只读：检查某个会话正文的行长度分布，找出异常超长的行。

客户端的「历史加载」是分页读取的；某一行特别大（内联图片、超长工具输出）
会让单次读取/解析变慢甚至超时，表现就是 HISTORY_LOAD_FAILED。

用法：
    python scripts/diagnostics/check-line-sizes.py <会话 id 前缀>

只统计，不修改任何文件。
"""

import glob
import os
import sys


def main() -> None:
    sid = sys.argv[1] if len(sys.argv) > 1 else "fe277719"
    pattern = os.path.expanduser(os.path.join("~/.workbuddy/projects", "*", f"{sid}*.jsonl"))
    paths = sorted(glob.glob(pattern))
    if not paths:
        print(f"没有找到 {sid} 的正文文件")
        return
    for path in paths:
        sizes = []
        with open(path, "rb") as fh:
            for lineno, raw in enumerate(fh, 1):
                sizes.append((len(raw), lineno))
        sizes.sort(reverse=True)
        total = sum(s for s, _ in sizes)
        print(f"{os.path.basename(path)}")
        print(f"  总字节 {total / 1048576:.2f} MB，行数 {len(sizes)}，"
              f"平均 {total / max(1, len(sizes)):.0f} B，最大 {sizes[0][0] / 1024:.0f} KB")
        for size, lineno in sizes[:5]:
            print(f"    最长行 行 {lineno}: {size / 1024:.0f} KB")
        print(f"  >256 KB 的行: {sum(1 for s, _ in sizes if s > 262144)}")
        print(f"  >64 KB 的行: {sum(1 for s, _ in sizes if s > 65536)}")


if __name__ == "__main__":
    main()
