#!/usr/bin/env python3
"""Release 正文里 `release-smoke` 标记区块的剥离 / 提取。

设计依据：`delivery/design-release-smoke.md` §7.1（区块模板）、§7.2（幂等写入）。

`gh release edit --notes-file` 是**整篇替换**语义：想「只更新我们那一块」就必须
「先读出整篇 → 剥掉旧区块 → 拼上新区块 → 整篇写回」。本文件负责其中的**剥离/提取**，
不碰网络、不写文件、不猜顺序，所以可以离线自测。

区块形态
--------
```
（发行说明 / 资源说明……，逐字保留）

<!-- release-smoke:begin -->
**结论：PASS** · …
…
<!-- release-smoke:end -->
```

- 区块**一定**在正文末尾（§7.2 的约定），但本脚本不依赖这个位置：中间出现的区块也会剥掉；
- 出现**多个**区块时全部剥掉（历史事故：早期版本追加写入过），保证写回只有一份；
- **标记不成对**时保守处理：只有 `begin` 没有 `end` → 当作「一直到文末」剥掉（否则下次写回
  会把它和新区块一起留在正文里）；只有 `end` 没有 `begin` → **不动**（无法确定起点，
  宁可原样保留也不吞掉用户的发行说明），并在 stderr 记一条 warning；
- 原文**没有**区块时逐字原样返回（`printf '%s'` 级一致，不做任何格式化）。

幂等性
------
`strip(strip(x)) == strip(x)`：剥过一次后没有标记了，第二次走「原样返回」分支。
`strip` 只在**真的剥掉区块**时才把末尾多余空行收敛成单个换行——这样
`strip → 追加 '\n\n' + 新区块` 重复执行不会累积空行。

用法
----
    # 剥掉旧区块，输出剩下正文（写回脚本的第一半）
    split-note.py --strip --file body.md > body.stripped.md
    gh release view v0.2.5 --json body --jq .body | split-note.py --strip

    # 只取区块内容（读上一次的「本次处置」行，用于「每类每版本 ≤1」）
    split-note.py --extract --file body.md

    # 离线自测
    split-note.py --selftest

退出码：0 = 正常；1 = 输入不可解析 / I/O 失败；2 = 用法错误（argparse）。
"""

from __future__ import annotations

import argparse
import io
import sys
from typing import Any, Sequence

BEGIN_MARKER = "<!-- release-smoke:begin -->"
END_MARKER = "<!-- release-smoke:end -->"


def force_lf_stdout(stream: Any = None) -> None:
    """把输出流的换行固定成 `\\n`，关掉 Windows 的 CRLF 翻译。

    为什么必须做：`--strip` 的输出会被 `update-release-note.sh` **原样**写回 Release 正文。
    Windows 上 Python 的文本模式默认 `newline="\\r\\n"`，每一行都会被加上 `\\r`——
    于是「同一份正文写回两次」在 Windows 上会得到两份不同的字节（`diff` 每次都不同），
    幂等性只在 Linux/macOS 成立（DEF-5）。

    `reconfigure(newline=...)` 是 py3.7+ 的接口；被替换成非 `TextIOWrapper`（测试替身、
    已关闭的流）时静默跳过——「换行更干净」不该让主流程崩掉。

    `stream` 缺省用 `sys.stdout`；显式传入只是为了自测能打到一个临时流上。
    """
    target = sys.stdout if stream is None else stream
    reconfigure = getattr(target, "reconfigure", None)
    if not callable(reconfigure):
        return
    try:
        reconfigure(newline="\n")
    except (ValueError, OSError):  # 流已关闭 / 不支持该参数：退回默认行为
        pass


