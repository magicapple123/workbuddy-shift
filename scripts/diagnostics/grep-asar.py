#!/usr/bin/env python3
"""只读：在 app.asar 里定位某个字符串，并打印它前后的上下文。

用途：客户端的错误码（如 HISTORY_LOAD_FAILED）只有打包后的 JS 里才有，
用来判断触发条件（大小上限、解析失败、超时）比猜快得多。

用法：
    python scripts/diagnostics/grep-asar.py HISTORY_LOAD_FAILED --window 400
"""

import sys
from pathlib import Path

ASAR = Path(r"D:\WorkBuddy\resources\app.asar")


def main() -> None:
    needle = sys.argv[1].encode()
    window = 400
    if "--window" in sys.argv:
        window = int(sys.argv[sys.argv.index("--window") + 1])

    data = ASAR.read_bytes()
    start = 0
    hits = 0
    while True:
        i = data.find(needle, start)
        if i < 0:
            break
        hits += 1
        lo, hi = max(0, i - window), min(len(data), i + window)
        chunk = data[lo:hi].decode("utf-8", errors="replace")
        print(f"===== 命中 {hits} @ 偏移 {i} =====")
        print(chunk.replace("\x00", "·"))
        print()
        start = i + 1
    print(f"共 {hits} 处")


if __name__ == "__main__":
    main()
