#!/usr/bin/env python3
r"""把 `artifacts/release-smoke/*.json` 归类成**处置计划**（设计 §8.1–§8.3）。

本文件只做「读结果 → 出计划」，**不执行任何 gh 命令**（执行在 `dispose.py`，
写回在 `update-release-note.sh`，开 issue 在 `issue.sh`）。这样「归类错了」永远不会
造成副作用，可以先 `dry_run` 看计划。

输入 / 输出
-----------
- 输入：`--results`（目录或 glob，合并后的 `artifacts/release-smoke/`）+ 可选 `--preflight`；
- 输出：`--out`（默认 `artifacts/release-smoke/triage-plan.json`）供 `dispose.py` /
  `render-conclusion.py --matrix` / `issue.sh` 共用，同时打印一张人读表并写 step summary。

计划项字段（**`action` 是处置，`cell` 是矩阵列**——两者不能混用）

| 字段 | 说明 |
|---|---|
| `form` | 形态（`windows` / `macos` / `linux` / `npm` / `docker` / `consistency`） |
| `cell` | 矩阵里的动作列（`install` / `upgrade` / `startup` / …） |
| `status` | 该格的原始状态（`fail` / `blocked`） |
| `category` | A–J；**`null` 表示「无法归类」**（fail-closed，直接升级为 issue） |
| `severity` | `high`（G 签名类）/ `normal` / `info`（J 无法判定） |
| `action` | 处置：`rerun-job` \| `dispatch-docker` \| `rerun-release` \| `issue` \| `none` |
| `reason` | 一句话理由（进结论区块与 issue 正文） |
| `assertion` / `detail` / `evidence` | 从格子原样带过来，issue 正文要逐字引用 |

归类规则（§8.1 的机读版）
------------------------
| 类别 | 判据 | 处置 |
|---|---|---|
| A 瞬时失败 | 网络关键词（`timeout` / `ECONNRESET` / `5xx` / `429` / `rate limit` …） | `rerun-job` |
| B npm 平台包缺失 | npm 形态的 404 / 版本不齐 | `rerun-release` |
| C GHCR tag 缺失 | `manifest unknown` / `docker pull` 404 | `dispatch-docker` |
| D Release 资产缺失 | `digest` / `sha256` / 资产名 / `gh release download` 失败 | `rerun-release` |
| E 安装失败 | 安装器退出码 ≠ 0 / 装完主程序缺失 | `issue` |
| F 启动即崩 | 20s 内退出且非 J / `error.log` 有 crash 行 | `issue` |
| G 签名不匹配 | minisign / `.sig` 校验失败 | `issue`（`severity=high`） |
| H 版本不一致 | tag / plist / dpkg / package.json / latest.json / `/api/status` 任一不等 | `issue` |
| I 容器不可用 | unhealthy / `uid != 1000` / 卷不可写 | `issue` |
| J 无法判定 | `blocked` 格（§6.2 三条件） | `none`（⛔，**不开 issue**） |

三条**兜底**（都比「猜一个类别」更安全）：

1. 驱动脚本已经写了合法 `category` → **以它为准**，不再看日志（司机比乘客清楚）；
2. 没写 `category`，但文本能命中关键词 → 按上表推断，并在 `reason` 里注明「推断」；
3. 什么都命中不了 → `category=null` + `action=issue`（**fail-closed**）：宁可多开一张
   issue 让人看一眼，也不能让一个真实的失败因为「归类不出来」被静默吞掉。

另外两种「没有格子」的情况也会进计划（§8.3 的边界 case）：

- **整个形态没有结果文件**（驱动 job 崩了 / artifact 没上传）→ A，`rerun-job` 一次；
- **结果文件零格**（driver 在 `emit` 之后、写第一格之前崩了）→ A，`rerun-job` 一次
  （设计 §6.3 的 D4：零格不是「都通过了」）。

「结果文件本身有问题」分**两类**，处置不同（DEF-4，见 :func:`read_results`）：

| 现象 | 归到 | 处置 | 为什么 |
|---|---|---|---|
| 文件不存在 / 打不开 | A | `rerun-job` | 驱动 job 崩了或没跑，重跑有意义 |
| 文件存在但契约非法（缺 `category` / 类型错 / JSON 坏） | `null` | `issue` + `severity=high` | 流水线缺陷：重跑同一份脚本只会再产出一份坏文件 |

过去两者都归 A：契约违反被当成「环境抖动」，`category=null + issue` 那条分支永远走不到。

用法
----
    # CI（triage 第一步）
    python3 scripts/smoke/classify.py --results artifacts/release-smoke \\
        --tag v0.2.5 --run-id "$GITHUB_RUN_ID" --run-attempt "$GITHUB_RUN_ATTEMPT" \\
        --out artifacts/release-smoke/triage-plan.json

    # 离线自测
    python3 scripts/smoke/classify.py --selftest

退出码：0 = 计划已写出（**计划里有失败也是 0**：判定在 Gate，triage 负责写回）；
1 = 输入不可读（目录不存在）；2 = 用法错误（argparse）。
"""

