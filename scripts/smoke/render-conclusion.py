#!/usr/bin/env python3
"""结论渲染 / 单格 Gate。

本脚本有两个阶段：

1. `--gate FILE...`（T01）
   每个验收 job 的最后一步：读该 job 产出的 `<form>.json`，校验契约，把格子渲染成
   Markdown 写进 job summary，并按格子状态决定本 job 的成败。
   契约校验、字段白名单、`upgradeMode` 词表全部转发给 `scripts/smoke/common.py`
   （唯一真源，见设计 §9.2）。

2. `--matrix FILE...`（T05）
   把所有形态的结果汇总成**一张矩阵**（行=形态，列=动作），供
   `update-release-note.sh` 写回 Release 正文、以及 triage 的 step summary。
   同时做**跨文件版本一致性断言**（§9.1）：`productVersion` / `apiVersion` /
   `npmVersion` / `npxPackageVersion` / `latestJsonVersion` / `preflight.version`
   必须收敛到同一个值，否则追加一个 `form=consistency`、`category=H` 的 fail 格。

Gate 的判定口径（**与设计 §2.2 的「任一非 pass → exit 1」有一处收窄，见下**）
------------------------------------------------------------------------------
| 格子状态 | Gate 行为 |
| --- | --- |
| `pass` | 通过 |
| `skipped` | 通过（不适用，例如没有上一版时的升级格） |
| `blocked` | **不通过也不失败**：exit 0 + `::warning::` + summary 里标 ⛔ |
| `fail` | exit 1 |
| 文件缺失 / JSON 非法 / 契约不符 | exit 1（流水线缺陷，不是产品缺陷） |

`blocked`（J 类，托管 runner 无桌面会话）在本项目是**预期内**的结果：它既不能算
通过（矩阵里必须留 ⛔），也不能让 job 变红（否则「环境限制」会被当成「产品故障」，
违反 PRD G3「不阻断」）。真正的失败判定在 `triage` 的矩阵与 issue 里。

用法
----
    # 单个 job 的 Gate（workflow 里的 `Gate` 步骤）
    python3 scripts/smoke/render-conclusion.py --gate artifacts/release-smoke/windows.json

    # 本地看一份结果长什么样
    python3 scripts/smoke/render-conclusion.py --gate artifacts/release-smoke/*.json --out /tmp/x.md

    # triage：全形态矩阵（写 conclusion.md，并追加到 step summary）
    python3 scripts/smoke/render-conclusion.py --matrix artifacts/release-smoke/*.json \\
        --preflight artifacts/release-smoke/preflight.json --out artifacts/release-smoke/conclusion.md

    # 自测（不读磁盘）
    python3 scripts/smoke/render-conclusion.py --selftest

退出码：0 = 无 fail；1 = 有 fail 或结果不可读；2 = 用法错误（argparse）。

`--matrix` 的退出码与 `--gate` **一致**（有 fail → 1）：triage 里它后面还有写回步骤，
所以调用方要用 `if: always()` 或 `continue-on-error`，否则「有失败就不写回结论」——
那正是最需要写回的时候。
"""

from __future__ import annotations

import argparse
import contextlib
import importlib.util
import io
import json
import os
import shutil
import sys
import tempfile
from typing import Any, Sequence

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))

#: 格子状态符号（与设计 §6.3 一致；结论区块与 job summary 用同一套）。
STATUS_MARKS: dict[str, str] = {
    "pass": "✅",
    "fail": "❌",
    "blocked": "⛔",
    "skipped": "⏭",
}

#: 视为「本 job 可以绿」的状态。
PASSING_STATUSES: frozenset[str] = frozenset({"pass", "skipped", "blocked"})


def _load_common() -> Any:
    """按路径加载同目录的 `common.py`（契约唯一真源）。"""
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


# ---------------------------------------------------------------------------
# 渲染
# ---------------------------------------------------------------------------


def mark_for(status: str) -> str:
    """状态 → 符号；未知状态用 `❓`（契约校验会先把未知状态拦掉）。"""
    return STATUS_MARKS.get(status, "❓")


def verdict_of(cells: Sequence[dict[str, Any]]) -> str:
    """一份结果的总体判定：有 fail → FAIL；否则有 blocked → BLOCKED；否则 PASS。

    `BLOCKED` 表示「全部能判的都通过了，但有格子取不到证据」，和 `FAIL` 分开，
    对应 PRD 的「⛔ 不算通过也不算失败」。

    两条 fail-closed 规则（D2 / D4，都是「静默绿通道」的堵口）：

    - **未知或非字符串状态**（例如 `status: null`）→ `FAIL`，不是「非 fail 即通过」；
    - **零格**（driver 在 `emit` 之后、写第一格之前崩了）→ `FAIL`：空结果不是「都通过了」，
      而是「什么都没验」，不能绿。
    """
    if not cells:
        return "FAIL"
    statuses: list[str] = []
    for cell in cells:
        if not isinstance(cell, dict):
            return "FAIL"
        status = cell.get("status")
        if not isinstance(status, str) or status not in STATUS_MARKS:
            return "FAIL"
        statuses.append(status)
    if "fail" in statuses:
        return "FAIL"
    if "blocked" in statuses:
        return "BLOCKED"
    return "PASS"


