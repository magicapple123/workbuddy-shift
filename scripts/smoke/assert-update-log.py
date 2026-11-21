#!/usr/bin/env python3
"""断言 `--smoke-self-update` 写出的 JSON Lines 日志（Windows in-app 升级的唯一硬证据）。

设计依据：`delivery/design-release-smoke.md` §3.1.3（日志契约）、§5.1 W6（断言什么）、
§8.1（失败归类 G/F/H）。日志的**生产者**是 `src-tauri/src/update_service/smoke.rs`，
它把 `phase` 词表与字段名写死在 Rust 里；本文件是消费侧，**只按词表做精确匹配**，
不做模糊匹配、不猜。

三个必须同时成立的断言
----------------------
1. **阶段序列**：默认要求 `start → check → check_ok → download_start → download_done
   → install_start → install_ok` 全部出现**且顺序正确**；缺哪个阶段直接指出哪个。
   安装器接管进程、把 App 杀掉导致 `install_ok` 没机会落盘时，调用方用
   `--allow-missing install_ok`（Windows）显式豁免——豁免只降级成 NOTE，不静默通过，
   且写入格子的 `detail` 里必须由 driver 说明理由。
2. **版本**：凡是带 `version` 的事件，都必须等于目标版本（`0.2.5`）；`start` / `check`
   两个事件的 `version` 恒为 `null`（Rust 侧就是这样写的），不当作缺失。
3. **没有失败事件**：出现 `phase=error` 或 `phase=abort` 即失败；`up_to_date` 说明
   旧版本认为「自己已是最新」，按 H 类（版本不一致）归类。

为什么「按 pid 分组、取第一个进程」
-----------------------------------
Windows 的 NSIS 安装完可能**自动拉起新版本进程**，而 `restart()` 在 macOS / Linux 会把
`--smoke-self-update` 连同环境变量转给新进程。新进程若写同一份日志，就会把自己的
`start` 插进序列里，污染断言。因此：

- 按 `pid` 分组 → 取**文件里最先出现**的那个 pid（= 被 CI 启动的旧版本进程）；
- 组内按 `seq` 排序（`seq` 从 0 单调递增，跨进程会重复，所以必须先分组）；
- 其余进程只统计数量，写进 `notes`/`processCount` 作 bonus 证据（「升级后确实重启了」）。

用法
----
    # CI（Windows driver 里调用）：失败时用 --json 拿 category 做归类
    python3 scripts/smoke/assert-update-log.py "$sb/smoke-self-update.jsonl" \\
        --target 0.2.5 --expect-current 0.2.4 \\
        --allow-missing install_ok --allow-missing relaunch --json \\
        --out artifacts/release-smoke/update-log.json

    # 只做序列断言（阶段表用默认的）
    python3 scripts/smoke/assert-update-log.py smoke.jsonl --target 0.2.5

    # 离线自测（造 3 份 JSONL：正常 / 缺阶段 / 含 error）
    python3 scripts/smoke/assert-update-log.py --selftest

退出码：0 = 全部断言成立；1 = 有不成立项；2 = 用法错误（argparse）。
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
import tempfile
from dataclasses import dataclass, field
from typing import Any, Iterable, Sequence

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))


def _load_common() -> Any:
    """按路径加载同目录的 `common.py`（版本归一化等口径的唯一真源）。"""
    path = os.path.join(SCRIPT_DIR, "common.py")
    if not os.path.exists(path):
        raise SystemExit(f"缺少契约模块：{path}")
    spec = importlib.util.spec_from_file_location("wb_smoke_common", path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"无法加载契约模块：{path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


common = _load_common()

#: phase 词表（与 `src-tauri/src/update_service/smoke.rs` 的 `PHASES` 逐字一致；13 个）。
#: Rust 侧单测会 `debug_assert` 词表成员，这里是消费侧的同一份白名单——新增必须两侧同步。
PHASES: tuple[str, ...] = (
    "start",
    "check",
    "check_ok",
    "up_to_date",
    "download_start",
    "download_progress",
    "download_done",
    "install_start",
    "install_ok",
    "relaunch",
    "done",
    "error",
    "abort",
)

#: 默认要求的阶段表（顺序即期望顺序）。
#: `install_ok` 放在最后：macOS / Linux 会写它；Windows 的安装器在 `install_start`
#: 之后就把进程接管并结束，通常写不到——调用方用 `--allow-missing install_ok` 显式豁免。
DEFAULT_REQUIRED: tuple[str, ...] = (
    "start",
    "check",
    "check_ok",
    "download_start",
    "download_done",
    "install_start",
    "install_ok",
)

#: 「装完了」的终态阶段（任一出现即算走完安装）。
TERMINAL_PHASES: tuple[str, ...] = ("install_ok", "done")

#: 命中这些子串的错误按 G 类（签名）归类，否则按 F（启动/流程）归类。
SIGNATURE_HINTS: tuple[str, ...] = (
    "signature",
    "minisign",
    "verify failed",
    "verification failed",
    "签名",
)

#: 允许出现、但不当作阶段序列要求的「噪声」阶段。
NOISE_PHASES: tuple[str, ...] = ("download_progress",)


class LogError(RuntimeError):
    """日志无法读取（文件不存在 / 一行都不是 JSON）。driver 应当据此记一格 fail。"""


@dataclass
class Record:
    """一行日志。字段名与 Rust 的 `SmokeRecord` 对齐（camelCase 即契约）。"""

    index: int
    seq: int
    phase: str
    pid: int
    version: str | None
    current_version: str | None
    url: str | None
    error: str | None
    detail: dict[str, Any] | None

    @property
    def is_terminal(self) -> bool:
        return self.phase in TERMINAL_PHASES


@dataclass
class Report:
    """断言结果。`ok` 为真时 `problems` 必为空。"""

    path: str
    ok: bool = False
    target: str | None = None
    expect_current: str | None = None
    first_pid: int | None = None
    process_count: int = 0
    records: int = 0
    phases: list[str] = field(default_factory=list)
    missing: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    problems: list[str] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)
    category: str | None = None

    def as_dict(self) -> dict[str, Any]:
        return {
            "path": self.path,
            "ok": self.ok,
            "target": self.target,
            "expectCurrent": self.expect_current,
            "firstPid": self.first_pid,
            "processCount": self.process_count,
            "records": self.records,
            "phases": self.phases,
            "missing": self.missing,
            "errors": self.errors,
            "problems": self.problems,
            "notes": self.notes,
            "category": self.category,
        }


# ---------------------------------------------------------------------------
# 读取 / 分组
# ---------------------------------------------------------------------------


def _as_int(value: Any, fallback: int) -> int:
    """JSON 里的整数可能是字符串（历史写法）；取不出来就用 fallback，不抛异常。"""
    if isinstance(value, bool):
        return fallback
    if isinstance(value, int):
        return value
    if isinstance(value, str):
        try:
            return int(value.strip())
        except ValueError:
            return fallback
    return fallback


def parse_records(lines: Iterable[str], path: str = "<memory>") -> tuple[list[Record], list[str]]:
    """把 JSONL 行解析成 `Record`；返回 `(记录, 问题)`。

    解析问题的处理口径：**不抛异常**（driver 要写格子而不是崩），把问题返回给调用方，
    由 [`evaluate`] 决定是 fail 还是 note。
    """
    records: list[Record] = []
    problems: list[str] = []
    for index, raw in enumerate(lines):
        text = raw.strip()
        if not text:
            continue
        try:
            data = json.loads(text)
        except (TypeError, ValueError):
            problems.append(f"第 {index + 1} 行不是合法 JSON：{text[:120]}")
            continue
        if not isinstance(data, dict):
            problems.append(f"第 {index + 1} 行不是 JSON 对象：{text[:120]}")
            continue
        phase = data.get("phase")
        if not isinstance(phase, str) or not phase:
            problems.append(f"第 {index + 1} 行缺少 phase 字段：{text[:120]}")
            continue
        detail = data.get("detail")
        records.append(
            Record(
                index=index,
                seq=_as_int(data.get("seq"), index),
                phase=phase,
                pid=_as_int(data.get("pid"), -1),
                version=(
                    common.normalize_version(data["version"])
                    if isinstance(data.get("version"), str)
                    else None
                ),
                current_version=(
                    common.normalize_version(data["currentVersion"])
                    if isinstance(data.get("currentVersion"), str)
                    else None
                ),
                url=data.get("url") if isinstance(data.get("url"), str) else None,
                error=data.get("error") if isinstance(data.get("error"), str) else None,
                detail=detail if isinstance(detail, dict) else None,
            )
        )
    if not records and not problems:
        problems.append(f"日志没有任何记录：{path}")
    return records, problems


def read_log(path: str) -> tuple[list[Record], list[str]]:
    """读日志文件；文件不存在抛 :class:`LogError`（driver 记 fail，不猜）。"""
    if not os.path.exists(path):
        raise LogError(f"日志文件不存在：{path}")
    with open(path, encoding="utf-8", errors="replace") as handle:
        return parse_records(handle, path)


def split_first_process(records: Sequence[Record]) -> tuple[int | None, list[Record], int]:
    """按 `pid` 分组，返回 `(第一个进程的 pid, 该进程按 seq 排序的记录, 进程总数)`。

    「第一个进程」= **文件里最先出现**的 pid（= CI 启动的那个旧版本进程）；
    组内按 `seq` 排序，`seq` 相同则保持文件顺序（稳定排序，Python 的 `sorted` 即稳定）。
    """
    order: list[int] = []
    groups: dict[int, list[Record]] = {}
    for record in records:
        if record.pid not in groups:
            groups[record.pid] = []
            order.append(record.pid)
        groups[record.pid].append(record)
    if not order:
        return None, [], 0
    first_pid = order[0]
    return first_pid, sorted(groups[first_pid], key=lambda item: (item.seq, item.index)), len(order)


# ---------------------------------------------------------------------------
# 断言
# ---------------------------------------------------------------------------


def _missing_hint(required: Sequence[str], missing: str, positions: dict[str, int]) -> str:
    """给「缺少阶段」一条可执行的提示：它本该出现在哪个阶段之后。"""
    index = list(required).index(missing)
    for previous in reversed(list(required)[:index]):
        if previous in positions:
            return f"（应在 `{previous}` 之后出现）"
    return "（日志在它之前就结束了）"


def _order_problems(required: Sequence[str], positions: dict[str, int]) -> list[str]:
    """期望顺序里已出现的阶段必须严格递增；返回顺序问题。"""
    present = [(phase, positions[phase]) for phase in required if phase in positions]
    problems: list[str] = []
    for (left, left_pos), (right, right_pos) in zip(present, present[1:]):
        if right_pos < left_pos:
            problems.append(f"阶段顺序错乱：`{right}` 出现在 `{left}` 之前")
    return problems


def _classify(problems: Sequence[str], errors: Sequence[str], version_problems: bool) -> str | None:
    """归类：签名类错误 → G；版本类（含 up_to_date）→ H；其余 → F。"""
    if not problems:
        return None
    joined = " ".join(errors).lower()
    if any(hint in joined for hint in SIGNATURE_HINTS):
        return "G"
    if version_problems:
        return "H"
    return "F"


def evaluate(
    records: Sequence[Record],
    parse_problems: Sequence[str] = (),
    target: str | None = None,
    expect: Sequence[str] = DEFAULT_REQUIRED,
    expect_current: str | None = None,
    allow_missing: Sequence[str] = (),
    path: str = "<memory>",
) -> Report:
    """对一组记录做全部断言；返回 :class:`Report`（不抛异常）。"""
    report = Report(path=path, target=common.normalize_version(target) if target else None)
    report.expect_current = common.normalize_version(expect_current) if expect_current else None
    report.records = len(records)
    report.problems.extend(parse_problems)

    first_pid, process, process_count = split_first_process(records)
    report.first_pid = first_pid
    report.process_count = process_count
    report.phases = [record.phase for record in process]
    if process_count > 1:
        report.notes.append(
            f"日志含 {process_count} 个进程（pid={first_pid} 是被启动的旧版本；"
            f"其余大概是安装后自动拉起的新进程，作升级后重启的 bonus 证据）"
        )

    if not process:
        report.problems.append("日志里没有任何可用记录（进程没起来，或 WB_SHIFT_SMOKE_LOG 不可写）")
        report.category = "F"
        return report

    # 1) 词表：出现词表外的 phase 说明日志契约漂移（Rust 与消费侧不同步）。
    unknown = sorted({record.phase for record in process if record.phase not in PHASES})
    if unknown:
        report.problems.append(f"出现词表外的 phase：{unknown}（词表见 smoke.rs 的 PHASES）")

    # 2) 阶段序列：缺失 / 顺序。
    positions: dict[str, int] = {}
    for index, phase in enumerate(report.phases):
        positions.setdefault(phase, index)
    waived = {str(item) for item in allow_missing}
    for phase in expect:
        if phase in positions:
            continue
        report.missing.append(phase)
        if phase in waived:
            report.notes.append(
                f"阶段 `{phase}` 未出现，但调用方已显式豁免"
                f"（{_missing_hint(expect, phase, positions)}）"
            )
        else:
            report.problems.append(f"缺少阶段 `{phase}`{_missing_hint(expect, phase, positions)}")
    report.problems.extend(_order_problems(expect, positions))

    # 3) 失败事件：error / abort 一律失败；up_to_date 归 H（旧版本认为自己是 latest）。
    version_problems = False
    for record in process:
        if record.phase == "error":
            message = record.error or "（error 事件没有 error 字段）"
            report.errors.append(message)
            report.problems.append(f"出现 error 事件：{message}")
        elif record.phase == "abort":
            message = record.error or "（abort 事件没有 error 字段）"
            report.errors.append(message)
            report.problems.append(f"出现 abort 事件（闸门/环境不可用）：{message}")
        elif record.phase == "up_to_date":
            version_problems = True
            report.problems.append(
                "出现 up_to_date 事件：被验收的旧版本认为已是最新"
                f"（latest={record.version or '未知'}），没有可下载的新版本"
            )

    # 4) 版本：带 version 的事件必须等于目标版本（start / check 恒为 null，不算缺失）。
    if report.target:
        for record in process:
            if record.version is None or record.phase == "up_to_date":
                continue
            if not common.same_version(record.version, report.target):
                version_problems = True
                report.problems.append(
                    f"阶段 `{record.phase}` 的 version={record.version} 与目标 {report.target} 不一致"
                )
    if report.expect_current:
        seen = [record.current_version for record in process if record.current_version]
        wrong = sorted({value for value in seen if not common.same_version(value, report.expect_current)})
        if wrong:
            version_problems = True
            report.problems.append(
                f"currentVersion={wrong} 与预期旧版本 {report.expect_current} 不一致"
                "（自更新不是从上一版启动的）"
            )

    # 5) 下载完成事件的 `detail.percent`：只在它存在时校验（Rust 侧写的就是 100）。
    for record in process:
        if record.phase != "download_done" or not record.detail:
            continue
        percent = record.detail.get("percent")
        if percent is not None and _as_int(percent, -1) != 100:
            report.problems.append(f"download_done 的 percent={percent}，应为 100")

    if not any(record.is_terminal for record in process):
        report.notes.append(
            "日志没有 install_ok / done 终态记录（Windows 的 NSIS 安装器会在 install_start "
            "之后接管并结束进程，属预期；版本变化由 driver 的装后再读版本断言）"
        )

    report.ok = not report.problems
    report.category = _classify(report.problems, report.errors, version_problems)
    return report


# ---------------------------------------------------------------------------
# 输出
# ---------------------------------------------------------------------------


def render_text(report: Report) -> str:
    """人读输出：问题逐条 `FAIL`，豁免与 bonus 逐条 `NOTE`。"""
    lines: list[str] = []
    for problem in report.problems:
        lines.append(f"FAIL {problem}")
    for note in report.notes:
        lines.append(f"NOTE {note}")
    if report.ok:
        lines.append(
            f"OK 阶段序列={report.phases} pid={report.first_pid} "
            f"进程数={report.process_count} 记录数={report.records}"
        )
    else:
        lines.append(
            f"FAILED {len(report.problems)} 项 · category={report.category} "
            f"pid={report.first_pid} 阶段序列={report.phases}"
        )
    return "\n".join(lines)


def write_report(path: str, report: Report) -> None:
    """原子写 JSON 报告（driver 读它拿 category，triage 读它做归类证据）。"""
    parent = os.path.dirname(os.path.abspath(path))
    if parent:
        os.makedirs(parent, exist_ok=True)
    tmp = f"{path}.tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(report.as_dict(), handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    os.replace(tmp, path)


# ---------------------------------------------------------------------------
# 自测
# ---------------------------------------------------------------------------


def _record_line(
    seq: int,
    phase: str,
    version: str | None,
    pid: int,
    current: str = "0.2.4",
    error: str | None = None,
    detail: dict[str, Any] | None = None,
    url: str | None = "https://github.com/o/r/releases/tag/v0.2.5",
) -> str:
    """造一行日志（字段与 Rust 的 `SmokeRecord` 同名同序）。"""
    return json.dumps(
        {
            "ts": "2026-02-14T03:01:00Z",
            "seq": seq,
            "phase": phase,
            "version": version,
            "currentVersion": current,
            "url": url,
            "error": error,
            "detail": detail,
            "pid": pid,
        },
        ensure_ascii=False,
        separators=(",", ":"),
    )


def _fixture_normal() -> list[str]:
    """正常：旧进程走完 start→install_start，装完自动拉起的新进程追加一条 start。"""
    lines = [
        _record_line(0, "start", None, 4242, detail={"logPath": "x.jsonl"}),
        _record_line(1, "check", None, 4242, url=None),
        _record_line(2, "check_ok", "0.2.5", 4242),
        _record_line(3, "download_start", "0.2.5", 4242),
        _record_line(4, "download_progress", "0.2.5", 4242, detail={"percent": 40}),
        _record_line(5, "download_done", "0.2.5", 4242, detail={"percent": 100, "bytes": 12345}),
        _record_line(6, "install_start", "0.2.5", 4242),
        # 新进程（NSIS 自动拉起）：pid 不同、seq 从 0 重新开始，只在最后追加一条 start。
        _record_line(0, "start", None, 9999, current="0.2.5"),
    ]
    return lines


def _fixture_missing_install() -> list[str]:
    """缺阶段：只到 download_done 就断了（下载后进程被别的路径杀掉）。"""
    return [
        _record_line(0, "start", None, 4242, detail={"logPath": "x.jsonl"}),
        _record_line(1, "check", None, 4242, url=None),
        _record_line(2, "check_ok", "0.2.5", 4242),
        _record_line(3, "download_start", "0.2.5", 4242),
        _record_line(4, "download_done", "0.2.5", 4242, detail={"percent": 100}),
    ]


def _fixture_error() -> list[str]:
    """含 error：minisign 校验失败（应归 G）。"""
    return [
        _record_line(0, "start", None, 4242, detail={"logPath": "x.jsonl"}),
        _record_line(1, "check", None, 4242, url=None),
        _record_line(2, "check_ok", "0.2.5", 4242),
        _record_line(3, "download_start", "0.2.5", 4242),
        _record_line(4, "download_done", "0.2.5", 4242, detail={"percent": 100}),
        _record_line(5, "install_start", "0.2.5", 4242),
        _record_line(6, "error", "0.2.5", 4242, error="signature verification failed"),
    ]


def selftest() -> int:
    """离线自测：3 份 JSONL（正常 / 缺阶段 / 含 error）互相对照。"""
    problems: list[str] = []
    with tempfile.TemporaryDirectory(prefix="assert-update-log.") as tmp:
        cases = {
            "normal": _fixture_normal(),
            "missing": _fixture_missing_install(),
            "error": _fixture_error(),
        }
        paths: dict[str, str] = {}
        for name, lines in cases.items():
            path = os.path.join(tmp, f"{name}.jsonl")
            with open(path, "w", encoding="utf-8", newline="\n") as handle:
                handle.write("\n".join(lines) + "\n")
            paths[name] = path

        # 1) 正常：pass，且 firstPid 是文件里最先出现的 4242（不是自动拉起的新进程）。
        records, parse_problems = read_log(paths["normal"])
        report = evaluate(
            records,
            parse_problems,
            target="0.2.5",
            expect_current="0.2.4",
            allow_missing=("install_ok", "relaunch", "done"),
            path=paths["normal"],
        )
        if not report.ok:
            problems.append(f"正常日志被误判失败：{report.problems}")
        if report.first_pid != 4242:
            problems.append(f"第一个进程应是 4242，实际 {report.first_pid}")
        if report.process_count != 2:
            problems.append(f"进程数应为 2，实际 {report.process_count}")
        if "install_start" not in report.phases:
            problems.append("正常日志的阶段序列里应有 install_start")

        # 2) 缺阶段：fail，且错误信息必须点出 install_start（而不是只说「序列不对」）。
        records, parse_problems = read_log(paths["missing"])
        report = evaluate(
            records,
            parse_problems,
            target="0.2.5",
            expect_current="0.2.4",
            path=paths["missing"],
        )
        if report.ok:
            problems.append("缺阶段的日志未被判失败")
        if "install_start" not in report.missing:
            problems.append(f"缺阶段未被指出（missing={report.missing}）")
        if not any("install_start" in item for item in report.problems):
            problems.append(f"错误信息没点出 install_start：{report.problems}")
        if report.category != "F":
            problems.append(f"缺阶段应归 F，实际 {report.category}")

        # 3) 含 error：fail，签名类错误归 G。
        records, parse_problems = read_log(paths["error"])
        report = evaluate(
            records,
            parse_problems,
            target="0.2.5",
            expect_current="0.2.4",
            allow_missing=("install_ok", "relaunch", "done"),
            path=paths["error"],
        )
        if report.ok:
            problems.append("含 error 事件的日志未被判失败")
        if report.category != "G":
            problems.append(f"签名类错误应归 G，实际 {report.category}")
        if not any("signature" in item for item in report.errors):
            problems.append(f"error 文本未进 errors：{report.errors}")

        # 4) 版本不一致：目标 0.2.6 → check_ok 的 0.2.5 应被判 H。
        records, parse_problems = read_log(paths["normal"])
        report = evaluate(
            records,
            parse_problems,
            target="0.2.6",
            allow_missing=("install_ok", "relaunch", "done"),
            path=paths["normal"],
        )
        if report.ok or report.category != "H":
            problems.append(f"目标版本不一致应归 H，实际 ok={report.ok} category={report.category}")

        # 5) 非法 JSON 行只记问题、不抛异常。
        records, parse_problems = parse_records(["not json", _record_line(0, "start", None, 1)], "x")
        if len(records) != 1 or not parse_problems:
            problems.append("非法 JSON 行未按「记问题、保记录」处理")

        # 6) 文件不存在：read_log 抛 LogError（driver 据此记 fail 而不是崩）。
        try:
            read_log(os.path.join(tmp, "nope.jsonl"))
            problems.append("缺失日志未抛 LogError")
        except LogError:
            pass

    for message in problems:
        print(f"SELFTEST-FAIL {message}")
    if problems:
        return 1
    print("self-test passed")
    return 0


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _split_list(values: Sequence[str] | None) -> list[str]:
    """`--expect-phase a,b` / 多次 `--expect-phase a` 两种写法都支持。"""
    items: list[str] = []
    for value in values or []:
        for part in str(value).split(","):
            part = part.strip()
            if part:
                items.append(part)
    return items


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="assert-update-log.py",
        description="断言 --smoke-self-update 的 JSONL 日志（阶段序列 / 版本 / 无 error）",
    )
    parser.add_argument("log", nargs="?", default=None, help="JSONL 日志路径")
    parser.add_argument("--target", default=None, help="目标版本（0.2.5，可带 v 前缀）")
    parser.add_argument(
        "--expect-phase",
        action="append",
        default=None,
        metavar="PHASE[,PHASE]",
        help=f"要求的阶段（默认 {' → '.join(DEFAULT_REQUIRED)}）",
    )
    parser.add_argument("--expect-current", default=None, help="预期的 currentVersion（旧版本号）")
    parser.add_argument(
        "--allow-missing",
        action="append",
        default=None,
        metavar="PHASE",
        help="把某阶段的缺失降级为 NOTE（例如 Windows 的 install_ok）",
    )
    parser.add_argument("--json", action="store_true", help="输出 JSON 报告（driver 用）")
    parser.add_argument("--out", default=None, help="把 JSON 报告另存一份")
    parser.add_argument("--selftest", action="store_true", help="离线自测（3 份 JSONL）")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()
    if not args.log:
        parser.error("需要日志路径（或用 --selftest）")

    expect = _split_list(args.expect_phase) or list(DEFAULT_REQUIRED)
    allow_missing = _split_list(args.allow_missing)
    unknown_expect = [phase for phase in expect if phase not in PHASES]
    if unknown_expect:
        parser.error(f"--expect-phase 含词表外阶段：{unknown_expect}（词表 {list(PHASES)}）")
    unknown_waive = [phase for phase in allow_missing if phase not in PHASES]
    if unknown_waive:
        parser.error(f"--allow-missing 含词表外阶段：{unknown_waive}")

    try:
        records, parse_problems = read_log(args.log)
    except LogError as error:
        report = Report(path=args.log, target=args.target)
        report.problems.append(str(error))
        report.category = "F"
        if args.json:
            print(json.dumps(report.as_dict(), ensure_ascii=False))
        else:
            print(render_text(report))
        if args.out:
            write_report(args.out, report)
        return 1

    report = evaluate(
        records,
        parse_problems,
        target=args.target,
        expect=tuple(expect),
        expect_current=args.expect_current,
        allow_missing=tuple(allow_missing),
        path=args.log,
    )
    if args.out:
        write_report(args.out, report)
    if args.json:
        print(json.dumps(report.as_dict(), ensure_ascii=False))
    else:
        print(render_text(report))
    return 0 if report.ok else 1


if __name__ == "__main__":
    sys.exit(main())