from __future__ import annotations

import argparse
import glob
import importlib.util
import json
import os
import shutil
import sys
import tempfile
from typing import Any, Iterable, Sequence

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))


def _load_common() -> Any:
    """按路径加载 `common.py`（契约唯一真源：白名单 / 排序 / 归一化都从它来）。"""
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

SCHEMA_VERSION = 1

#: 处置动作白名单。`dispose.py` 只认这五个，多一个都不执行。
DISPOSITIONS: tuple[str, ...] = (
    "rerun-job",
    "dispatch-docker",
    "rerun-release",
    "issue",
    "none",
)

#: 严重级别。`high` 给 G（签名）与「结果文件契约非法」（流水线缺陷）：
#: 前者要带 `[security]` 前缀，后者说明生成结果的脚本自己坏了，两者人都要立刻看。
SEVERITIES: tuple[str, ...] = ("high", "normal", "info")

#: 读结果时的两类问题（DEF-4）。**必须分开**，因为处置完全不同：
#:
#: - :data:`READ_PROBLEM_UNREADABLE`：文件不存在 / 打不开 —— 驱动 job 崩了或没跑，
#:   重跑一次有意义（A / `rerun-job`）；
#: - :data:`READ_PROBLEM_INVALID`：文件读到了但**契约非法**（缺 `category`、类型错、
#:   JSON 坏）—— 这是流水线的缺陷，重跑同一份脚本只会再产出一份坏文件，所以
#:   `category=null` + `action=issue` + `severity=high`，交给人改脚本。
#:
#: 过去两者都归 A/`rerun-job`：contract 违反了还被当「环境抖动」，`category=null + issue`
#: 这条分支永远走不到，等于把「流水线缺陷」伪装成「重跑就好」。
READ_PROBLEM_UNREADABLE = "unreadable"
READ_PROBLEM_INVALID = "invalid"

#: 类别 → 处置（§8.1）。J 单列（只记 ⛔，不开 issue）。
CATEGORY_ACTIONS: dict[str, str] = {
    "A": "rerun-job",
    "B": "rerun-release",
    "C": "dispatch-docker",
    "D": "rerun-release",
    "E": "issue",
    "F": "issue",
    "G": "issue",
    "H": "issue",
    "I": "issue",
    "J": "none",
}

#: 网络类关键词（A）：**只用于**驱动脚本没给 category 时的兜底推断。
NETWORK_KEYWORDS: tuple[str, ...] = (
    "timeout",
    "timed out",
    "etimedout",
    "getaddrinfo",
    "econnreset",
    "econnrefused",
    "eai_again",
    "socket hang up",
    "rate limit",
    "too many requests",
    " 429",
    " 500",
    " 502",
    " 503",
    " 504",
    "network is unreachable",
    "temporary failure in name resolution",
)

#: 兜底推断表：**顺序即优先级**（越具体的越靠前，避免 "version" 吃掉 "signature"）。
INFER_RULES: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("G", ("minisign", "signature", "签名", "sig 校验", ".sig")),
    ("C", ("manifest unknown", "ghcr", "docker pull", "manifest")),
    ("B", ("platform package", "平台分包", "optionaldependencies", "npm 平台")),
    ("D", ("digest", "sha256", "资产", "release download", "asset")),
    ("I", ("unhealthy", "healthy", "uid", "volume", "卷", "容器")),
    ("H", ("displayversion", "cfbundleshortversion", "版本", "version", "api/status")),
    ("E", ("installer", "安装", "dpkg -i", "setup.exe", "dmg")),
    ("F", ("启动", "startup", "存活", "crash", "panic")),
)


# ---------------------------------------------------------------------------
# 归类
# ---------------------------------------------------------------------------


def infer_category(text: str, form: str) -> str | None:
    """关键词兜底推断；推不出来返回 `None`（调用方 fail-closed 走 issue）。

    只在驱动脚本没写 `category` 时调用。刻意做得**保守**：宁可返回 `None` 让人看一眼，
    也不要凭一个宽泛的词（例如 "error"）猜出一个 A。
    """
    lowered = (text or "").lower()
    if not lowered:
        return None
    for keyword in NETWORK_KEYWORDS:
        if keyword in lowered:
            return "A"
    # 404 的语义取决于形态：npm 的 404 = 平台分包没发出来（B，重跑 build 的分包 job）；
    # 其它形态的 404 = Release 资产缺失（D）。GHCR 的 `manifest unknown` 先判（C），
    # 否则会被 404 抢走。
    if "manifest unknown" in lowered or "ghcr" in lowered or "docker pull" in lowered:
        return "C"
    if "404" in lowered or "not found" in lowered:
        return "B" if form == "npm" else "D"
    for category, keywords in INFER_RULES:
        if any(keyword in lowered for keyword in keywords):
            if category == "D" and form == "npm":
                return "B"
            return category
    return None


