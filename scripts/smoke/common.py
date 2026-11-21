#!/usr/bin/env python3
"""发版后验收（release-smoke）结果契约的**唯一真源**。

本文件定义 `artifacts/release-smoke/<form>.json` 的全部字段名、必填项、白名单与
读写函数。其它脚本（`common.sh` / `common.ps1` / 各平台 driver / `render-conclusion.py`）
**不得**自己拼字段名，只能：

1. 直接 import 本模块（Python 侧）；
2. 或走本文件的 CLI——`emit` / `cell` / `finish` / `validate` / `probe` / `sha256` /
   `selftest`（shell 与 pwsh 侧）。

`selftest` 子命令离线跑契约自测（`python3 scripts/smoke/common.py selftest`，exit 0 = 契约成立）。

设计依据：`delivery/design-release-smoke.md` §9「数据契约」与 §6.3「矩阵与格子状态」。

字段命名口径
------------
设计文档 §9.1 的示例用 **camelCase**（`durationMs` / `prevVersion` / `upgradeMode`），
与仓库其它 JSON 契约（`src/lib/types/`、`api/dto.rs`）一致。任务书里出现的
`duration_ms` / `log_url` 是同一字段的 snake_case 写法，因此本模块：

- **写出**只产出 camelCase（一个字段一个名字，避免双份真源）；
- **读入**接受 snake_case 别名并自动归一化（`read_result` / `validate_result`），
  下游脚本用哪种写法都能跑。

`upgradeMode` 词表
------------------
§0.4 给出 `in-app` / `pkg-upgrade` / `appimage-replace` / `cli-update`，并**禁用**
`overwrite-install`（出现即判 FAIL）；§5.4 N4 与 §11 T04 又要求 npm 格写
`npm-upgrade`（§0.4 表格漏登记）。本模块按「实际用例」收全 5 个，并把
`overwrite-install` 单独列为禁用值。

两个层级都合法
--------------
- **顶层** `upgradeMode`：该**形态**的主线口径（`windows=in-app`、`macos=pkg-upgrade`、
  `npm=npm-upgrade`、`docker=null`），矩阵行首展示它；
- **格子级** `upgradeMode`（§9.1 的 `upgradeMode?`）：**仅当该格与主线不同**时写。
  Linux 一格里同时跑 deb 与 AppImage 两条路径，主线是 `pkg-upgrade`，AppImage 那几格
  标 `appimage-replace`。结论矩阵每格优先取格子级、无则取顶层。

格子级同样受白名单约束（`overwrite-install` 在**两个层级**都是禁用值）。

用法示例
--------
    # 建结果（driver 第一步）
    python3 scripts/smoke/common.py emit --form windows --version 0.2.5 \\
        --prev-version 0.2.4 --upgrade-mode in-app --out artifacts/release-smoke/windows.json

    # 追加一格（driver 每个断言一步）
    python3 scripts/smoke/common.py cell --file artifacts/release-smoke/windows.json \\
        --action install --status pass --assertion "registry DisplayVersion == 0.2.5"

    # 收尾（写 durationMs / 校验）
    python3 scripts/smoke/common.py finish --file artifacts/release-smoke/windows.json

    # 回填「跑完才知道」的顶层字段（productVersion / apiVersion / npmVersion …）
    python3 scripts/smoke/common.py set --file artifacts/release-smoke/windows.json \\
        --set productVersion=0.2.5

    # 校验任意结果文件（CI 与本地都能跑）
    python3 scripts/smoke/common.py validate artifacts/release-smoke/*.json
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import sys
import tempfile
import time
import urllib.error
import urllib.request
from typing import Any, Callable, Iterable, Sequence

# ---------------------------------------------------------------------------
# 白名单（新增格子 / 平台必须同时改这里 + render-conclusion.py 的矩阵列）
# ---------------------------------------------------------------------------

SCHEMA_VERSION = 1

#: 格子状态：设计 §6.3。
STATUSES: tuple[str, ...] = ("pass", "fail", "blocked", "skipped")

#: 升级口径：设计 §0.4 + §5.4（`npm-upgrade`）。
UPGRADE_MODES: tuple[str, ...] = (
    "in-app",
    "pkg-upgrade",
    "appimage-replace",
    "cli-update",
    "npm-upgrade",
)

#: 禁用口径：出现即判流水线缺陷（设计 §0.4 末行）。
BANNED_UPGRADE_MODES: tuple[str, ...] = ("overwrite-install",)

#: 形态（= 结果文件名，`<form>.json`）。`consistency` 由 render 步骤合成，不对应 runner。
FORMS: tuple[str, ...] = ("windows", "macos", "linux", "npm", "docker", "consistency")

#: 动作（矩阵列）。design §6.3 / §5.1–5.5。
ACTIONS: tuple[str, ...] = (
    "install",
    "upgrade",
    "startup",
    "version",
    "serve",
    "api",
    "pull",
    "health",
    "nonroot",
    "volume",
    "consistency",
)

#: 失败类别：设计 §8.1（A 瞬时 … J 无法判定）。
CATEGORIES: tuple[str, ...] = tuple("ABCDEFGHIJ")

#: 不需要跑失败归类的状态（`fail` 必须有 category）。
_STATUS_WITHOUT_CATEGORY: frozenset[str] = frozenset({"pass", "blocked", "skipped"})

REQUIRED_RESULT_FIELDS: tuple[str, ...] = (
    "schemaVersion",
    "form",
    "version",
    "startedAt",
    "durationMs",
    "env",
    "cells",
)

REQUIRED_CELL_FIELDS: tuple[str, ...] = ("action", "status", "assertion")

#: snake_case → camelCase。读入时归一化，写出时不使用。
FIELD_ALIASES: dict[str, str] = {
    "schema_version": "schemaVersion",
    "prev_version": "prevVersion",
    "upgrade_mode": "upgradeMode",
    "started_at": "startedAt",
    "finished_at": "finishedAt",
    "duration_ms": "durationMs",
    "log_url": "logUrl",
    "product_version": "productVersion",
    "api_version": "apiVersion",
    "npm_version": "npmVersion",
    "npx_package_version": "npxPackageVersion",
    "latest_json_version": "latestJsonVersion",
    "tag_version": "tagVersion",
    "body_head": "bodyHead",
    "elapsed_ms": "elapsedMs",
    "content_type": "contentType",
}

_VERSION_RE = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.\-]+))?$")


class SmokeContractError(ValueError):
    """结果 JSON 不符合契约。CI 里应当直接失败，不要静默修补。"""


# ---------------------------------------------------------------------------
# 版本号 / 时间
# ---------------------------------------------------------------------------


def normalize_version(value: str | None) -> str:
    """去首尾空白与 `v` / `V` 前缀（`v0.2.5` → `0.2.5`）。"""
    if not value:
        return ""
    return str(value).strip().lstrip("vV").strip()


def parse_version(value: str | None) -> tuple[int, int, int, str]:
    """解析 `X.Y.Z[-pre]`；非法版本抛 :class:`SmokeContractError`（不猜）。"""
    text = normalize_version(value)
    match = _VERSION_RE.match(text)
    if not match:
        raise SmokeContractError(f"版本号无法解析：{value!r}")
    major, minor, patch, pre = match.groups()
    return int(major), int(minor), int(patch), pre or ""


def compare_versions(left: str, right: str) -> int:
    """比较两个版本：返回 -1 / 0 / 1。预发布版本低于同号正式版。"""
    a = parse_version(left)
    b = parse_version(right)
    if a[:3] != b[:3]:
        return -1 if a[:3] < b[:3] else 1
    if a[3] == b[3]:
        return 0
    if not a[3]:
        return 1
    if not b[3]:
        return -1
    return -1 if a[3] < b[3] else 1


def same_version(left: str | None, right: str | None) -> bool:
    """`v0.2.5` 与 `0.2.5` 视为同一版本；两侧都为空视为不相等。"""
    a, b = normalize_version(left), normalize_version(right)
    return bool(a) and a == b


def now_iso() -> str:
    """当前时刻的 ISO 8601 UTC 字符串（秒精度，`Z` 结尾）。"""
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


# ---------------------------------------------------------------------------
# 结果 JSON：构造 / 归一化 / 校验 / 读写
# ---------------------------------------------------------------------------


def normalize_fields(data: Any) -> Any:
    """递归把 snake_case 键归一化成 camelCase（只作用于 dict 的键）。"""
    if isinstance(data, dict):
        result: dict[str, Any] = {}
        for key, value in data.items():
            canonical = FIELD_ALIASES.get(str(key), str(key))
            result[canonical] = normalize_fields(value)
        return result
    if isinstance(data, list):
        return [normalize_fields(item) for item in data]
    return data


def make_cell(
    action: str,
    status: str,
    assertion: str,
    category: str | None = None,
    detail: str = "",
    evidence: Iterable[str] | str | None = None,
    duration_ms: int = 0,
    upgrade_mode: str | None = None,
) -> dict[str, Any]:
    """构造一格。字段顺序即落盘顺序，便于 diff。

    `upgrade_mode` 是**格子级覆盖**（设计 §9.1 的 `upgradeMode?`）：只有该格的升级口径与
    形态主线不同时才写（例如 Linux 的 AppImage 格写 `appimage-replace`，而形态主线是
    `pkg-upgrade`）。非法值在这里就抛 :class:`SmokeContractError`——等到 `validate_cell`
    才发现会让「driver 已跑完 15 分钟」的 run 白跑。
    """
    if upgrade_mode is not None:
        problem = _check_upgrade_mode(upgrade_mode, "cells[?]")
        if problem:
            raise SmokeContractError(problem)
    if isinstance(evidence, str):
        evidence_list = [evidence] if evidence else []
    else:
        evidence_list = [str(item) for item in (evidence or [])]
    return {
        "action": action,
        "status": status,
        "category": category,
        "assertion": assertion,
        "detail": detail,
        "evidence": evidence_list,
        "durationMs": int(duration_ms),
        # 恒有该键：null = 继承顶层口径。显式 null 让「没写过」与「写过 null」在 diff 里可辨。
        "upgradeMode": upgrade_mode,
    }


def new_result(
    form: str,
    version: str,
    prev_version: str | None = None,
    upgrade_mode: str | None = None,
    extra: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """建一份空结果；`startedAt` 现在，`durationMs` 由 :func:`finish_result` 回填。"""
    result: dict[str, Any] = {
        "schemaVersion": SCHEMA_VERSION,
        "form": form,
        "version": normalize_version(version),
        "prevVersion": normalize_version(prev_version) if prev_version else None,
        "upgradeMode": upgrade_mode,
        "startedAt": now_iso(),
        "finishedAt": None,
        "durationMs": 0,
        "env": runner_env(),
        "cells": [],
        "productVersion": None,
        "apiVersion": None,
        "npmVersion": None,
        "npxPackageVersion": None,
        "logUrl": log_url_from_env(),
    }
    if extra:
        for key, value in normalize_fields(extra).items():
            result[str(key)] = value
    return result


def _check_upgrade_mode(mode: Any, where: str) -> str | None:
    """校验一个 `upgradeMode` 值；合法返回问题描述为 `None`，非法返回一句话。

    顶层（`validate_result`）与格子级（`validate_cell` / `make_cell`）共用同一份规则：
    **禁用值优先**报错（`overwrite-install` 是设计 §0.4 明令禁止的口径，不能混进
    「未知值」的泛指里），其次是白名单外。
    """
    if mode is None:
        return None
    if not isinstance(mode, str):
        return f"{where}.upgradeMode 必须是字符串或 null，实际 {mode!r}"
    if mode in BANNED_UPGRADE_MODES:
        return f"{where}.upgradeMode={mode!r} 是**禁用**口径（design §0.4）：出现即判流水线缺陷"
    if mode not in UPGRADE_MODES:
        return f"{where}.upgradeMode 未知：{mode!r}（白名单 {list(UPGRADE_MODES)}）"
    return None


def validate_cell(cell: Any, index: int) -> list[str]:
    """校验单格；返回问题列表（空列表 = 合法）。

    类型校验是**安全网**：CLI 产不出 `status=null`，但手写 / 未来脚本可能。过去只在
    `isinstance(x, str)` 成立时才查白名单，非字符串就整条跳过——`status: null` 的结果文件
    会一路走到 Gate 并渲染成 `❓ None`，最终**静默判 PASS**。因此这里一律先查类型再查白名单。
    """
    problems: list[str] = []
    if not isinstance(cell, dict):
        return [f"cells[{index}] 必须是对象，实际 {type(cell).__name__}"]
    cell = normalize_fields(cell)
    for field in REQUIRED_CELL_FIELDS:
        if field not in cell:
            problems.append(f"cells[{index}] 缺少必填字段 {field}")
    action = cell.get("action")
    if not isinstance(action, str):
        problems.append(f"cells[{index}].action 必须是字符串，实际 {action!r}")
    elif action not in ACTIONS:
        problems.append(f"cells[{index}].action 未知：{action!r}（白名单 {list(ACTIONS)}）")
    status = cell.get("status")
    if not isinstance(status, str):
        problems.append(f"cells[{index}].status 必须是字符串，实际 {status!r}")
    elif status not in STATUSES:
        problems.append(f"cells[{index}].status 未知：{status!r}（白名单 {list(STATUSES)}）")
    assertion = cell.get("assertion")
    if not isinstance(assertion, str):
        problems.append(f"cells[{index}].assertion 必须是字符串，实际 {assertion!r}")
    elif not assertion.strip():
        problems.append(f"cells[{index}].assertion 不能为空（进 issue 时要逐字引用）")
    category = cell.get("category")
    if status == "fail" and not category:
        problems.append(f"cells[{index}] status=fail 必须带 category（A–J）")
    if category and category not in CATEGORIES:
        problems.append(f"cells[{index}].category 未知：{category!r}（白名单 A–J）")
    if category and not isinstance(category, str):
        problems.append(f"cells[{index}].category 必须是字符串，实际 {category!r}")
    if status in _STATUS_WITHOUT_CATEGORY and category:
        problems.append(f"cells[{index}] status={status} 不应带 category（实际 {category}）")
    detail = cell.get("detail", "")
    if detail is not None and not isinstance(detail, str):
        problems.append(f"cells[{index}].detail 必须是字符串")
    evidence = cell.get("evidence", [])
    if evidence is not None and not isinstance(evidence, list):
        problems.append(f"cells[{index}].evidence 必须是数组")
    duration = cell.get("durationMs", 0)
    if not isinstance(duration, int) or isinstance(duration, bool):
        problems.append(f"cells[{index}].durationMs 必须是整数")
    elif duration < 0:
        problems.append(f"cells[{index}].durationMs 不能为负")
    # 格子级 upgradeMode 覆盖（R1）：可选，写了就必须在词表内、且不能是禁用值。
    mode_problem = _check_upgrade_mode(cell.get("upgradeMode"), f"cells[{index}]")
    if mode_problem:
        problems.append(mode_problem)
    return problems


def validate_result(raw: Any) -> list[str]:
    """校验整份结果；返回问题列表（空列表 = 合法）。入参允许 snake_case。"""
    data = normalize_fields(raw)
    if not isinstance(data, dict):
        return ["结果必须是 JSON 对象"]
    problems: list[str] = []
    for field in REQUIRED_RESULT_FIELDS:
        if field not in data:
            problems.append(f"缺少必填字段 {field}")
    if data.get("schemaVersion") != SCHEMA_VERSION:
        problems.append(
            f"schemaVersion={data.get('schemaVersion')!r}，本工具只认 {SCHEMA_VERSION}"
        )
    form = data.get("form")
    if not isinstance(form, str):
        problems.append(f"form 必须是字符串，实际 {form!r}")
    elif form not in FORMS:
        problems.append(f"form 未知：{form!r}（白名单 {list(FORMS)}；新增形态要同时改 common.py）")
    version = data.get("version")
    if isinstance(version, str):
        try:
            parse_version(version)
        except SmokeContractError as error:
            problems.append(f"version 非法：{error}")
    else:
        problems.append("version 必须是字符串")

    prev = data.get("prevVersion")
    if prev is not None and not isinstance(prev, str):
        problems.append("prevVersion 必须是字符串或 null")

    mode = data.get("upgradeMode")
    mode_problem = _check_upgrade_mode(mode, "顶层")
    if mode_problem:
        problems.append(mode_problem)

    duration = data.get("durationMs", 0)
    if not isinstance(duration, int) or isinstance(duration, bool):
        problems.append("durationMs 必须是整数")
    elif duration < 0:
        problems.append("durationMs 不能为负")

    env = data.get("env")
    if not isinstance(env, dict):
        problems.append("env 必须是对象")
    else:
        # `sessionId` 是**可选**字段（D7）：Windows 判 BLOCKED(J) 需要「SessionId == 0」的证据，
        # 但它不是所有平台都取得到，所以缺省不报错；写了就必须是整数或 null。
        session_id = env.get("sessionId")
        if session_id is not None and (
            not isinstance(session_id, int) or isinstance(session_id, bool)
        ):
            problems.append(f"env.sessionId 必须是整数或 null，实际 {session_id!r}")

    cells = data.get("cells")
    if not isinstance(cells, list):
        problems.append("cells 必须是数组")
    else:
        for index, cell in enumerate(cells):
            problems.extend(validate_cell(cell, index))
    return problems


def read_result(path: str | os.PathLike[str]) -> dict[str, Any]:
    """读一份结果并校验；不合法抛 :class:`SmokeContractError`（fail-closed）。

    JSON 解析失败也归一到 :class:`SmokeContractError`：损坏的文件必须有**可读的一句话** +
    Gate 的 `::error::`，而不是一段 Python traceback（D1：调用方只捕 SmokeContractError/OSError，
    过去 `JSONDecodeError` 会直接冒泡，连带同批其它合法结果都不渲染）。
    """
    try:
        with open(path, encoding="utf-8") as handle:
            raw = json.load(handle)
    except ValueError as error:  # json.JSONDecodeError 是 ValueError 的子类
        raise SmokeContractError(f"{path}: JSON 解析失败：{error}") from error
    data = normalize_fields(raw)
    problems = validate_result(data)
    if problems:
        raise SmokeContractError(f"{path}: " + "; ".join(problems))
    return data


def append_cell(path: str | os.PathLike[str], cell: dict[str, Any]) -> dict[str, Any]:
    """向已有结果追加一格并立即落盘（进程被安装器杀掉也不会丢已记录的格子）。"""
    data = read_result(path)
    data["cells"].append(normalize_fields(cell))
    write_result(path, data)
    return data


def finish_result(path: str | os.PathLike[str]) -> dict[str, Any]:
    """收尾：回填 `finishedAt` / `durationMs`，再校验落盘。"""
    data = read_result(path)
    started = data.get("startedAt") or now_iso()
    finished = now_iso()
    data["durationMs"] = max(0, _iso_to_epoch_ms(finished) - _iso_to_epoch_ms(started))
    data["finishedAt"] = finished
    write_result(path, data)
    return data


def write_result(path: str | os.PathLike[str], data: dict[str, Any]) -> None:
    """归一化 + 校验 + 原子写（先写 `.tmp` 再 replace）。"""
    normalized = normalize_fields(data)
    problems = validate_result(normalized)
    if problems:
        raise SmokeContractError(f"{path}: " + "; ".join(problems))
    target = str(path)
    parent = os.path.dirname(os.path.abspath(target))
    if parent:
        os.makedirs(parent, exist_ok=True)
    tmp = f"{target}.tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(normalized, handle, ensure_ascii=False, indent=2, sort_keys=False)
        handle.write("\n")
    os.replace(tmp, target)


def _iso_to_epoch_ms(text: str) -> int:
    """`YYYY-MM-DDTHH:MM:SSZ` → epoch 毫秒；解析失败返回 0（duration 归零不报错）。"""
    for fmt in ("%Y-%m-%dT%H:%M:%SZ", "%Y-%m-%dT%H:%M:%S"):
        try:
            return int(time.mktime(time.strptime(text[:19], fmt)) * 1000)
        except (TypeError, ValueError):
            continue
    return 0


def log_url_from_env() -> str | None:
    """当前 run 的日志入口（结论里每格都要能点回原始日志）。"""
    server = os.environ.get("GITHUB_SERVER_URL") or "https://github.com"
    repo = os.environ.get("GITHUB_REPOSITORY")
    run_id = os.environ.get("GITHUB_RUN_ID")
    if not repo or not run_id:
        return None
    attempt = os.environ.get("GITHUB_RUN_ATTEMPT") or "1"
    return f"{server}/{repo}/actions/runs/{run_id}/attempts/{attempt}"


# ---------------------------------------------------------------------------
# 运行环境 / 文件 / 网络
# ---------------------------------------------------------------------------


def runner_env() -> dict[str, Any]:
    """runner 环境快照：结论里解释「为什么某一格取不到证据」时要看它。

    `sessionId` 是可选字段（D7）：Windows 上「Session 0 = 无交互桌面」是判 BLOCKED(J) 的
    三条证据之一，所以 driver 用 `$env:SMOKE_SESSION_ID` 显式传进来；其它平台留 null。
    """
    session_raw = (os.environ.get("SMOKE_SESSION_ID") or "").strip()
    session_id: int | None = None
    if session_raw:
        try:
            session_id = int(session_raw)
        except ValueError:
            session_id = None
    return {
        "runner": os.environ.get("RUNNER_OS") or sys.platform,
        "runnerName": os.environ.get("RUNNER_NAME"),
        "osVersion": os.environ.get("RUNNER_OS_VERSION") or os.environ.get("ImageOS"),
        "arch": os.environ.get("RUNNER_ARCH"),
        "sessionId": session_id,
        "githubRunId": os.environ.get("GITHUB_RUN_ID"),
        "githubRunAttempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
    }


def sha256(path: str | os.PathLike[str]) -> str:
    """文件 sha256（小写十六进制）。资产哈希断言（AppImage / 安装包）共用这一份。"""
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def http_probe(
    url: str,
    expect_status: Sequence[int] = (200,),
    timeout: float = 15.0,
    headers: dict[str, str] | None = None,
    want_body: bool = False,
) -> dict[str, Any]:
    """探测一个 HTTP 端点；**不抛异常**，所有失败走返回值（driver 要写格子而不是崩）。

    返回 `{ok, status, elapsedMs, contentType, bodyHead, body, error}`。
    """
    started = time.monotonic()
    request = urllib.request.Request(url, method="GET")
    request.add_header("User-Agent", "workbuddy-shift-release-smoke/1")
    for key, value in (headers or {}).items():
        request.add_header(key, value)
    result: dict[str, Any] = {
        "ok": False,
        "status": None,
        "elapsedMs": 0,
        "contentType": None,
        "bodyHead": "",
        "body": None,
        "error": None,
    }
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read() if want_body else response.read(4096)
            result["status"] = int(response.status)
            result["contentType"] = response.headers.get("Content-Type")
            result["body"] = body.decode("utf-8", "replace")
            result["bodyHead"] = result["body"][:400]
            result["ok"] = int(response.status) in tuple(expect_status)
            if not result["ok"]:
                result["error"] = f"HTTP {response.status} not in {list(expect_status)}"
    except urllib.error.HTTPError as error:
        result["status"] = int(error.code)
        result["error"] = f"HTTP {error.code}"
        result["ok"] = int(error.code) in tuple(expect_status)
        try:
            result["bodyHead"] = error.read(400).decode("utf-8", "replace")
        except OSError:
            result["bodyHead"] = ""
    except Exception as error:  # noqa: BLE001 - 网络错误统一归一化，driver 不该因探测崩掉
        result["error"] = f"{type(error).__name__}: {error}"
    result["elapsedMs"] = int((time.monotonic() - started) * 1000)
    return result


def retry(
    func: Callable[[], Any],
    attempts: int = 3,
    delay: float = 2.0,
    on_retry: Callable[[int, Exception], None] | None = None,
) -> Any:
    """重试 `attempts` 次（默认 3），最后一次仍失败则抛出原异常。"""
    last: Exception | None = None
    for index in range(max(1, attempts)):
        try:
            return func()
        except Exception as error:  # noqa: BLE001 - 重试策略由调用方决定
            last = error
            if on_retry:
                on_retry(index + 1, error)
            if index + 1 < attempts:
                time.sleep(delay)
    raise last if last else RuntimeError("retry: 无可重试的调用")


def wait_until(
    predicate: Callable[[], bool],
    timeout: float,
    interval: float = 0.5,
) -> bool:
    """轮询直到 `predicate` 为真或超时；返回是否命中。"""
    deadline = time.monotonic() + max(0.0, timeout)
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    return predicate()


# ---------------------------------------------------------------------------
# CI 输出
# ---------------------------------------------------------------------------


def write_github_output(pairs: dict[str, Any], path: str | None = None) -> bool:
    """写 `$GITHUB_OUTPUT`（key=value 单行格式）；不在 CI 里返回 False。"""
    target = path or os.environ.get("GITHUB_OUTPUT")
    if not target:
        return False
    with open(target, "a", encoding="utf-8", newline="\n") as handle:
        for key, value in pairs.items():
            text = "" if value is None else str(value)
            if "\n" in text:
                raise SmokeContractError(f"GitHub output 的值不能含换行：{key}")
            handle.write(f"{key}={text}\n")
    return True


def append_step_summary(markdown: str, path: str | None = None) -> bool:
    """追加 job summary；不在 CI 里返回 False。"""
    target = path or os.environ.get("GITHUB_STEP_SUMMARY")
    if not target:
        return False
    with open(target, "a", encoding="utf-8", newline="\n") as handle:
        handle.write(markdown.rstrip("\n") + "\n")
    return True


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _split_evidence(values: Sequence[str] | None) -> list[str]:
    items: list[str] = []
    for value in values or []:
        for part in str(value).split(";"):
            part = part.strip()
            if part:
                items.append(part)
    return items


def _cmd_emit(args: argparse.Namespace) -> int:
    extra: dict[str, Any] = {}
    for pair in args.set or []:
        key, _, value = pair.partition("=")
        if not key:
            raise SmokeContractError(f"--set 需要 key=value，实际 {pair!r}")
        extra[key] = value
    result = new_result(
        form=args.form,
        version=args.version,
        prev_version=args.prev_version,
        upgrade_mode=args.upgrade_mode,
        extra=extra,
    )
    write_result(args.out, result)
    print(f"[smoke] 初始化结果：{args.out}（form={args.form} version={result['version']}）")
    return 0


def _cmd_cell(args: argparse.Namespace) -> int:
    cell = make_cell(
        action=args.action,
        status=args.status,
        assertion=args.assertion,
        category=args.category,
        detail=args.detail or "",
        evidence=_split_evidence(args.evidence),
        duration_ms=args.duration_ms,
        upgrade_mode=args.upgrade_mode,
    )
    append_cell(args.file, cell)
    flag = f" category={cell['category']}" if cell["category"] else ""
    if cell["upgradeMode"]:
        flag += f" upgradeMode={cell['upgradeMode']}"
    print(f"[smoke] {args.file} += {args.action}/{args.status}{flag}")
    return 0


def _cmd_finish(args: argparse.Namespace) -> int:
    data = finish_result(args.file)
    print(f"[smoke] 收尾：{args.file}（durationMs={data['durationMs']}）")
    return 0


def _cmd_set(args: argparse.Namespace) -> int:
    """回填顶层字段（`productVersion` / `apiVersion` / `npmVersion` …）。

    这些值只有**跑完**才知道（装完读版本 / 探测 `/api/status`），而 `emit` 在第一步就写了
    结果文件。driver 因此在断言通过后调用本命令回填；写入仍走 :func:`write_result`
    （校验 + 原子写），字段名走 :data:`FIELD_ALIASES`，不接受任意键。
    """
    data = read_result(args.file)
    changed: list[str] = []
    for pair in args.set or []:
        key, _, value = pair.partition("=")
        if not key:
            raise SmokeContractError(f"--set 需要 key=value，实际 {pair!r}")
        canonical = FIELD_ALIASES.get(key, key)
        data[canonical] = value
        changed.append(canonical)
    write_result(args.file, data)
    print(f"[smoke] {args.file} 回填：{', '.join(changed)}")
    return 0


def _cmd_validate(args: argparse.Namespace) -> int:
    failures = 0
    for path in args.files:
        try:
            data = read_result(path)
        except SmokeContractError as error:
            failures += 1
            print(f"ERROR {error}", file=sys.stderr)
            continue
        print(f"OK    {path}（{data['form']} · {len(data['cells'])} 格）")
    return 1 if failures else 0


def _cmd_probe(args: argparse.Namespace) -> int:
    probe = http_probe(
        args.url,
        expect_status=tuple(int(item) for item in args.expect),
        timeout=args.timeout,
        want_body=args.body,
    )
    print(json.dumps(probe, ensure_ascii=False))
    return 0 if probe["ok"] else 1


def _cmd_sha256(args: argparse.Namespace) -> int:
    print(sha256(args.file))
    return 0


def _cmd_selftest(_args: argparse.Namespace) -> int:
    """契约自测：不联网、不依赖 CI 环境，任何平台都能跑。"""
    problems: list[str] = []

    good = new_result("windows", "v0.2.5", "v0.2.4", "in-app")
    good["cells"].append(
        make_cell("install", "pass", "registry DisplayVersion == 0.2.5", evidence=["a.log"])
    )
    problems += validate_result(good)

    if not validate_result(new_result("windows", "0.2.5", None, "overwrite-install")):
        problems.append("禁用口径 overwrite-install 未被拒绝")
    if not validate_result(new_result("nosuchform", "0.2.5")):
        problems.append("未知 form 未被拒绝")
    if not validate_result({"form": "windows"}):
        problems.append("缺字段未被拒绝")

    fail_cell = make_cell("startup", "fail", "20s 存活", category=None)
    if not validate_result({**new_result("windows", "0.2.5"), "cells": [fail_cell]}):
        problems.append("fail 缺 category 未被拒绝")

    aliased = {"duration_ms": 1, "log_url": "x", "upgrade_mode": "in-app"}
    if normalize_fields(aliased)["durationMs"] != 1 or "logUrl" not in normalize_fields(aliased):
        problems.append("snake_case 别名未归一化")

    # D2（类型安全网）：非字符串的 action/status/assertion/form 必须被拒，不能再「跳过即通过」。
    for label, cell in (
        ("status=null", {"action": "install", "status": None, "assertion": "x"}),
        ("status=123", {"action": "install", "status": 123, "assertion": "x"}),
        ("action=null", {"action": None, "status": "pass", "assertion": "x"}),
        ("assertion=123", {"action": "install", "status": "pass", "assertion": 123}),
        ("assertion 空串", {"action": "install", "status": "pass", "assertion": "   "}),
    ):
        if not validate_result({**new_result("windows", "0.2.5"), "cells": [cell]}):
            problems.append(f"坏数据未被拒：{label}")
    if not validate_result({**new_result("windows", "0.2.5"), "form": None}):
        problems.append("坏数据未被拒：form=null")
    # 可选字段：sessionId 缺省合法，写了非整数要拒。
    if validate_result({**new_result("windows", "0.2.5"), "env": {"runner": "x"}}):
        problems.append("env 缺 sessionId 应当合法（可选字段）")
    if not validate_result({**new_result("windows", "0.2.5"), "env": {"sessionId": "0"}}):
        problems.append("env.sessionId 非整数未被拒")

    # R1（格子级 upgradeMode）：合法覆盖值必须能为（Linux AppImage 格），禁用值必须拒。
    linux = new_result("linux", "0.2.5", "0.2.4", "pkg-upgrade")
    override = make_cell(
        "install", "pass", "AppImage sha256 == Release digest", upgrade_mode="appimage-replace"
    )
    linux["cells"].append(override)
    if validate_result(linux):
        problems.append("cells[].upgradeMode=appimage-replace 应当合法（格子级覆盖）")
    if make_cell("install", "pass", "x")["upgradeMode"] is not None:
        problems.append("未指定 upgradeMode 的格子应写 null（继承顶层口径）")
    try:
        make_cell("install", "pass", "x", upgrade_mode="overwrite-install")
    except SmokeContractError:
        pass
    else:
        problems.append("cells[].upgradeMode=overwrite-install 未被 make_cell 拒绝")
    for bad_mode, label in ((("overwrite-install"), "禁用值"), (123, "非字符串"), ("nope", "未知值")):
        broken = new_result("linux", "0.2.5", "0.2.4", "pkg-upgrade")
        broken["cells"].append({**override, "upgradeMode": bad_mode})
        if not validate_result(broken):
            problems.append(f"cells[].upgradeMode {label} 未被 validate_cell 拒绝")
    aliased_cell = normalize_fields({"upgrade_mode": "appimage-replace"})
    if aliased_cell.get("upgradeMode") != "appimage-replace":
        problems.append("cells[].upgrade_mode 的 snake_case 别名未归一化")

    # D1（损坏 JSON）：read_result 必须抛 SmokeContractError（含文件名），不是 JSONDecodeError。
    broken_dir = tempfile.mkdtemp(prefix="smoke-selftest.")
    try:
        broken_path = os.path.join(broken_dir, "broken.json")
        with open(broken_path, "w", encoding="utf-8") as handle:
            handle.write('{"form":"windows", "cells":[')
        try:
            read_result(broken_path)
            problems.append("损坏 JSON 未被拒（read_result 没抛异常）")
        except SmokeContractError as error:
            if "JSON 解析失败" not in str(error):
                problems.append(f"损坏 JSON 的错误文案不含「JSON 解析失败」：{error}")
        except ValueError as error:  # noqa: BLE001 - 这里就是要确认不是裸 ValueError
            problems.append(f"损坏 JSON 抛出了裸 ValueError（D1 回归）：{type(error).__name__}")
    finally:
        shutil.rmtree(broken_dir, ignore_errors=True)

    if compare_versions("0.2.5", "0.2.4") != 1:
        problems.append("版本比较 0.2.5 > 0.2.4 失败")
    if compare_versions("0.2.5-rc.1", "0.2.5") != -1:
        problems.append("预发布版本未低于同号正式版")
    if not same_version("v0.2.5", "0.2.5"):
        problems.append("v 前缀未归一化")

    for message in problems:
        print(f"SELFTEST-FAIL {message}")
    if problems:
        return 1
    print("self-test passed")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="common.py",
        description="release-smoke 结果契约的唯一真源（读写 / 校验 / 探测 / 哈希）",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    emit = sub.add_parser("emit", help="新建一份结果 JSON")
    emit.add_argument("--form", required=True, choices=FORMS)
    emit.add_argument("--version", required=True, help="目标版本，可带 v 前缀")
    emit.add_argument("--prev-version", default=None)
    emit.add_argument("--upgrade-mode", default=None, choices=UPGRADE_MODES)
    emit.add_argument("--set", action="append", metavar="KEY=VALUE", help="额外顶层字段，可重复")
    emit.add_argument("--out", required=True)
    emit.set_defaults(func=_cmd_emit)

    cell = sub.add_parser("cell", help="向结果追加一格")
    cell.add_argument("--file", required=True)
    cell.add_argument("--action", required=True, choices=ACTIONS)
    cell.add_argument("--status", required=True, choices=STATUSES)
    cell.add_argument("--assertion", required=True, help="断言原文（进 issue 时逐字引用）")
    cell.add_argument("--category", default=None, choices=CATEGORIES)
    cell.add_argument("--detail", default="")
    cell.add_argument("--evidence", action="append", metavar="PATH[;PATH]")
    cell.add_argument("--duration-ms", type=int, default=0)
    cell.add_argument(
        "--upgrade-mode",
        default=None,
        choices=UPGRADE_MODES,
        help="格子级升级口径覆盖（仅当与形态主线不同时写，例如 appimage-replace）",
    )
    cell.set_defaults(func=_cmd_cell)

    finish = sub.add_parser("finish", help="回填 finishedAt / durationMs")
    finish.add_argument("--file", required=True)
    finish.set_defaults(func=_cmd_finish)

    patch = sub.add_parser("set", help="回填顶层字段（productVersion / apiVersion …）")
    patch.add_argument("--file", required=True)
    patch.add_argument("--set", action="append", metavar="KEY=VALUE", required=True, help="可重复")
    patch.set_defaults(func=_cmd_set)

    validate = sub.add_parser("validate", help="校验一份或多份结果")
    validate.add_argument("files", nargs="+")
    validate.set_defaults(func=_cmd_validate)

    probe = sub.add_parser("probe", help="HTTP 探测（输出 JSON，不抛异常）")
    probe.add_argument("--url", required=True)
    probe.add_argument("--expect", action="append", default=None, help="期望状态码，可重复，默认 200")
    probe.add_argument("--timeout", type=float, default=15.0)
    probe.add_argument("--body", action="store_true", help="读完整响应体（默认只读前 4KB）")
    probe.set_defaults(func=_cmd_probe, expect=None)

    digest = sub.add_parser("sha256", help="输出文件 sha256")
    digest.add_argument("--file", required=True)
    digest.set_defaults(func=_cmd_sha256)

    selftest = sub.add_parser("selftest", help="契约自测（离线）")
    selftest.set_defaults(func=_cmd_selftest)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if getattr(args, "expect", "missing") is None:
        args.expect = ["200"]
    try:
        return int(args.func(args))
    except SmokeContractError as error:
        print(f"ERROR {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