def split_blocks(
    body: str,
    begin: str = BEGIN_MARKER,
    end: str = END_MARKER,
) -> tuple[str, list[str], list[str]]:
    """把正文拆成 `(去掉所有区块的正文, 区块列表, warning 列表)`。

    纯函数：不读文件、不打印，便于自测。`warning` 里是「标记不成对」这类**可继续**的
    输入问题；真正无法处理的情况只有「`begin` 出现在 `end` 之后」这种矛盾输入，
    此时按「从 `begin` 剥到文末」处理并记 warning。
    """
    blocks: list[str] = []
    warnings: list[str] = []
    kept: list[str] = []
    cursor = 0
    length = len(body)
    while cursor < length:
        start = body.find(begin, cursor)
        if start < 0:
            kept.append(body[cursor:])
            break
        finish = body.find(end, start + len(begin))
        if finish < 0:
            # 只有 begin：保守剥到文末（否则下次写回会把残块留在正文里）。
            blocks.append(body[start:])
            warnings.append("标记不成对：有 begin 没有 end，已剥到文末")
            kept.append(body[cursor:start])
            cursor = length
            break
        stop = finish + len(end)
        blocks.append(body[start:stop])
        kept.append(body[cursor:start])
        cursor = stop
    # 残留的孤立 end（没有 begin）保持原样，只记 warning——此时无法判断区块起点。
    if end in "".join(kept) and begin not in "".join(kept):
        warnings.append("标记不成对：有 end 没有 begin（该行保持原样）")
    return "".join(kept), blocks, warnings


def strip_body(
    body: str,
    begin: str = BEGIN_MARKER,
    end: str = END_MARKER,
) -> tuple[str, list[str]]:
    """剥掉全部区块；返回 `(正文, warnings)`。

    - 没有区块 → **逐字原样**返回（调用方据此判断「要不要写回」也不会误判）；
    - 剥掉了区块 → 末尾连续空行收敛成一个换行，保证「拼-写-再剥-再拼」幂等。
    """
    kept, blocks, warnings = split_blocks(body, begin, end)
    if not blocks:
        return body, warnings
    return kept.rstrip() + "\n", warnings


def extract_blocks(
    body: str,
    begin: str = BEGIN_MARKER,
    end: str = END_MARKER,
) -> tuple[list[str], list[str]]:
    """只取区块内容（含标记行），按出现顺序返回。"""
    _kept, blocks, warnings = split_blocks(body, begin, end)
    return blocks, warnings


def compose(body: str, block: str, begin: str = BEGIN_MARKER, end: str = END_MARKER) -> str:
    """把新区块拼到（已剥离的）正文末尾，区块与正文之间留一个空行。

    区块必须自带成对标记，否则拼出来的正文下次无法被剥离（这里直接抛错，不猜）。
    """
    if begin not in block or end not in block:
        raise ValueError("新区块必须同时包含 begin / end 标记")
    head = body.rstrip()
    if not head:
        return block.rstrip() + "\n"
    return f"{head}\n\n{block.rstrip()}\n"


def _read_input(path: str | None) -> str:
    if path and path != "-":
        with open(path, encoding="utf-8") as handle:
            return handle.read()
    return sys.stdin.read()