def severity_for(category: str | None, status: str, form: str) -> str:
    """`G` 是安全类（high）；`blocked`（J）是 info；其余 normal。"""
    if status == "blocked" or category == "J":
        return "info"
    if category == "G":
        return "high"
    if category is None:
        # 无法归类 → 按 normal 开 issue（不开 high，避免把「归类器没认出来」升级成安全事件）。
        return "normal"
    return "normal"


def issue_title(tag: str, form: str, cell: str) -> str:
    """issue 标题：`[release-smoke] v0.2.5 验收失败：linux / startup`（§7.3）。

    标题里带 tag 是去重的唯一依据（`gh issue list --search "v0.2.5 in:title"`），
    改格式会让 `issue.sh` 的去重失效——两处都要改。
    """
    return f"[release-smoke] {tag} 验收失败：{form} / {cell}"


def _item(
    *,
    form: str,
    cell: str,
    status: str,
    category: str | None,
    reason: str,
    tag: str,
    assertion: str = "",
    detail: str = "",
    evidence: Iterable[str] | None = None,
    log_url: str | None = None,
    severity: str | None = None,
) -> dict[str, Any]:
    """构造一个计划项（字段顺序即落盘顺序）。

    `severity` 缺省交给 :func:`severity_for` 推；只有「结果文件契约非法」这类
    **流水线缺陷**才显式传 `high`（`severity_for` 只看 category/status，推不出来）。
    """
    action = CATEGORY_ACTIONS.get(category or "", "issue")
    return {
        "form": form,
        "cell": cell,
        "status": status,
        "category": category,
        "severity": severity if severity in SEVERITIES else severity_for(category, status, form),
        "action": action,
        "reason": reason,
        "assertion": assertion,
        "detail": detail,
        "evidence": [str(item) for item in (evidence or [])],
        "logUrl": log_url,
        "issueTitle": issue_title(tag, form, cell) if action == "issue" else None,
    }


def items_from_result(result: dict[str, Any], tag: str) -> list[dict[str, Any]]:
    """一份结果 → 计划项（只出「需要处置」的格）。"""
    form = str(result.get("form") or "?")
    log_url = result.get("logUrl")
    items: list[dict[str, Any]] = []
    cells = result.get("cells") or []

    if not cells:
        items.append(
            _item(
                form=form,
                cell="(结果文件)",
                status="fail",
                category="A",
                reason="结果文件零格：driver 在 emit 之后、写第一格之前崩了（设计 D4）",
                tag=tag,
                log_url=log_url,
            )
        )
        return items

    for cell in cells:
        if not isinstance(cell, dict):
            items.append(
                _item(
                    form=form,
                    cell="(未知)",
                    status="fail",
                    category=None,
                    reason="格子不是对象（结果文件被手改过？）→ fail-closed 走 issue",
                    tag=tag,
                    log_url=log_url,
                )
            )
            continue
        status = cell.get("status")
        action = str(cell.get("action") or "?")
        assertion = str(cell.get("assertion") or "")
        detail = str(cell.get("detail") or "")
        evidence = cell.get("evidence") or []

        if status in ("pass", "skipped"):
            continue
        if status == "blocked":
            # J：取不到证据。⛔ 不算通过也不算失败，**不开 issue**（PRD J 类 / 设计 §6.2）。
            items.append(
                _item(
                    form=form,
                    cell=action,
                    status="blocked",
                    category="J",
                    reason=detail or "取不到证据（三条件成立，见设计 §6.2）",
                    tag=tag,
                    assertion=assertion,
                    detail=detail,
                    evidence=evidence,
                    log_url=log_url,
                )
            )
            continue
        if status != "fail":
            # 契约层已经拦掉了未知状态；这里仍然兜一层，避免「不认识的 status」被当通过。
            items.append(
                _item(
                    form=form,
                    cell=action,
                    status="fail",
                    category=None,
                    reason=f"未知 status={status!r}（契约应已拦下）→ fail-closed 走 issue",
                    tag=tag,
                    assertion=assertion,
                    detail=detail,
                    evidence=evidence,
                    log_url=log_url,
                )
            )
            continue

        declared = cell.get("category")
        if isinstance(declared, str) and declared in common.CATEGORIES:
            items.append(
                _item(
                    form=form,
                    cell=action,
                    status="fail",
                    category=declared,
                    reason=f"驱动脚本判定 {declared} 类：" + (detail or assertion or "（无 detail）"),
                    tag=tag,
                    assertion=assertion,
                    detail=detail,
                    evidence=evidence,
                    log_url=log_url,
                )
            )
            continue

        inferred = infer_category(f"{assertion} {detail}", form)
        if inferred:
            items.append(
                _item(
                    form=form,
                    cell=action,
                    status="fail",
                    category=inferred,
                    reason=f"驱动脚本未给 category，按关键词推断为 {inferred} 类："
                    + (detail or assertion or "（无 detail）"),
                    tag=tag,
                    assertion=assertion,
                    detail=detail,
                    evidence=evidence,
                    log_url=log_url,
                )
            )
            continue
        items.append(
            _item(
                form=form,
                cell=action,
                status="fail",
                category=None,
                reason="无法归类（既无 category 也推不出）→ fail-closed 走 issue，需人工看日志",
                tag=tag,
                assertion=assertion,
                detail=detail,
                evidence=evidence,
                log_url=log_url,
            )
        )
    return items