def render_result(data: dict[str, Any]) -> str:
    """把一份结果渲染成 Markdown 段（summary / 本地查看用；不含矩阵）。"""
    cells = data.get("cells") or []
    mode = data.get("upgradeMode") or "—"
    prev = data.get("prevVersion") or "—"
    lines = [
        f"#### {data['form']} · `{prev}` → `{data['version']}` · `{mode}` · **{verdict_of(cells)}**",
        "",
        f"- 用时 {data.get('durationMs', 0)} ms · 目标版本 `{data.get('version')}`"
        f" · 产物版本 `{data.get('productVersion') or '—'}`"
        f" · API 版本 `{data.get('apiVersion') or '—'}`",
    ]
    log_url = data.get("logUrl")
    if log_url:
        lines.append(f"- 日志：[run 页面]({log_url})")
    lines += [
        "",
        "| 动作 | 状态 | 类别 | 断言 | 说明 | 证据 |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for cell in cells:
        assertion = str(cell.get("assertion", "")).replace("|", "\\|")
        detail = str(cell.get("detail") or "").replace("|", "\\|")
        evidence = ", ".join(str(item) for item in (cell.get("evidence") or [])) or "—"
        lines.append(
            "| `{action}` | {mark} {status} | {category} | {assertion} | {detail} | {evidence} |".format(
                action=cell.get("action", "?"),
                mark=mark_for(str(cell.get("status", ""))),
                status=cell.get("status", "?"),
                category=cell.get("category") or "—",
                assertion=assertion or "—",
                detail=detail or "—",
                evidence=evidence,
            )
        )
    return "\n".join(lines) + "\n"


def render_gate_block(results: Sequence[dict[str, Any]]) -> str:
    """多份结果的汇总段：每份一行 + 总判定。"""
    verdicts = [verdict_of(result.get("cells") or []) for result in results]
    if "FAIL" in verdicts:
        overall = "FAIL"
    elif "BLOCKED" in verdicts:
        overall = "BLOCKED"
    else:
        overall = "PASS"
    lines = [f"**Gate 结论：{overall}**", ""]
    if len(results) > 1:
        lines += ["| 形态 | 结论 | 格子数 | 用时 |", "| --- | --- | ---: | ---: |"]
        for result, verdict in zip(results, verdicts):
            lines.append(
                f"| `{result['form']}` | {verdict} | {len(result.get('cells') or [])} "
                f"| {result.get('durationMs', 0)} ms |"
            )
        lines.append("")
    for result in results:
        lines.append(render_result(result))
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# 结论矩阵（T05）
# ---------------------------------------------------------------------------

#: 矩阵列：(动作, 表头)。顺序与设计 §6.3 / §7.1 一致；`consistency` 不是列
#: （它是跨文件合成出来的，放在「版本一致性交叉断言」那一行）。
MATRIX_COLUMNS: tuple[tuple[str, str], ...] = (
    ("install", "安装"),
    ("upgrade", "升级"),
    ("startup", "启动"),
    ("version", "版本"),
    ("serve", "serve"),
    ("api", "api"),
    ("pull", "pull"),
    ("health", "health"),
    ("nonroot", "nonroot"),
    ("volume", "volume"),
)

#: 聚合时的状态优先级：一格变红，整列就红（fail > blocked > pass > skipped）。
STATUS_PRECEDENCE: tuple[str, ...] = ("fail", "blocked", "pass", "skipped")

#: 跨文件一致性断言的取值来源（设计 §9.1 / PRD P1-4）。
CONSISTENCY_FIELDS: tuple[str, ...] = (
    "productVersion",
    "apiVersion",
    "npmVersion",
    "npxPackageVersion",
    "latestJsonVersion",
)


def aggregate_cells(
    cells: Sequence[Any],
    top_mode: str | None,
) -> tuple[str, list[tuple[str, str | None, list[str]]], list[str]]:
    """把同一 (形态, 动作) 的多个格子聚合成 `(状态, 分组, 类别列表)`。

    `分组` 是 `(状态, 生效口径, 该组的类别)` 的列表，**按发生顺序**，用于复刻设计 §7.1
    里 `✅ `pkg-upgrade` / ✅ `appimage-replace`` 这种「一列两条路径」的写法：

    - 生效口径 = 格子级 `upgradeMode` 优先，无则取形态顶层（R1 的口径）；
    - 每个**不同口径**一组（同一口径的多个格子合成一个符号）；
    - 未知 status 一律按 `fail`（安全网：未知不能当通过）。

    纯函数，自测直接打这里。
    """
    if not cells:
        return "skipped", [], []
    groups: list[tuple[str, str | None, list[str]]] = []
    index: dict[str | None, int] = {}
    statuses: list[str] = []
    categories: list[str] = []
    for cell in cells:
        if not isinstance(cell, dict):
            status = "fail"
            mode: str | None = None
            category: str | None = None
        else:
            raw_status = cell.get("status")
            status = raw_status if isinstance(raw_status, str) and raw_status in STATUS_MARKS else "fail"
            mode = cell.get("upgradeMode") or top_mode or None
            category = cell.get("category")
        statuses.append(status)
        if category and str(category) not in categories:
            categories.append(str(category))
        position = index.get(mode)
        if position is None:
            index[mode] = len(groups)
            groups.append((status, mode, [str(category)] if category else []))
        else:
            previous_status, previous_mode, previous_categories = groups[position]
            merged_status = next(
                item for item in STATUS_PRECEDENCE if item in (previous_status, status)
            )
            merged_categories = list(previous_categories)
            if category and str(category) not in merged_categories:
                merged_categories.append(str(category))
            groups[position] = (merged_status, previous_mode, merged_categories)
    overall = next(item for item in STATUS_PRECEDENCE if item in statuses)
    return overall, groups, categories


def render_matrix_cell(
    cells: Sequence[Any],
    top_mode: str | None,
    transition: str = "",
) -> str:
    """一格的 Markdown 文本：`✅` / `❌ H` / `⛔` / `⏭`，多口径用 ` / ` 分隔。"""
    if not cells:
        return "—"
    overall, groups, _categories = aggregate_cells(cells, top_mode)
    parts: list[str] = []
    for status, mode, categories in groups:
        piece = mark_for(status)
        if status == "fail" and categories:
            piece += " " + "/".join(categories)
        # 行首已经写了形态主线，这里只标**覆盖值**（同口径不重复）。
        if mode and mode != top_mode:
            piece += f" `{mode}`"
        parts.append(piece)
    text = " / ".join(parts)
    # 只有单一分组时才补「0.2.4→0.2.5」：多分组时再加会把格子撑得太宽。
    if transition and len(parts) == 1:
        text += f" {transition}"
    return text


def consistency_cells(
    results: Sequence[dict[str, Any]],
    preflight: dict[str, Any] | None = None,
) -> list[dict[str, Any]]:
    """跨文件版本一致性断言：所有来源必须收敛到同一个值（设计 §9.1）。

    返回**合成出来的格子**（`form=consistency`），由调用方拼进 verdict：

    - 取到 ≥2 个不同值 → `fail` + `H`（版本不一致，PRD P1-4）；
    - 只有一个值 → `pass`；
    - 一个值都没有（全 null）→ `skipped`（没有可比的东西，不是失败）。
    """
    values: dict[str, str] = {}
    for result in results:
        for field in CONSISTENCY_FIELDS:
            value = result.get(field)
            if isinstance(value, str) and value.strip():
                values[f"{result.get('form')}.{field}"] = common.normalize_version(value)
    if isinstance(preflight, dict):
        tag_value = preflight.get("version") or common.normalize_version(preflight.get("tag"))
        if isinstance(tag_value, str) and tag_value.strip():
            values["preflight.version"] = common.normalize_version(tag_value)
    distinct = sorted(set(values.values()))
    evidence = [f"{source}={value}" for source, value in sorted(values.items())]
    if len(distinct) > 1:
        return [
            common.make_cell(
                "consistency",
                "fail",
                "所有产物版本一致（tag / productVersion / apiVersion / npm / latest.json）",
                category="H",
                detail="出现了 " + str(len(distinct)) + " 个不同版本：" + ", ".join(distinct),
                evidence=evidence,
            )
        ]
    if len(distinct) == 1:
        return [
            common.make_cell(
                "consistency",
                "pass",
                f"所有产物版本一致 == {distinct[0]}",
                detail=", ".join(evidence),
            )
        ]
    return [
        common.make_cell(
            "consistency",
            "skipped",
            "所有产物版本一致",
            detail="结果里没有可比的版本字段（全部为空）",
        )
    ]


def render_matrix(
    results: Sequence[dict[str, Any]],
    preflight: dict[str, Any] | None = None,
    tag: str = "",
    run_url: str | None = None,
    version: str | None = None,
    problems: Sequence[str] = (),
) -> str:
    """整篇矩阵 Markdown（写回 Release 正文 / step summary）。

    **首行恒为 `**结论：PASS|FAIL|BLOCKED**`**（PRD P0-9）：`blocked` 只能是 BLOCKED，
    绝不能因为「除了取不到证据的格以外都过了」就渲染成 PASS。
    """
    consistency = consistency_cells(results, preflight)
    all_cells: list[Any] = [cell for result in results for cell in (result.get("cells") or [])]
    all_cells.extend(consistency)
    verdict = verdict_of(all_cells)

    by_form: dict[str, dict[str, Any]] = {}
    for result in results:
        form = str(result.get("form") or "?")
        if form == "consistency":
            continue
        by_form[form] = result
    # 「形态整个没产出」= 没验，不能因为「剩下的都过了」就渲染 PASS（D4 的同一条纪律）。
    missing_forms = [
        form for form in common.FORMS if form != "consistency" and form not in by_form
    ]
    if not results or missing_forms or problems:
        verdict = "FAIL"

    resolved_tag = tag or (str(preflight.get("tag")) if isinstance(preflight, dict) and preflight.get("tag") else "")
    resolved_version = version or (
        str(preflight.get("version")) if isinstance(preflight, dict) and preflight.get("version") else ""
    )
    if not resolved_tag and results:
        resolved_version = resolved_version or str(results[0].get("version") or "")

    lines = [f"**结论：{verdict}**", ""]

    meta: list[str] = ["release-smoke"]
    if resolved_tag:
        meta.append(f"`{resolved_tag}`")
    elif resolved_version:
        meta.append(f"`{resolved_version}`")
    started = min((str(result.get("startedAt") or "") for result in results), default="")
    if started:
        meta.append(f"开始 {started}")
    if run_url:
        meta.append(f"[run]({run_url})")
    lines.append(" · ".join(meta))
    lines.append("")

    header = "| 形态 | " + " | ".join(label for _action, label in MATRIX_COLUMNS) + " |"
    separator = "| --- |" + " --- |" * len(MATRIX_COLUMNS)
    lines += [header, separator]

    for form in common.FORMS:
        if form == "consistency":
            continue
        result = by_form.get(form)
        if result is None:
            # 形态整个缺失：写「缺」而不是留空——留空会让人以为「不需要验」。
            cells = ["❌ 无结果"] + ["—"] * (len(MATRIX_COLUMNS) - 1)
            lines.append(f"| `{form}` ⚠️ | " + " | ".join(cells) + " |")
            continue
        top_mode = result.get("upgradeMode")
        prev = common.normalize_version(result.get("prevVersion")) or "—"
        current = common.normalize_version(result.get("version")) or "—"
        transition = f"{prev}→{current}" if prev != "—" else ""
        label = f"`{form}`"
        if top_mode:
            label += f" `{top_mode}`"
        rendered: list[str] = []
        for action, _label in MATRIX_COLUMNS:
            cells = [
                cell
                for cell in (result.get("cells") or [])
                if isinstance(cell, dict) and cell.get("action") == action
            ]
            text = render_matrix_cell(
                cells, top_mode, transition if action == "upgrade" else ""
            )
            rendered.append(text.replace("|", "\\|"))
        lines.append(f"| {label} | " + " | ".join(rendered) + " |")

    # 「不一致 / 无法判定」：⛔ 的原因必须写出来，否则读者只能去翻日志。
    notes: list[str] = []
    for result in results:
        for cell in result.get("cells") or []:
            if not isinstance(cell, dict) or cell.get("status") != "blocked":
                continue
            reason = str(cell.get("detail") or cell.get("assertion") or "").strip()
            notes.append(
                f"- `{result.get('form')}/{cell.get('action')}` ⛔ {cell.get('category') or 'J'}"
                f"：{reason or '（无 detail）'}"
            )
    for cell in consistency:
        if cell.get("status") == "fail":
            notes.append(f"- `consistency` ❌ H：{cell.get('detail')}")
    for problem in problems:
        notes.append(f"- 结果文件不可读（流水线缺陷，不是产品缺陷）：{problem}")

    if notes:
        lines += ["", "**不一致 / 无法判定**：", ""] + notes

    # 版本一致性交叉断言（设计 §7.1 的最后一行）。
    if consistency:
        cell = consistency[0]
        status = cell.get("status")
        if status == "pass":
            lines += ["", f"**版本一致性交叉断言**：{cell.get('detail')} ✅"]
        elif status == "fail":
            lines += ["", f"**版本一致性交叉断言**：{cell.get('detail')} ❌"]
        else:
            lines += ["", "**版本一致性交叉断言**：无可比字段（全部为空）➖"]

    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# Gate
# ---------------------------------------------------------------------------


def load_results(paths: Sequence[str]) -> tuple[list[dict[str, Any]], list[str]]:
    """读并校验结果；返回 `(合法的结果列表, 问题列表)`。

    单个文件读不了**不能中断同批其它文件**（D1）：`--gate 好.json 坏.json` 时好文件仍要渲染，
    坏文件单独报 `::error::` 并让本 job 红。因此这里按文件隔离异常，逐个 continue。
    """
    results: list[dict[str, Any]] = []
    problems: list[str] = []
    for path in paths:
        if not os.path.exists(path):
            problems.append(f"{path}: 结果文件不存在（驱动脚本没落盘 = 流水线缺陷）")
            continue
        try:
            results.append(common.read_result(path))
        except common.SmokeContractError as error:
            problems.append(f"{error}")
        except OSError as error:
            problems.append(f"{path}: 读取失败 {error}")
        except Exception as error:  # noqa: BLE001 - 任何解析/校验异常都要落成 problem，不能冒泡
            problems.append(f"{path}: 无法解析结果（{type(error).__name__}）：{error}")
    return results, problems


def empty_cells_problems(results: Sequence[dict[str, Any]]) -> list[str]:
    """零格结果不是「都通过了」（D4）：driver 崩在 `emit` 与第一格之间时，矩阵会缺口。"""
    problems: list[str] = []
    for result in results:
        if not (result.get("cells") or []):
            problems.append(
                f"{result.get('form')}: 结果文件没有任何断言格"
                "（driver 在 emit 之后、写第一格之前崩了？矩阵会缺格）"
            )
    return problems


def conclusion_lines(markdown: str) -> list[str]:
    """从渲染结果里挑出结论行（`**Gate 结论：…**`）。

    本地跑（没有 `$GITHUB_STEP_SUMMARY`）时结论只能靠 stdout：过去 `--gate` 既不打印
    结论、`--out` 又在「没有可读结果」时什么都不写，跑完像「什么都没发生」（DEF-8）。
    """
    return [line for line in markdown.splitlines() if line.startswith("**Gate 结论")]


def write_markdown(path: str, markdown: str) -> None:
    """把 Markdown 原子写到 `path`（先 `.tmp` 再 replace；换行固定 LF）。"""
    parent = os.path.dirname(os.path.abspath(path))
    if parent:
        os.makedirs(parent, exist_ok=True)
    tmp = f"{path}.tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(markdown)
    os.replace(tmp, path)


def gate(paths: Sequence[str], out_path: str | None = None) -> int:
    """Gate 主流程：校验 + 渲染 + 判定。返回进程退出码。

    step summary 与 `--out` 写的是**同一份** Markdown（含结论行）：过去 `--out` 只写
    `render_gate_block(results)`——`results` 为空时连文件都不写，而「结果不可读」的结论
    只进了 `$GITHUB_STEP_SUMMARY`，导出文件里看不到结论（DEF-8）。
    """
    results, problems = load_results(paths)
    problems.extend(empty_cells_problems(results))
    blocks: list[str] = [render_gate_block(results)] if results else []
    for result in results:
        blocked = [cell for cell in result.get("cells") or [] if cell.get("status") == "blocked"]
        if blocked:
            for cell in blocked:
                print(
                    f"::warning::[{result['form']}/{cell.get('action')}] "
                    f"BLOCKED（{cell.get('category') or 'J'}）：{cell.get('detail') or cell.get('assertion')}"
                )
    if problems:
        for problem in problems:
            print(f"ERROR {problem}", file=sys.stderr)
            print(f"::error::{problem}")
        blocks.append("**Gate 结论：FAIL（结果不可读）**\n\n" + "\n".join(f"- {item}" for item in problems) + "\n")

    failed = [
        (result["form"], cell)
        for result in results
        for cell in (result.get("cells") or [])
        if cell.get("status") == "fail"
    ]
    if failed:
        for form, cell in failed:
            print(
                f"::error::[{form}/{cell.get('action')}] {cell.get('category') or '?'}："
                f"{cell.get('assertion')}"
            )

    markdown = "\n".join(blocks)
    if out_path:
        write_markdown(out_path, markdown)
        print(f"[smoke] Gate 结论已导出 {out_path}")
    if markdown:
        if not common.append_step_summary(markdown):
            # 不在 CI 里（没有 $GITHUB_STEP_SUMMARY）：结论打到 stdout，别让它只活在内存里。
            for line in conclusion_lines(markdown):
                print(line)
    return 1 if (failed or problems) else 0


# ---------------------------------------------------------------------------
# 自测
# ---------------------------------------------------------------------------


def matrix(
    paths: Sequence[str],
    preflight_path: str | None,
    tag: str,
    run_url: str | None,
    out_path: str | None,
    no_summary: bool,
) -> int:
    """`--matrix` 的主流程：读结果 → 渲染 → 写文件 / step summary → 返回退出码。

    退出码与 `--gate` 一致（有 fail → 1）。triage 里这一步后面还有「写回 Release」，
    所以 workflow 用 `continue-on-error` / `if: always()` 保证「失败了也要写回」。
    """
    results, problems = load_results(paths)
    preflight: dict[str, Any] | None = None
    if preflight_path:
        if not os.path.exists(preflight_path):
            problems.append(f"{preflight_path}: preflight.json 不存在（结论里会缺 tag 与上一版）")
        else:
            try:
                with open(preflight_path, encoding="utf-8") as handle:
                    loaded = json.load(handle)
                preflight = loaded if isinstance(loaded, dict) else None
                if preflight is None:
                    problems.append(f"{preflight_path}: 不是 JSON 对象")
            except (OSError, ValueError) as error:
                problems.append(f"{preflight_path}: 读取失败 {error}")

    text = render_matrix(
        results,
        preflight=preflight,
        tag=tag,
        run_url=run_url or common.log_url_from_env(),
        problems=problems,
    )
    if out_path:
        os.makedirs(os.path.dirname(os.path.abspath(out_path)) or ".", exist_ok=True)
        with open(out_path, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
        print(f"[smoke] 结论矩阵已写入 {out_path}")
    if not no_summary:
        common.append_step_summary(text)

    for problem in problems:
        print(f"ERROR {problem}", file=sys.stderr)
        print(f"::error::{problem}")
    failed = [
        cell
        for result in results
        for cell in (result.get("cells") or [])
        if isinstance(cell, dict) and cell.get("status") == "fail"
    ]
    for cell in failed:
        print(f"::error::[{cell.get('action')}] {cell.get('category') or '?'}：{cell.get('assertion')}")
    if failed or problems:
        return 1
    # 全 blocked / 缺形态也要红：`--matrix` 是「结论」，不是「某个 job 的 Gate」。
    if text.startswith("**结论：PASS**"):
        return 0
    return 1


def selftest() -> int:
    """自测：判定口径、符号表、渲染不炸、坏数据不放行（D1/D2/D4 的回归锁）。"""
    problems: list[str] = []

    if verdict_of([{"status": "pass"}, {"status": "skipped"}]) != "PASS":
        problems.append("全 pass/skipped 应判 PASS")
    if verdict_of([{"status": "pass"}, {"status": "blocked"}]) != "BLOCKED":
        problems.append("有 blocked 应判 BLOCKED")
    if verdict_of([{"status": "blocked"}, {"status": "fail"}]) != "FAIL":
        problems.append("有 fail 应优先判 FAIL")
    # D4：零格不是「都通过了」，结论必须是 FAIL（否则 driver 崩在 emit 后会让 job 绿）。
    if verdict_of([]) != "FAIL":
        problems.append("零格应判 FAIL（D4：空结果不能当通过）")
    # D2：未知 / 非字符串状态必须 FAIL（过去会被 `str()` 成 "None" 然后判 PASS）。
    for bad_status in (None, 123, "passs", ["pass"]):
        if verdict_of([{"status": bad_status}]) != "FAIL":
            problems.append(f"未知状态 {bad_status!r} 应判 FAIL（D2）")
    if verdict_of([{"action": "install"}]) != "FAIL":
        problems.append("缺 status 的格子应判 FAIL")
    if set(STATUS_MARKS) != set(common.STATUSES):
        problems.append("符号表与 common.STATUSES 不一致")
    if set(STATUS_MARKS) - PASSING_STATUSES != {"fail"}:
        problems.append("PASSING_STATUSES 必须恰好排除 fail")

    sample = common.new_result("windows", "0.2.5", "0.2.4", "in-app")
    sample["cells"].append(common.make_cell("install", "pass", "registry DisplayVersion == 0.2.5"))
    sample["cells"].append(
        common.make_cell("startup", "blocked", "20s 内存活", detail="SessionId=0")
    )
    markdown = render_result(sample)
    for needle in ("windows", "0.2.5", "BLOCKED", "⛔", "install"):
        if needle not in markdown:
            problems.append(f"渲染缺少 {needle}")
    if "|" not in markdown.splitlines()[0] and "####" not in markdown.splitlines()[0]:
        problems.append("渲染首行既不是标题也不是表格")

    # ---- T05：矩阵 ----------------------------------------------------------
    def _form(form: str, mode: str | None, cells: list[dict[str, Any]]) -> dict[str, Any]:
        result = common.new_result(form, "0.2.5", "0.2.4", mode)
        result["cells"] = list(cells)
        result["productVersion"] = "0.2.5"
        return result

    mixed = [
        _form("windows", "in-app", [common.make_cell("install", "pass", "双读一致")]),
        _form(
            "linux",
            "pkg-upgrade",
            [
                common.make_cell("install", "pass", "dpkg -i"),
                # 格子级覆盖：AppImage 那条路径（R1）
                common.make_cell("upgrade", "pass", "同路径替换", upgrade_mode="appimage-replace"),
                common.make_cell("startup", "fail", "xvfb 20s 存活", category="F"),
            ],
        ),
        _form("macos", "pkg-upgrade", [common.make_cell("install", "skipped", "x86_64 非目标")]),
        _form("npm", "npm-upgrade", [common.make_cell("install", "pass", "npm i -g")]),
        _form("docker", None, [common.make_cell("pull", "blocked", "GHCR 探测", detail="镜像未推完")]),
    ]
    matrix_text = render_matrix(mixed, tag="v0.2.5", run_url="https://example/run/1")
    first_line = matrix_text.splitlines()[0]
    if first_line != "**结论：FAIL**":
        problems.append(f"矩阵首行应为 `**结论：FAIL**`，实际 {first_line!r}")
    for needle in ("| 形态 | 安装 | 升级 | 启动 |", "`linux` `pkg-upgrade`", "appimage-replace", "❌ F", "⛔", "⏭"):
        if needle not in matrix_text:
            problems.append(f"矩阵缺少 {needle}")
    # 行首是主线（pkg-upgrade），AppImage 那条路径的覆盖值出现在格内。
    linux_line = next(line for line in matrix_text.splitlines() if line.startswith("| `linux`"))
    if "appimage-replace" not in linux_line:
        problems.append("Linux 行的升级格没有展示格子级覆盖值")
    if "`pkg-upgrade`" not in linux_line.split("|")[1]:
        problems.append("Linux 行的行首没有展示顶层口径")

    # 全 blocked → 必须渲染 BLOCKED（不是 PASS）。
    all_blocked = [
        _form(form, None, [common.make_cell("startup", "blocked", "取不到证据")])
        for form in ("windows", "macos", "linux", "npm", "docker")
    ]
    blocked_text = render_matrix(all_blocked, tag="v0.2.5")
    if blocked_text.splitlines()[0] != "**结论：BLOCKED**":
        problems.append(
            f"全 blocked 必须渲染 BLOCKED，实际 {blocked_text.splitlines()[0]!r}"
        )

    # 全 pass → PASS；缺一个形态 → FAIL（不能因为「剩下的都过了」就绿）。
    all_pass = [
        _form(form, None, [common.make_cell("install", "pass", "ok")])
        for form in ("windows", "macos", "linux", "npm", "docker")
    ]
    pass_text = render_matrix(all_pass, tag="v0.2.5")
    if pass_text.splitlines()[0] != "**结论：PASS**":
        problems.append(f"全 pass 应渲染 PASS，实际 {pass_text.splitlines()[0]!r}")
    missing_text = render_matrix(all_pass[:-1], tag="v0.2.5")
    if missing_text.splitlines()[0] != "**结论：FAIL**":
        problems.append("缺形态时应判 FAIL")
    if "⚠️" not in missing_text:
        problems.append("缺形态的行没有标记 ⚠️")
    if render_matrix([], tag="v0.2.5").splitlines()[0] != "**结论：FAIL**":
        problems.append("零结果时矩阵应判 FAIL（D4）")

    # 跨文件版本一致性：两个不同 productVersion → fail/H。
    drifted = [
        _form("windows", "in-app", [common.make_cell("install", "pass", "ok")]),
        _form("macos", "pkg-upgrade", [common.make_cell("install", "pass", "ok")]),
    ]
    drifted[1]["productVersion"] = "0.2.4"
    drifted_text = render_matrix(drifted, tag="v0.2.5")
    if "consistency" not in drifted_text or "H" not in drifted_text:
        problems.append("版本漂移未追加 consistency/H 断言")
    cells = consistency_cells(drifted, {"version": "0.2.5", "tag": "v0.2.5"})
    if len(cells) != 1 or cells[0]["status"] != "fail" or cells[0]["category"] != "H":
        problems.append(f"consistency_cells 对漂移的判定不对：{cells}")
    same = consistency_cells(
        [_form("windows", "in-app", [common.make_cell("install", "pass", "ok")])],
        {"version": "0.2.5", "tag": "v0.2.5"},
    )
    if len(same) != 1 or same[0]["status"] != "pass":
        problems.append(f"版本一致时应 pass：{same}")
    bare = common.new_result("windows", "0.2.5")
    bare["cells"] = [common.make_cell("install", "pass", "ok")]
    if consistency_cells([bare], None)[0]["status"] != "skipped":
        problems.append("没有可比值时应 skipped")

    # D1：同批「好文件 + 坏文件」——坏文件记 problem，好文件必须仍然渲染出来。
    tmp = tempfile.mkdtemp(prefix="render-selftest.")
    try:
        good_path = os.path.join(tmp, "good.json")
        broken_path = os.path.join(tmp, "broken.json")
        empty_path = os.path.join(tmp, "empty.json")
        common.write_result(good_path, sample)
        with open(broken_path, "w", encoding="utf-8") as handle:
            handle.write('{"form":"windows", "cells":[')
        common.write_result(empty_path, common.new_result("npm", "0.2.5"))
        results, read_problems = load_results([good_path, broken_path])
        if len(results) != 1:
            problems.append(f"好文件应被读到（实际 {len(results)} 个），坏文件不该中断同批渲染")
        if not any("JSON 解析失败" in item for item in read_problems):
            problems.append(f"损坏 JSON 未记 problem：{read_problems}")
        if not render_gate_block(results):
            problems.append("同批有坏文件时，好文件的渲染块为空")
        empty_loaded, empty_read_problems = load_results([empty_path])
        empty_cell_problems = empty_cells_problems(empty_loaded)
        if empty_read_problems:
            problems.append(f"零格结果应当能读出（交 empty_cells_problems 判失败）：{empty_read_problems}")
        if not empty_cell_problems:
            problems.append("零格结果未记 problem（D4）")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    # DEF-8：`--gate --out` 的导出文件里必须有结论行；没有 $GITHUB_STEP_SUMMARY 时 stdout 也要有。
    tmp_gate = tempfile.mkdtemp(prefix="gate-selftest.")
    try:
        good_gate = os.path.join(tmp_gate, "windows.json")
        all_pass_gate = common.new_result("windows", "0.2.5", "0.2.4", "in-app")
        all_pass_gate["cells"] = [common.make_cell("install", "pass", "registry DisplayVersion == 0.2.5")]
        common.write_result(good_gate, all_pass_gate)
        broken_gate = os.path.join(tmp_gate, "broken.json")
        with open(broken_gate, "w", encoding="utf-8") as handle:
            handle.write('{"form":"windows", "cells":[')
        previous_summary = os.environ.pop("GITHUB_STEP_SUMMARY", None)
        try:
            bad_out = os.path.join(tmp_gate, "gate-bad.md")
            bad_stdout = io.StringIO()
            # stderr 也收走：坏文件本来就会打 `::error::`，自测输出里不该混进这行噪声。
            with contextlib.redirect_stdout(bad_stdout), contextlib.redirect_stderr(io.StringIO()):
                bad_code = gate([broken_gate], bad_out)
            good_out = os.path.join(tmp_gate, "gate-good.md")
            good_stdout = io.StringIO()
            with contextlib.redirect_stdout(good_stdout), contextlib.redirect_stderr(io.StringIO()):
                good_code = gate([good_gate], good_out)
        finally:
            if previous_summary is not None:
                os.environ["GITHUB_STEP_SUMMARY"] = previous_summary
        exported_bad = ""
        if os.path.exists(bad_out):
            with open(bad_out, encoding="utf-8") as handle:
                exported_bad = handle.read()
        exported_good = ""
        if os.path.exists(good_out):
            with open(good_out, encoding="utf-8") as handle:
                exported_good = handle.read()
        if bad_code != 1 or good_code != 0:
            problems.append(f"Gate 退出码不对（坏 {bad_code} / 好 {good_code}）")
        if "**Gate 结论：FAIL（结果不可读）**" not in exported_bad:
            problems.append(f"--gate --out 的导出文件缺结论行（DEF-8）：{exported_bad[:80]!r}")
        if "**Gate 结论：PASS**" not in exported_good:
            problems.append(f"--gate --out 的导出文件缺 PASS 结论行（DEF-8）：{exported_good[:80]!r}")
        if "**Gate 结论：FAIL（结果不可读）**" not in bad_stdout.getvalue():
            problems.append(f"无 $GITHUB_STEP_SUMMARY 时 stdout 缺结论行（DEF-8）：{bad_stdout.getvalue()!r}")
        if "**Gate 结论：PASS**" not in good_stdout.getvalue():
            problems.append(f"无 $GITHUB_STEP_SUMMARY 时 stdout 缺 PASS 结论行：{good_stdout.getvalue()!r}")
        if conclusion_lines("**Gate 结论：PASS**\n\n| a |") != ["**Gate 结论：PASS**"]:
            problems.append("conclusion_lines 未正确挑出结论行")
    finally:
        shutil.rmtree(tmp_gate, ignore_errors=True)

    for message in problems:
        print(f"SELFTEST-FAIL {message}")
    if problems:
        return 1
    print("self-test passed")
    return 0


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="render-conclusion.py",
        description="release-smoke 的 Gate（--gate）与结论矩阵（--matrix）",
    )
    parser.add_argument(
        "--gate",
        nargs="+",
        metavar="RESULT_JSON",
        default=None,
        help="校验并判定的结果文件（可多个）；任一格 fail 即 exit 1",
    )
    parser.add_argument(
        "--matrix",
        nargs="+",
        metavar="RESULT_JSON",
        default=None,
        help="渲染全形态矩阵（结论首行 + 跨文件一致性断言）；任一格 fail 即 exit 1",
    )
    parser.add_argument("--preflight", default=None, help="preflight.json（--matrix 用，提供 tag/version）")
    parser.add_argument("--tag", default=os.environ.get("SMOKE_VERSION", ""), help="tag（默认读 SMOKE_VERSION）")
    parser.add_argument("--run-url", default=None, help="本次 run 的链接（默认由 GITHUB_* 拼）")
    parser.add_argument("--no-summary", action="store_true", help="不追加到 $GITHUB_STEP_SUMMARY")
    parser.add_argument("--out", default=None, help="把渲染结果另存一份到文件（可选）")
    parser.add_argument("--selftest", action="store_true", help="离线自测")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()
    if not args.gate and not args.matrix:
        parser.error("要么 --gate，要么 --matrix")
    if args.gate and args.matrix:
        parser.error("--gate 与 --matrix 不能同时用（判定口径不同）")

    if args.matrix:
        return matrix(args.matrix, args.preflight, args.tag, args.run_url, args.out, args.no_summary)

    # `--out` 交给 gate()：它才知道「有多少结果不可读」，也只有它能把结论行一起写进文件（DEF-8）。
    return gate(args.gate, args.out)


if __name__ == "__main__":
    sys.exit(main())