def selftest() -> int:
    """离线自测：剥离 / 原样返回 / 多块 / 成对性 / 幂等。"""
    problems: list[str] = []

    block = f"{BEGIN_MARKER}\n**结论：PASS** · release-smoke · v0.2.5\n{END_MARKER}"
    body = "# 发行说明\n\n- 修了三个 bug\n\n" + block

    stripped, warnings = strip_body(body)
    if warnings:
        problems.append(f"正常输入不该有 warning：{warnings}")
    if BEGIN_MARKER in stripped or END_MARKER in stripped:
        problems.append("剥离后仍残留标记")
    if "修了三个 bug" not in stripped:
        problems.append("剥离把发行说明弄丢了")
    if stripped != "# 发行说明\n\n- 修了三个 bug\n":
        problems.append(f"剥离后正文形态不符：{stripped!r}")

    # 幂等：再剥一次必须逐字相同（此时已无标记 → 原样返回）。
    again, _ = strip_body(stripped)
    if again != stripped:
        problems.append("strip 不幂等（第二次输出与第一次不同）")

    # 没有区块 → 逐字原样（包括末尾空行也要保留）。
    plain = "只有发行说明\n\n\n"
    kept, _ = strip_body(plain)
    if kept != plain:
        problems.append("没有区块时未原样返回")

    # 多个区块 → 全部剥掉（历史事故：旧实现只剥第一个）。
    twice = f"{block}\n\n{block}"
    one, _ = strip_body(twice)
    if BEGIN_MARKER in one or one.strip():
        problems.append(f"多个区块未被全部剥掉：{one!r}")

    # 只有 begin 没有 end → 剥到文末 + warning。
    unpaired, unpaired_warnings = strip_body("# 说明\n\n" + BEGIN_MARKER + "\n残块")
    if END_MARKER in unpaired or BEGIN_MARKER in unpaired:
        problems.append("不成对的 begin 未被剥掉")
    if "# 说明" not in unpaired:
        problems.append("不成对 begin 的剥离吞掉了正文")
    if not unpaired_warnings:
        problems.append("不成对 begin 未记 warning")

    # 只有 end → 保持原样（不猜起点）。
    stray = "# 说明\n\n" + END_MARKER + "\n后面的话"
    stray_out, stray_warnings = strip_body(stray)
    if stray_out != stray:
        problems.append("孤立的 end 不该改动正文")
    if not stray_warnings:
        problems.append("孤立的 end 未记 warning")

    # extract 取到区块本身，也支持多个。
    blocks, _ = extract_blocks(body)
    if len(blocks) != 1 or "结论：PASS" not in blocks[0]:
        problems.append(f"extract 未取到区块：{blocks}")
    many, _ = extract_blocks(twice)
    if len(many) != 2:
        problems.append(f"extract 应取到 2 个区块，实际 {len(many)}")

    # compose + strip 往返：拼出来的正文必须能被剥回同一份 head（幂等写回的核心）。
    head = "# 发行说明\n\n- 修了三个 bug"
    composed = compose(head, block)
    round_trip, _ = strip_body(composed)
    if round_trip != head + "\n":
        problems.append(f"compose→strip 往返不一致：{round_trip!r}")
    if compose(round_trip, block) != composed:
        problems.append("连续两次 compose 的结果不同（写回会飘）")

    try:
        compose(head, "**结论：PASS**（缺标记）")
        problems.append("缺少标记的区块未被拒绝")
    except ValueError:
        pass

    # DEF-5：Windows 的文本模式会把 `\n` 翻成 `\r\n`，写回 Release 正文就不再幂等。
    # 用一对真实文本流复现「未处理」与「处理后」的字节差异（不依赖平台的默认 newline）。
    raw_before = io.BytesIO()
    stream_before = io.TextIOWrapper(raw_before, encoding="utf-8", newline="\r\n")
    stream_before.write("a\nb")
    stream_before.flush()
    if b"\r\n" not in raw_before.getvalue():
        problems.append("自测前提失败：newline='\\r\\n' 的文本流应当产出 CRLF")
    raw_after = io.BytesIO()
    stream_after = io.TextIOWrapper(raw_after, encoding="utf-8", newline="\r\n")
    force_lf_stdout(stream_after)
    stream_after.write("a\nb")
    stream_after.flush()
    if b"\r" in raw_after.getvalue():
        problems.append(f"force_lf_stdout 未关掉 CRLF 翻译（DEF-5）：{raw_after.getvalue()!r}")
    if raw_after.getvalue() != b"a\nb":
        problems.append(f"force_lf_stdout 后输出字节不对：{raw_after.getvalue()!r}")
    # 没有 reconfigure 的流（StringIO / 已关闭）不能抛异常。
    force_lf_stdout(io.StringIO())

    for message in problems:
        print(f"SELFTEST-FAIL {message}")
    if problems:
        return 1
    print("self-test passed")
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="split-note.py",
        description="剥离 / 提取 Release 正文里的 release-smoke 标记区块",
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--strip", action="store_true", help="剥掉所有区块，输出剩余正文（默认）")
    mode.add_argument("--extract", action="store_true", help="只输出区块内容（含标记）")
    parser.add_argument("--file", default=None, help="输入文件；缺省或 - 读 stdin")
    parser.add_argument("--selftest", action="store_true", help="离线自测")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    # 输出会被原样写回 Release 正文：必须固定成 LF（DEF-5，见 force_lf_stdout）。
    force_lf_stdout()

    try:
        body = _read_input(args.file)
    except OSError as error:
        print(f"ERROR 读取失败：{error}", file=sys.stderr)
        return 1

    if args.extract:
        blocks, warnings = extract_blocks(body)
        for warning in warnings:
            print(f"::warning::{warning}", file=sys.stderr)
        if not blocks:
            print("::warning::正文里没有 release-smoke 区块", file=sys.stderr)
            return 0
        sys.stdout.write("\n\n".join(block.rstrip() for block in blocks) + "\n")
        return 0

    stripped, warnings = strip_body(body)
    for warning in warnings:
        print(f"::warning::{warning}", file=sys.stderr)
    # 用 write 而不是 print：`--strip` 的输出会被原样写回 Release，不能多一个换行。
    sys.stdout.write(stripped if stripped.endswith("\n") or not stripped else stripped + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