def missing_form_items(present: Iterable[str], tag: str) -> list[dict[str, Any]]:
    """整个形态没有结果文件 → A（一次重跑）；重跑仍无 → 由下一次 triage 升级为 E。"""
    items: list[dict[str, Any]] = []
    for form in common.FORMS:
        if form == "consistency" or form in present:
            continue
        items.append(
            _item(
                form=form,
                cell="(整个形态)",
                status="fail",
                category="A",
                reason="没有结果文件（驱动 job 崩了 / artifact 没上传）→ 重跑该 job 一次",
                tag=tag,
            )
        )
    return items


def form_from_path(path: str) -> str | None:
    """从结果文件名反推形态（`…/windows.json` → `windows`）；反推不出返回 `None`。

    用途：`windows.json` 存在但**契约非法**时它不会进 `results`，如果只按
    `results` 统计「哪些形态有结果」，这个形态会被同时记成「缺失」（A）与
    「契约非法」（issue）——同一份坏文件开两张单子（DEF-4 的对偶错误）。
    """
    name = os.path.basename(str(path or ""))
    stem = name[:-5] if name.endswith(".json") else name
    return stem if stem in common.FORMS else None


def read_problem_item(problem: dict[str, str], tag: str) -> dict[str, Any]:
    """读结果失败 → 计划项（DEF-4：两类问题的处置**不同**）。

    - `unreadable`（文件不存在 / 打不开）：驱动 job 崩了或没跑 → A / `rerun-job`；
    - `invalid`（读到了但契约非法）：流水线缺陷，重跑同一份脚本只会再坏一次 →
      `category=null` + `action=issue` + `severity=high`（人要改脚本，不是重跑）。

    入参是 :func:`read_results` 产出的 `{"path", "kind", "message"}`。
    """
    kind = str(problem.get("kind") or READ_PROBLEM_INVALID)
    message = str(problem.get("message") or "")
    form = form_from_path(str(problem.get("path") or "")) or "(结果文件)"
    if kind == READ_PROBLEM_UNREADABLE:
        return _item(
            form=form,
            cell="(不可读)",
            status="fail",
            category="A",
            reason=f"结果文件读不到（驱动 job 崩了 / artifact 没上传）→ 重跑该 job 一次：{message}",
            tag=tag,
        )
    return _item(
        form=form,
        cell="(契约非法)",
        status="fail",
        category=None,
        reason=f"结果文件存在但不符合契约（流水线缺陷，重跑无用，需要改生成结果的脚本）：{message}",
        tag=tag,
        severity="high",
    )


def dedupe(items: Sequence[dict[str, Any]]) -> list[dict[str, Any]]:
    """同一 `(form, cell, status)` 的多个项合成一项：issue 一张就够，理由拼接。

    键里带 `status` 是必须的：同一次 `startup` 里可能既有 `fail`(F) 又有 `blocked`(J)，
    把它们合并会把「取不到证据」的 ⛔ 折进一个失败项里（J 不该开 issue、也不该被 F 吞掉）。

    §8.2 第 2 条要求「每类每版本 ≤1」，去重是第一道；`dispose.py` 的计数器是第二道。
    """
    merged: dict[tuple[str, str, str], dict[str, Any]] = {}
    order: list[tuple[str, str, str]] = []
    for item in items:
        key = (str(item.get("form")), str(item.get("cell")), str(item.get("status")))
        if key not in merged:
            merged[key] = dict(item)
            order.append(key)
            continue
        kept = merged[key]
        # 保守合并：类别不同时保留「更严重」的那个（issue 优先于重跑），理由只追加一次。
        if kept.get("action") != "issue" and item.get("action") == "issue":
            kept["action"] = "issue"
            kept["category"] = item.get("category")
            kept["severity"] = item.get("severity")
            kept["issueTitle"] = item.get("issueTitle")
        if item.get("reason") and item["reason"] not in kept.get("reason", ""):
            kept["reason"] = f"{kept.get('reason') or ''} · {item['reason']}".strip(" ·")
    return [merged[key] for key in order]


def sort_items(items: Sequence[dict[str, Any]]) -> list[dict[str, Any]]:
    """稳定排序：形态按 `common.FORMS`，动作按 `common.ACTIONS`，其余按出现顺序。"""
    form_rank = {name: index for index, name in enumerate(common.FORMS)}
    action_rank = {name: index for index, name in enumerate(common.ACTIONS)}

    def key(item: dict[str, Any]) -> tuple[int, int]:
        return (
            form_rank.get(str(item.get("form")), len(form_rank)),
            action_rank.get(str(item.get("cell")), len(action_rank)),
        )

    return sorted(items, key=key)


# ---------------------------------------------------------------------------
# 读结果
# ---------------------------------------------------------------------------


def collect_result_paths(targets: Sequence[str]) -> list[str]:
    """把目录 / glob / 显式文件名统一成「结果 JSON 路径」列表。

    只认 `<form>.json` 与 `*.json`：`triage-plan.json` / `dispose.json` / `preflight.json`
    也在同一个目录里，**必须排除**（否则第二次运行时会把计划当结果读，自己喂自己）。
    """
    skip = {"preflight.json", "triage-plan.json", "dispose.json", "conclusion.json", "update-log.json"}
    found: list[str] = []
    for target in targets:
        if os.path.isdir(target):
            candidates = sorted(glob.glob(os.path.join(target, "*.json")))
        elif any(char in target for char in "*?["):
            candidates = sorted(glob.glob(target))
        else:
            candidates = [target]
        for path in candidates:
            name = os.path.basename(path)
            # `*plan.json`（本地 `--out plan.json` 常落在同一目录）同样是辅助产物，
            # 不排除就会被当成结果文件自喂出一条「契约非法」的假 issue。
            if name in skip or name.endswith(".tmp") or name.endswith("plan.json"):
                continue
            if path not in found:
                found.append(path)
    return found


def build_plan(
    results: Sequence[dict[str, Any]],
    tag: str,
    run_id: str | None,
    run_attempt: int,
    preflight: dict[str, Any] | None = None,
    expect_forms: bool = True,
    read_problems: Sequence[dict[str, str]] = (),
) -> dict[str, Any]:
    """结果集 → 计划（纯函数，自测直接调它）。

    `read_problems`（`{"path","kind","message"}`）也进计划：坏文件是流水线缺陷，
    不能静默（D1 的同一条纪律），处置由 :func:`read_problem_item` 按类别决定（DEF-4）。
    """
    items: list[dict[str, Any]] = []
    for result in results:
        items.extend(items_from_result(result, tag))
    for problem in read_problems:
        items.append(read_problem_item(problem, tag))
    if expect_forms:
        present = {str(result.get("form")) for result in results}
        # 文件在但读不动 ≠ 形态缺失：否则同一份坏文件会被同时记成 A 与 issue。
        for problem in read_problems:
            form = form_from_path(str(problem.get("path") or ""))
            if form:
                present.add(form)
        items.extend(missing_form_items(present, tag))
    items = sort_items(dedupe(items))

    fail_items = [item for item in items if item.get("status") == "fail"]
    blocked_items = [item for item in items if item.get("status") == "blocked"]
    if fail_items:
        verdict = "FAIL"
    elif blocked_items:
        verdict = "BLOCKED"
    else:
        verdict = "PASS"

    by_action: dict[str, int] = {}
    by_category: dict[str, int] = {}
    for item in items:
        by_action[str(item.get("action"))] = by_action.get(str(item.get("action")), 0) + 1
        key = str(item.get("category") or "null")
        by_category[key] = by_category.get(key, 0) + 1

    return {
        "schemaVersion": SCHEMA_VERSION,
        "generatedAt": common.now_iso(),
        "tag": tag,
        "version": common.normalize_version(tag),
        "runId": run_id,
        "runAttempt": run_attempt,
        "verdict": verdict,
        "preflight": {
            "ok": None if preflight is None else bool(preflight.get("ok")),
            "prevVersion": None if preflight is None else preflight.get("prevVersion"),
            "isLatest": None if preflight is None else preflight.get("isLatest"),
            "missingAssets": [] if preflight is None else preflight.get("missingAssets") or [],
        },
        "items": items,
        "summary": {
            "total": len(items),
            "fail": len(fail_items),
            "blocked": len(blocked_items),
            "byAction": by_action,
            "byCategory": by_category,
        },
    }


def render_plan_text(plan: dict[str, Any]) -> str:
    """人读的计划表（stdout / step summary 共用）。"""
    lines = [
        f"### triage 处置计划 · {plan['tag']} · **{plan['verdict']}**",
        "",
        f"- run `{plan.get('runId') or '—'}` attempt `{plan.get('runAttempt')}`"
        f" · 计划项 {plan['summary']['total']} 个"
        f"（fail {plan['summary']['fail']} / blocked {plan['summary']['blocked']}）",
        "",
        "| 形态 | 格 | 状态 | 类别 | 严重级 | 处置 | 理由 |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    for item in plan["items"]:
        reason = str(item.get("reason") or "").replace("|", "\\|")
        lines.append(
            "| `{form}` | `{cell}` | {status} | {category} | {severity} | `{action}` | {reason} |".format(
                form=item.get("form"),
                cell=item.get("cell"),
                status=item.get("status"),
                category=item.get("category") or "**null**",
                severity=item.get("severity"),
                action=item.get("action"),
                reason=reason,
            )
        )
    if not plan["items"]:
        lines.append("| — | — | — | — | — | `none` | 没有需要处置的格（全 pass / skipped） |")
    return "\n".join(lines) + "\n"


def write_plan(path: str, plan: dict[str, Any]) -> None:
    """原子写计划（先 `.tmp` 再 replace）：triage 后面几步都读它。"""
    parent = os.path.dirname(os.path.abspath(path))
    if parent:
        os.makedirs(parent, exist_ok=True)
    tmp = f"{path}.tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(plan, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    os.replace(tmp, path)


# ---------------------------------------------------------------------------
# 自测
# ---------------------------------------------------------------------------


def selftest() -> int:
    """离线自测：分类映射、兜底推断、fail-closed、缺形态、零格、去重、判定。"""
    problems: list[str] = []

    tag = "v0.2.5"
    if CATEGORY_ACTIONS["A"] != "rerun-job" or CATEGORY_ACTIONS["B"] != "rerun-release":
        problems.append("§8.1 的 A/B 映射错误")
    if CATEGORY_ACTIONS["C"] != "dispatch-docker" or CATEGORY_ACTIONS["D"] != "rerun-release":
        problems.append("§8.1 的 C/D 映射错误")
    if CATEGORY_ACTIONS["J"] != "none":
        problems.append("J 必须是 none（不开 issue）")
    for category in ("E", "F", "G", "H", "I"):
        if CATEGORY_ACTIONS[category] != "issue":
            problems.append(f"§8.1：{category} 必须走 issue")
    if set(CATEGORY_ACTIONS) != set(common.CATEGORIES):
        problems.append("类别映射表与 common.CATEGORIES 不一致")

    # 驱动脚本给了合法 category → 以它为准，哪怕文本里全是网络关键词。
    declared = common.new_result("windows", "0.2.5", "0.2.4", "in-app")
    declared["cells"].append(
        common.make_cell("upgrade", "fail", "自更新日志缺 install_start", category="F", detail="timeout")
    )
    items = items_from_result(declared, tag)
    if len(items) != 1 or items[0]["category"] != "F" or items[0]["action"] != "issue":
        problems.append(f"驱动已声明的 category 未被采信：{items}")

    # blocked → J/none，且不开 issue。
    blocked = common.new_result("linux", "0.2.5", "0.2.4", "pkg-upgrade")
    blocked["cells"].append(common.make_cell("startup", "blocked", "xvfb 下 20s 存活", detail="cannot open display"))
    items = items_from_result(blocked, tag)
    if not items or items[0]["action"] != "none" or items[0]["category"] != "J":
        problems.append(f"blocked 未归到 J/none：{items}")
    if items and items[0]["severity"] != "info":
        problems.append("blocked 的 severity 应为 info")

    # 没给 category：网络关键词 → A。
    inferred = common.new_result("macos", "0.2.5", "0.2.4", "pkg-upgrade")
    inferred["cells"].append(
        common.make_cell("install", "fail", "gh release download 失败", category="H", detail="")
    )
    # 上面给了合法 H，先验证「合法值优先」；再造一个真·无 category 的（契约外输入）。
    if items_from_result(inferred, tag)[0]["category"] != "H":
        problems.append("合法 category 优先级错误")
    if infer_category("curl: (28) timeout", "macos") != "A":
        problems.append("网络关键词未推断为 A")
    if infer_category("minisign 校验失败", "windows") != "G":
        problems.append("签名关键词未推断为 G")
    if infer_category("manifest unknown", "docker") != "C":
        problems.append("manifest 关键词未推断为 C")
    if infer_category("404 for @workbuddy-shift/linux-x64", "npm") != "B":
        problems.append("npm 的 404 应归 B 而不是 D")
    if infer_category("", "linux") is not None:
        problems.append("空文本不该推断出类别")
    if infer_category("莫名其妙的错误信息", "linux") is not None:
        problems.append("无法归类时应返回 None（fail-closed）")

    # fail-closed：category 缺失且推不出来 → issue，且 category=null。
    unknown = common.new_result("windows", "0.2.5")
    unknown["cells"].append({"action": "install", "status": "fail", "assertion": "xx", "category": None})
    items = items_from_result(unknown, tag)
    if not items or items[0]["action"] != "issue" or items[0]["category"] is not None:
        problems.append(f"无法归类时未 fail-closed 走 issue：{items}")

    # 零格 → A + rerun-job（设计 D4）。
    empty = common.new_result("npm", "0.2.5")
    items = items_from_result(empty, tag)
    if not items or items[0]["category"] != "A" or items[0]["action"] != "rerun-job":
        problems.append(f"零格未归到 A/rerun-job：{items}")

    # 缺形态 → A。用一份「只含 windows 且全 pass」的结果集验证。
    ok_result = common.new_result("windows", "0.2.5", "0.2.4", "in-app")
    ok_result["cells"].append(common.make_cell("install", "pass", "registry DisplayVersion == 0.2.5"))
    plan = build_plan([ok_result], tag, "123", 1)
    missing = {item["form"] for item in plan["items"]}
    expected_missing = {form for form in common.FORMS if form not in ("windows", "consistency")}
    if missing != expected_missing:
        problems.append(f"缺形态集合不符：{missing} != {expected_missing}")
    if plan["verdict"] != "FAIL":
        problems.append("有缺形态时判定应为 FAIL")

    # 全 pass → PASS，且没有计划项。
    full = []
    for form in common.FORMS:
        if form == "consistency":
            continue
        result = common.new_result(form, "0.2.5", "0.2.4")
        result["cells"].append(common.make_cell("install", "pass", "ok"))
        full.append(result)
    plan = build_plan(full, tag, "123", 1)
    if plan["items"]:
        problems.append(f"全 pass 时不该有计划项：{plan['items']}")
    if plan["verdict"] != "PASS":
        problems.append("全 pass 应判 PASS")

    # 全 blocked → BLOCKED（不是 PASS）。
    all_blocked = []
    for form in common.FORMS:
        if form == "consistency":
            continue
        result = common.new_result(form, "0.2.5", "0.2.4")
        result["cells"].append(common.make_cell("startup", "blocked", "取不到证据"))
        all_blocked.append(result)
    plan = build_plan(all_blocked, tag, "123", 1)
    if plan["verdict"] != "BLOCKED":
        problems.append(f"全 blocked 应判 BLOCKED，实际 {plan['verdict']}")
    if all(item["action"] == "none" for item in plan["items"]):
        pass
    else:
        problems.append("blocked 项不该产生处置")
    if any(item["issueTitle"] for item in plan["items"]):
        problems.append("blocked 项不该有 issue 标题（J 不开 issue）")

    # 去重：同一 (form, cell, status) 的两个失败合成一项；fail 与 blocked 不能互相吞。
    dup = common.new_result("linux", "0.2.5", "0.2.4")
    dup["cells"].append(common.make_cell("startup", "fail", "20s 存活", category="F", detail="退出"))
    dup["cells"].append(common.make_cell("startup", "fail", "error.log 无 crash", category="F", detail="panic"))
    dup["cells"].append(common.make_cell("startup", "blocked", "store 落盘", detail="SessionId=0"))
    merged = dedupe(items_from_result(dup, tag))
    if len(merged) != 2:
        problems.append(f"(form,cell,status) 去重结果应为 2 项（fail + blocked），实际 {len(merged)}")
    if not any(item["status"] == "blocked" and item["action"] == "none" for item in merged):
        problems.append("blocked 项被 fail 项吞掉了（J 不该被折进 issue）")

    # issue 标题含 tag（去重依据）。
    if issue_title("v0.2.5", "linux", "startup") != "[release-smoke] v0.2.5 验收失败：linux / startup":
        problems.append("issue 标题格式漂移（issue.sh 的去重会失效）")

    # DEF-4：两类「结果文件有问题」的处置必须分开。
    #   ① 读到了但契约非法（fail 缺 category）→ issue / category=null / severity=high；
    #   ② 读不到（文件不存在）→ A / rerun-job。
    tmp_def4 = tempfile.mkdtemp(prefix="classify-def4.")
    try:
        bad_path = os.path.join(tmp_def4, "windows.json")
        broken = common.new_result("windows", "0.2.5")
        broken["cells"] = [
            {"action": "install", "status": "fail", "assertion": "装不上", "category": None}
        ]
        with open(bad_path, "w", encoding="utf-8") as handle:
            json.dump(broken, handle, ensure_ascii=False)
        loaded, read_problems = read_results([bad_path])
        if loaded:
            problems.append("契约非法的结果文件不该进 results（应先被 validate 拒掉）")
        if len(read_problems) != 1 or read_problems[0]["kind"] != READ_PROBLEM_INVALID:
            problems.append(f"契约非法应记为 {READ_PROBLEM_INVALID}：{read_problems}")
        else:
            plan_def4 = build_plan(
                loaded, tag, "123", 1, expect_forms=False, read_problems=read_problems
            )
            item = plan_def4["items"][0] if plan_def4["items"] else {}
            if (
                item.get("action") != "issue"
                or item.get("category") is not None
                or item.get("severity") != "high"
            ):
                problems.append(f"契约非法未走 issue/null/high：{item}")
            if not item.get("issueTitle"):
                problems.append(f"契约非法的计划项缺 issueTitle（issue.sh 会丢弃）：{item}")
            if plan_def4["verdict"] != "FAIL":
                problems.append("契约非法时判定应为 FAIL")

        missing_path = os.path.join(tmp_def4, "npm.json")
        loaded, read_problems = read_results([missing_path])
        if len(read_problems) != 1 or read_problems[0]["kind"] != READ_PROBLEM_UNREADABLE:
            problems.append(f"文件不存在应记为 {READ_PROBLEM_UNREADABLE}：{read_problems}")
        else:
            plan_def4 = build_plan(
                loaded, tag, "123", 1, expect_forms=False, read_problems=read_problems
            )
            item = plan_def4["items"][0] if plan_def4["items"] else {}
            if item.get("action") != "rerun-job" or item.get("category") != "A":
                problems.append(f"读不到文件未走 A/rerun-job：{item}")
            # 坏文件不该同时被记成「形态缺失」：否则同一份文件开两张单子。
            with_forms = build_plan(
                loaded, tag, "123", 1, expect_forms=True, read_problems=read_problems
            )
            npm_items = [entry for entry in with_forms["items"] if entry.get("form") == "npm"]
            if len(npm_items) != 1:
                problems.append(f"同一份 npm 结果不该既记「缺失」又记「不可读」：{npm_items}")
    finally:
        shutil.rmtree(tmp_def4, ignore_errors=True)

    # collect_result_paths：不能把 triage-plan.json / preflight.json 当结果读。
    tmp = tempfile.mkdtemp(prefix="classify-selftest.")
    try:
        for name in ("windows.json", "triage-plan.json", "preflight.json", "x.json.tmp"):
            with open(os.path.join(tmp, name), "w", encoding="utf-8") as handle:
                handle.write("{}")
        paths = sorted(os.path.basename(path) for path in collect_result_paths([tmp]))
        if paths != ["windows.json"]:
            problems.append(f"collect_result_paths 未排除非结果文件：{paths}")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    for message in problems:
        print(f"SELFTEST-FAIL {message}")
    if problems:
        return 1
    print("self-test passed")
    return 0


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def read_results(paths: Sequence[str]) -> tuple[list[dict[str, Any]], list[dict[str, str]]]:
    """读并校验结果；坏文件记 problem 但**不中断**同批（D1 的同一条纪律）。

    返回 `(合法的结果列表, 问题列表)`；每个问题是
    `{"path": …, "kind": …, "message": …}`，`kind` 只能是
    :data:`READ_PROBLEM_UNREADABLE` / :data:`READ_PROBLEM_INVALID`（DEF-4：
    「读不到」与「读到了但不合法」的处置完全不同，不能混成一类）。
    """
    results: list[dict[str, Any]] = []
    problems: list[dict[str, str]] = []
    for path in paths:
        try:
            results.append(common.read_result(path))
        except common.SmokeContractError as error:
            # 文件读到了（能 open、能 json.load 或至少拿到了字节），但契约不成立：
            # JSON 坏 / 缺 category / 类型错 —— 都是「生成结果的脚本坏了」。
            problems.append(
                {"path": str(path), "kind": READ_PROBLEM_INVALID, "message": str(error)}
            )
        except OSError as error:
            # 文件不存在 / 权限 / 目录：驱动 job 崩了或 artifact 没上传，重跑有意义。
            problems.append(
                {
                    "path": str(path),
                    "kind": READ_PROBLEM_UNREADABLE,
                    "message": f"{path}: 读取失败 {error}",
                }
            )
    return results, problems


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="classify.py",
        description="release-smoke 归类：结果 JSON → triage-plan.json（不执行任何处置）",
    )
    parser.add_argument(
        "--results",
        nargs="+",
        metavar="DIR_OR_JSON",
        default=["artifacts/release-smoke"],
        help="结果目录 / glob / 文件（默认 artifacts/release-smoke）",
    )
    parser.add_argument("--preflight", default=None, help="preflight.json（可选，用于补充上下文）")
    parser.add_argument("--tag", default=os.environ.get("SMOKE_VERSION", ""), help="要验收的 tag")
    parser.add_argument("--run-id", default=os.environ.get("GITHUB_RUN_ID"), help="本次 run id")
    parser.add_argument(
        "--run-attempt",
        type=int,
        default=int(os.environ.get("GITHUB_RUN_ATTEMPT") or 1),
        help="本次 run 的 attempt（>=2 时 dispose 不再重跑）",
    )
    parser.add_argument(
        "--out",
        default=os.path.join("artifacts", "release-smoke", "triage-plan.json"),
        help="计划输出路径",
    )
    parser.add_argument("--no-expect-forms", action="store_true", help="不把「缺形态」算成 A 类")
    parser.add_argument("--selftest", action="store_true", help="离线自测")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    tag = (args.tag or "").strip()
    if not tag:
        print("ERROR 缺少 --tag（或环境变量 SMOKE_VERSION）", file=sys.stderr)
        return 2

    targets = list(args.results or [])
    if not targets:
        print("ERROR 没有给出 --results", file=sys.stderr)
        return 2

    paths = collect_result_paths(targets)
    results, read_problems = read_results(paths)

    preflight = None
    if args.preflight and os.path.exists(args.preflight):
        try:
            with open(args.preflight, encoding="utf-8") as handle:
                loaded = json.load(handle)
            preflight = loaded if isinstance(loaded, dict) else None
        except (OSError, ValueError) as error:
            print(f"::warning::preflight 读不了（不影响归类）：{error}", file=sys.stderr)

    plan = build_plan(
        results,
        tag,
        args.run_id,
        max(1, args.run_attempt),
        preflight=preflight,
        expect_forms=not args.no_expect_forms,
        read_problems=read_problems,
    )

    write_plan(args.out, plan)
    text = render_plan_text(plan)
    print(text)
    common.append_step_summary(text)

    for item in plan["items"]:
        if item["status"] == "blocked":
            print(f"::warning::[{item['form']}/{item['cell']}] BLOCKED(J)：{item['reason']}")
    for problem in read_problems:
        print(f"::error::{problem['message']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
