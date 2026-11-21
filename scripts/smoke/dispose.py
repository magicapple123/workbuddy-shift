#!/usr/bin/env python3
r"""按 `triage-plan.json` 执行**白名单处置**，并守住「防风暴」闸门（设计 §8.2 / §8.4）。

本文件是整条验收链里**唯一会改变仓库外部状态**的一步，所以硬约束写成代码而不是注释：

| 闸门 | 实现位置 |
|---|---|
| 单 run 总处置 ≤ 3（优先级 `A > C > D > B`） | [`plan_dispositions`] |
| 每类每版本 ≤ 1 | [`plan_dispositions`]（`--used-categories` + 本次已选类别） |
| `run_attempt >= 2` 一律不重跑 | [`plan_dispositions`]（一条都不选） |
| `dry_run` 只打印不执行，且**零 gh 调用**（连 job 定位都不做） | [`execute`] |
| 只允许三类动作 | [`ALLOWED_ACTIONS`] + [`build_command`]；白名单外的项进 `rejected`（[`reject_items`]） |

允许的动作**只有**（§8.4 的 gh 命令清单，逐字对齐）：

- `rerun-job` → `gh run rerun <run-id> --failed`
- `dispatch-docker` → `gh workflow run docker.yml --ref <tag> -f push_image=true`
- `rerun-release` → 定位 `build.yml` 里该 tag 的 run，再 `gh api -X POST
  /repos/{repo}/actions/jobs/{job_id}/rerun`
- 其它一律拒绝（`gh` 二进制由 [`run_gh`] 统一调用，**不拼 shell 字符串**）

`--dry-run` 的语义是「**一条 gh 命令都不发**」，所以连**只读**的 job 定位也要跳过：
`rerun-release` 在 dry-run 下的命令里 job id 写成 `<待解析>`，并注明「执行时才解析」
（过去定位在 dry-run 判定之前执行，实测会发 2 条只读 GET：`gh run list` 与
`gh api .../jobs`，演练就不再是零副作用）。

`rejected`
----------
白名单外的动作（例如手改计划塞进来的 `release-delete` / `npm-publish`）不会被静默丢掉：
记进报告的 `rejected[]`（含动作与原因）并在 CLI 打印。`issue` / `none` 是计划里的
**被动动作**（由 `issue.sh` 负责），不算 rejected。

明确**不做**（设计 §1.2 的非目标）：改源码、删 tag、撤回 Release、`gh release delete`、
`git push`、`npm publish`。这些即使计划里出现也会被 [`build_command`] 拒绝。

与 issue 的分工
---------------
`dispose.py` 只管「重跑 / 派发」。**开 issue 不在本文件**（`issue.sh` 负责），因为：
E–I 类本来就没有自动动作；`rerun-release` 定位不到 build job 时也要开 issue（§8.1 B 的
「无 run 或 >7 天 → 升级为 issue」）——这些都由 `issue.sh` 读 `dispose.json` 的
`unresolved` 统一处理，避免「处置失败」和「产品失败」两张 issue。

用法
----
    # 演练：只打印将执行什么，一条 gh 命令都不发
    python3 scripts/smoke/dispose.py --plan artifacts/release-smoke/triage-plan.json --dry-run

    # CI（triage 第二步；dry_run input 为 true 时 workflow 传 --dry-run）
    python3 scripts/smoke/dispose.py --plan artifacts/release-smoke/triage-plan.json \\
        --tag v0.2.5 --run-id "$GITHUB_RUN_ID" --run-attempt "$GITHUB_RUN_ATTEMPT" \\
        --out artifacts/release-smoke/dispose.json

    # 离线自测（不跑 gh）
    python3 scripts/smoke/dispose.py --selftest

退出码：0 = 处置已执行 / 无需处置（**处置失败也是 0**：triage 的结论由结果决定，
不是因为「重跑没发出去」而变红）；1 = 计划不可读；2 = 用法错误（argparse）。
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import subprocess
import sys
from typing import Any, Callable, Sequence

SCHEMA_VERSION = 1

#: 本文件允许执行的动作（**白名单**）。多一个都不执行，且会写进 `rejected`。
ALLOWED_ACTIONS: tuple[str, ...] = ("rerun-job", "dispatch-docker", "rerun-release")

#: 计划里**合法但不由本文件执行**的动作：开 issue 在 `issue.sh`，`none` 是 J 类（⛔）。
#: 它们进 `suppressed` / 被跳过，但**不该**算「非法动作」（否则每次 triage 都刷警告）。
PASSIVE_ACTIONS: tuple[str, ...] = ("issue", "none")

#: `--dry-run` 下 `rerun-release` 的 build job id 占位符：dry-run 不解析（零 gh 调用），
#: 只用它把「将要执行的命令形态」印出来。
BUILD_JOB_PLACEHOLDER = "<待解析>"

#: 处置优先级（§8.2 第 1 条）：A 瞬时 > C 镜像缺失 > D 资产缺失 > B 分包缺失。
PRIORITY: dict[str, int] = {"A": 0, "C": 1, "D": 2, "B": 3}

#: 单 run 处置上限。
DEFAULT_LIMIT = 3

#: `rerun-release` 定位 build.yml 的 job 名关键词：D 找 `release`，B 找 `publish`。
BUILD_JOB_HINTS: dict[str, tuple[str, ...]] = {
    "D": ("release",),
    "B": ("publish", "npm", "package"),
}

GhRunner = Callable[[Sequence[str]], tuple[int, str]]


# ---------------------------------------------------------------------------
# 闸门（纯函数：自测直接打这里）
# ---------------------------------------------------------------------------


def candidate_items(items: Sequence[dict[str, Any]]) -> list[dict[str, Any]]:
    """只挑「需要自动动作」的项，并按优先级 / 形态稳定排序。"""
    picked = [item for item in items if item.get("action") in ALLOWED_ACTIONS]
    form_order = {name: index for index, name in enumerate(("windows", "macos", "linux", "npm", "docker"))}
    return sorted(
        picked,
        key=lambda item: (
            PRIORITY.get(str(item.get("category") or ""), 99),
            form_order.get(str(item.get("form")), 99),
            str(item.get("cell")),
        ),
    )


def plan_dispositions(
    items: Sequence[dict[str, Any]],
    run_attempt: int = 1,
    used_categories: Sequence[str] = (),
    limit: int = DEFAULT_LIMIT,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """返回 `(要执行的项, 被抑制的项)`。**纯函数**：不发命令、不读文件、无副作用。

    抑制优先级从高到低（先命中的理由更硬，直接决定 reason）：

    1. `run_attempt >= 2`：已经重跑过一次，再重跑就是在掩盖问题（§8.2 第 3 条）；
    2. 该类别在本版本已用过（`used_categories` 或本次已选中）；
    3. 累计已达到 `limit`（默认 3）。
    """
    suppressed: list[dict[str, Any]] = []
    if run_attempt >= 2:
        for item in candidate_items(items):
            suppressed.append(
                _suppressed(item, f"run_attempt={run_attempt} ≥ 2：不再重跑/派发（§8.2 第 3 条）")
            )
        return [], suppressed

    used = {str(category) for category in used_categories if category}
    selected: list[dict[str, Any]] = []
    for item in candidate_items(items):
        category = str(item.get("category") or "")
        if category and category in used:
            suppressed.append(_suppressed(item, f"{category} 类本版本已处置过（§8.2 第 2 条）"))
            continue
        if len(selected) >= max(0, limit):
            suppressed.append(_suppressed(item, f"单 run 处置已达上限 {limit}（§8.2 第 1 条）"))
            continue
        if category:
            used.add(category)
        selected.append(item)
    return selected, suppressed


def _suppressed(item: dict[str, Any], reason: str) -> dict[str, Any]:
    """被抑制项的记录（进结论区块的「已抑制」与日志）。"""
    return {
        "form": item.get("form"),
        "cell": item.get("cell"),
        "category": item.get("category"),
        "action": item.get("action"),
        "reason": reason,
    }


def reject_items(items: Sequence[dict[str, Any]]) -> list[dict[str, Any]]:
    """白名单外（且不是 `issue` / `none` 这类被动动作）的计划项 → `rejected`。

    过去这些项被 [`candidate_items`] 静默过滤：计划里写了 `npm-publish` / `release-delete`
    也毫无痕迹，报告里只报 executed / suppressed / unresolved（DEF-7）。计划是自动化写的，
    「有项被忽略」必须是**可见**的信号——它意味着 classify 的映射表与白名单不一致了。
    """
    rejected: list[dict[str, Any]] = []
    for item in items:
        action = item.get("action")
        if action in ALLOWED_ACTIONS or action in PASSIVE_ACTIONS:
            continue
        rejected.append(
            {
                "form": item.get("form"),
                "cell": item.get("cell"),
                "category": item.get("category"),
                "action": action,
                "reason": (
                    f"动作不在白名单：{action!r}（允许 {list(ALLOWED_ACTIONS)}；"
                    f"{list(PASSIVE_ACTIONS)} 由 issue.sh 负责）"
                ),
            }
        )
    return rejected


_classify_module: Any | None = None


def _issue_title_func() -> Callable[[str, str, str], str] | None:
    """取 `classify.py` 的 `issue_title`（issue 标题的**唯一真源**）。

    `issue.sh` 靠「标题里带 tag」去重（`gh issue list --search "<tag> in:title"`）：
    两处各拼一套格式，去重会**静默**失效。所以这里按路径加载同目录的 `classify.py`
    复用同一个函数，而不是抄一遍 f-string。

    延迟加载（失败返回 `None`）：`dispose.py` 平时不该因为 `classify.py` 的加载问题而崩；
    真拿不到标题时留空，`issue.sh` 会兜底生成（双保险，见 issue.sh 的 title 兜底）。
    """
    global _classify_module
    if _classify_module is None:
        path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "classify.py")
        try:
            spec = importlib.util.spec_from_file_location("wb_smoke_classify", path)
            if spec is None or spec.loader is None:
                raise ImportError(f"无法加载 {path}")
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
        except (OSError, ImportError, SyntaxError, SystemExit) as error:
            print(
                f"::warning::加载 {path} 失败（issue 标题留空，由 issue.sh 兜底）：{error}",
                file=sys.stderr,
            )
            return None
        _classify_module = module
    func = getattr(_classify_module, "issue_title", None)
    return func if callable(func) else None


def issue_title_for(tag: str, item: dict[str, Any]) -> str:
    """计划项 → issue 标题（同源 `classify.issue_title()`）；拿不到规则时返回空串。"""
    if not tag:
        return ""
    func = _issue_title_func()
    if func is None:
        return ""
    return func(tag, str(item.get("form") or "?"), str(item.get("cell") or "?"))


def build_command(
    item: dict[str, Any],
    *,
    tag: str,
    repo: str,
    run_id: str,
    build_run_id: str,
    build_job_id: str,
) -> list[str]:
    """把一个计划项翻译成 gh 命令行（**列表**形态，绝不拼 shell 字符串）。

    不认识的 action 直接抛 `ValueError`：白名单之外的动作不该走到这里，宁可让 triage
    报错也不要有「顺手执行了什么」的可能。
    """
    action = item.get("action")
    if action not in ALLOWED_ACTIONS:
        raise ValueError(f"动作不在白名单：{action!r}（允许 {list(ALLOWED_ACTIONS)}）")
    if action == "rerun-job":
        return ["gh", "run", "rerun", run_id, "--failed"]
    if action == "dispatch-docker":
        return ["gh", "workflow", "run", "docker.yml", "--ref", tag, "-f", "push_image=true"]
    # rerun-release：直接重跑 build.yml 里失败的那个 job（release / publish-*）。
    return [
        "gh",
        "api",
        "-X",
        "POST",
        f"/repos/{repo}/actions/jobs/{build_job_id}/rerun",
    ]


# ---------------------------------------------------------------------------
# gh 调用
# ---------------------------------------------------------------------------


def run_gh(args: Sequence[str]) -> tuple[int, str]:
    """跑一条 gh 命令，返回 `(exit, 合并后的输出)`。**不抛异常**：处置失败不该让 triage 崩。"""
    try:
        result = subprocess.run(
            list(args),
            check=False,
            capture_output=True,
            text=True,
            timeout=120,
        )
    except (OSError, subprocess.SubprocessError) as error:
        return 1, f"{type(error).__name__}: {error}"
    output = ((result.stdout or "") + (result.stderr or "")).strip()
    return int(result.returncode), output


def gh_json(args: Sequence[str], runner: GhRunner) -> Any:
    """跑一条 `gh ... --json`，把 stdout 解析成 JSON；失败返回 `None`。"""
    code, output = runner(list(args))
    if code != 0 or not output:
        return None
    try:
        return json.loads(output)
    except (TypeError, ValueError):
        return None


def resolve_build_job(
    item: dict[str, Any],
    *,
    tag: str,
    repo: str,
    runner: GhRunner = run_gh,
) -> tuple[str, str]:
    """定位 `build.yml` 中该 tag 的 run 与目标 job，返回 `(run_id, job_id)`；找不到返回空串。

    §8.4 的做法：`gh run list --workflow=build.yml` 里按 `headBranch == tag` 找这次发版的
    run（`build.yml` 由 tag 触发，所以 `headBranch` 就是 tag），再用 api 列出 jobs，
    挑**失败且名字命中**（D → `release`，B → `publish`/`npm`/`package`）的那个。

    找不到（run 超过 `gh run list` 的窗口 / job 没失败过）时返回空串，由调用方记
    `unresolved` 并交给 `issue.sh` 升级成 issue（§8.1 B 的「无 run → 升级为 issue」）。
    """
    runs = gh_json(
        [
            "gh",
            "run",
            "list",
            "--workflow=build.yml",
            "--branch",
            tag,
            "--limit",
            "20",
            "--json",
            "databaseId,headBranch,conclusion,runAttempt",
            "--repo",
            repo,
        ],
        runner,
    )
    if not isinstance(runs, list):
        return "", ""
    build_run_id = ""
    for run in runs:
        if str(run.get("headBranch")) == tag:
            build_run_id = str(run.get("databaseId"))
            break
    if not build_run_id:
        return "", ""
    jobs = gh_json(
        ["gh", "api", f"/repos/{repo}/actions/runs/{build_run_id}/jobs", "--paginate"],
        runner,
    )
    if not isinstance(jobs, dict):
        return build_run_id, ""
    hints = BUILD_JOB_HINTS.get(str(item.get("category") or ""), ("release",))
    for job in jobs.get("jobs") or []:
        name = str(job.get("name") or "").lower()
        if job.get("conclusion") == "failure" and any(hint in name for hint in hints):
            return build_run_id, str(job.get("id"))
    return build_run_id, ""


def execute(
    selected: Sequence[dict[str, Any]],
    *,
    tag: str,
    repo: str,
    run_id: str,
    dry_run: bool,
    runner: GhRunner = run_gh,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """执行选中的项；返回 `(executed, unresolved)`。

    - `dry_run=True`：**一条 gh 命令都不发**（包括只读的 job 定位），只填 `command`：
      `rerun-release` 的命令里 build job id 写成 `<待解析>`，并在 `note` 里注明
      「执行时才解析」——演练路径必须零副作用，哪怕只是 GET；
    - `rerun-release` 定位不到 job：记 `unresolved`（带 `issueTitle`，交给 `issue.sh`
      升级为 issue）；
    - 命令非 0 退出：记 `unresolved`（同样带 `issueTitle`），因为「处置没发出去」等于没处置。

    `unresolved` 的 `issueTitle` 用 `classify.py` 的 `issue_title()` 同源生成（DEF-3）：
    没有标题时 `issue.sh` 过去会直接 `continue`，这类「处置失败」就被静默丢掉了。
    """
    executed: list[dict[str, Any]] = []
    unresolved: list[dict[str, Any]] = []
    for item in selected:
        build_run_id = ""
        build_job_id = ""
        needs_build_job = item.get("action") == "rerun-release"
        if needs_build_job and not dry_run:
            # 定位要发 2 条只读 GET，所以**只在真执行时**做（dry_run 必须零调用，DEF-2）。
            build_run_id, build_job_id = resolve_build_job(item, tag=tag, repo=repo, runner=runner)
            if not build_job_id:
                unresolved.append(
                    {
                        **_suppressed(
                            item,
                            "定位不到 build.yml 里该 tag 的失败 job"
                            "（§8.1 B/D 的「无 run → 升级为 issue」）",
                        ),
                        "buildRunId": build_run_id or None,
                        "issueTitle": issue_title_for(tag, item),
                    }
                )
                continue
        command = build_command(
            item,
            tag=tag,
            repo=repo,
            run_id=run_id,
            build_run_id=build_run_id,
            build_job_id=build_job_id or (BUILD_JOB_PLACEHOLDER if needs_build_job else ""),
        )
        record = {
            "form": item.get("form"),
            "cell": item.get("cell"),
            "category": item.get("category"),
            "action": item.get("action"),
            "command": command,
            "buildRunId": build_run_id or None,
            "buildJobId": build_job_id or None,
            "dryRun": bool(dry_run),
            "note": (
                f"rerun-release 的 build job id 用 {BUILD_JOB_PLACEHOLDER} 占位："
                "dry_run 不解析（零 gh 调用），执行时才解析"
                if needs_build_job and dry_run
                else ""
            ),
            "exit": None,
            "output": "",
        }
        if not dry_run:
            code, output = runner(command)
            record["exit"] = code
            record["output"] = output[-2000:]
            if code != 0:
                unresolved.append(
                    {
                        **_suppressed(item, f"处置命令失败（exit={code}）：{output[:400]}"),
                        "failedCommand": command,
                        "issueTitle": issue_title_for(tag, item),
                    }
                )
        executed.append(record)
    return executed, unresolved


# ---------------------------------------------------------------------------
# 自测
# ---------------------------------------------------------------------------


def _item(form: str, cell: str, category: str, action: str) -> dict[str, Any]:
    """造一个计划项（自测用）。"""
    return {"form": form, "cell": cell, "category": category, "action": action, "status": "fail"}


def selftest() -> int:
    """离线自测：优先级、上限、attempt 闸门、每类一次、dry-run、命令白名单。"""
    problems: list[str] = []

    items = [
        _item("npm", "install", "B", "rerun-release"),
        _item("linux", "install", "D", "rerun-release"),
        _item("docker", "pull", "C", "dispatch-docker"),
        _item("windows", "install", "A", "rerun-job"),
        _item("macos", "startup", "F", "issue"),
        _item("linux", "startup", "J", "none"),
    ]
    selected, suppressed = plan_dispositions(items, run_attempt=1)
    order = [item["category"] for item in selected]
    if order != ["A", "C", "D"]:
        problems.append(f"优先级应为 A>C>D 且上限 3，实际 {order}")
    if not any("上限 3" in item["reason"] for item in suppressed if item["category"] == "B"):
        problems.append("超出上限的 B 未记「上限」理由")
    if any(item["action"] == "issue" for item in selected):
        problems.append("issue 类不该进自动处置")

    # attempt >= 2：一条都不选。
    selected, suppressed = plan_dispositions(items, run_attempt=2)
    if selected:
        problems.append(f"run_attempt=2 仍选了 {[i['category'] for i in selected]}")
    if not all("run_attempt" in item["reason"] for item in suppressed):
        problems.append("attempt 闸门的理由文案不对")
    if len(suppressed) != 4:
        problems.append(f"attempt=2 时被抑制项应为 4 个（A/C/D/B），实际 {len(suppressed)}")

    # 每类每版本 ≤ 1：A 已用过 → 不再选 A，顺延到 C。
    selected, suppressed = plan_dispositions(items, run_attempt=1, used_categories=("A",))
    if [item["category"] for item in selected] != ["C", "D", "B"]:
        problems.append(f"used_categories 未生效：{[i['category'] for i in selected]}")
    if not any(item["category"] == "A" and "已处置过" in item["reason"] for item in suppressed):
        problems.append("已用过的类别未记「已处置过」")

    # 同类多项：只选第一个，其余抑制（每类每版本 ≤1 的 run 内版本）。
    duplicated = [
        _item("windows", "install", "A", "rerun-job"),
        _item("macos", "install", "A", "rerun-job"),
    ]
    selected, suppressed = plan_dispositions(duplicated, run_attempt=1)
    if len(selected) != 1 or not suppressed:
        problems.append("同类多项未收敛成 1 次处置")

    # limit=0 → 全抑制（演练用）。
    selected, _ = plan_dispositions(items, run_attempt=1, limit=0)
    if selected:
        problems.append("limit=0 仍选了处置项")

    # 命令白名单：合法动作可翻译，非法动作抛错。
    cmd = build_command(_item("docker", "pull", "C", "dispatch-docker"), tag="v0.2.5", repo="o/r", run_id="1", build_run_id="", build_job_id="")
    if cmd[:4] != ["gh", "workflow", "run", "docker.yml"] or "v0.2.5" not in cmd:
        problems.append(f"dispatch-docker 命令不符：{cmd}")
    if "-f" not in cmd or "push_image=true" not in cmd:
        problems.append("dispatch-docker 缺少 `-f push_image=true`")
    cmd = build_command(_item("windows", "install", "A", "rerun-job"), tag="v0.2.5", repo="o/r", run_id="77", build_run_id="", build_job_id="")
    if cmd != ["gh", "run", "rerun", "77", "--failed"]:
        problems.append(f"rerun-job 命令不符：{cmd}")
    cmd = build_command(_item("linux", "install", "D", "rerun-release"), tag="v0.2.5", repo="o/r", run_id="1", build_run_id="9", build_job_id="42")
    if cmd[-1] != "/repos/o/r/actions/jobs/42/rerun" or "-X" not in cmd:
        problems.append(f"rerun-release 命令不符：{cmd}")
    for bad_action in ("issue", "none", "gh release delete", "push"):
        try:
            build_command(_item("linux", "install", "D", bad_action), tag="v0.2.5", repo="o/r", run_id="1", build_run_id="", build_job_id="")
            problems.append(f"非法动作 {bad_action!r} 未被拒绝")
        except ValueError:
            pass

    # dry_run：不调用 runner，但仍填好 command。
    calls: list[Sequence[str]] = []

    def fake_runner(args: Sequence[str]) -> tuple[int, str]:
        calls.append(list(args))
        return 0, ""

    executed, unresolved = execute(
        [_item("windows", "install", "A", "rerun-job")],
        tag="v0.2.5",
        repo="o/r",
        run_id="88",
        dry_run=True,
        runner=fake_runner,
    )
    if calls:
        problems.append("dry_run 仍然调用了 runner（有副作用）")
    if len(executed) != 1 or executed[0]["command"] != ["gh", "run", "rerun", "88", "--failed"]:
        problems.append(f"dry_run 未记录将要执行的命令：{executed}")
    if unresolved:
        problems.append("dry_run 不该产生 unresolved")

    # 真执行（假 runner）：成功进 executed，失败进 unresolved（「处置没发出去」= 要开 issue）。
    executed, unresolved = execute(
        [_item("windows", "install", "A", "rerun-job")],
        tag="v0.2.5",
        repo="o/r",
        run_id="88",
        dry_run=False,
        runner=lambda args: (0, "ok"),
    )
    if len(executed) != 1 or executed[0]["exit"] != 0 or unresolved:
        problems.append("成功执行未记进 executed")
    executed, unresolved = execute(
        [_item("windows", "install", "A", "rerun-job")],
        tag="v0.2.5",
        repo="o/r",
        run_id="88",
        dry_run=False,
        runner=lambda args: (1, "HTTP 403"),
    )
    if not unresolved:
        problems.append("处置命令失败未记 unresolved（会静默丢掉一次重跑）")

    # rerun-release 定位不到 job → unresolved，而不是发一条没有目标 id 的 api 调用。
    def runner_without_build(args: Sequence[str]) -> tuple[int, str]:
        if "list" in args:
            return 0, "[]"
        return 1, "not found"

    executed, unresolved = execute(
        [_item("linux", "install", "D", "rerun-release")],
        tag="v0.2.5",
        repo="o/r",
        run_id="1",
        dry_run=False,
        runner=runner_without_build,
    )
    if executed or not unresolved:
        problems.append("定位不到 build job 时应记 unresolved 且不执行")

    # DEF-2：dry_run 下 rerun-release 也必须 0 次 runner 调用（过去定位在 dry_run 判定之前）。
    dry_calls: list[Sequence[str]] = []

    def recording_runner(args: Sequence[str]) -> tuple[int, str]:
        dry_calls.append(list(args))
        return 0, "[]"

    executed, unresolved = execute(
        [_item("linux", "install", "D", "rerun-release")],
        tag="v0.2.5",
        repo="o/r",
        run_id="1",
        dry_run=True,
        runner=recording_runner,
    )
    if dry_calls:
        problems.append(f"dry_run 下 rerun-release 仍调用了 runner（{len(dry_calls)} 次）")
    if unresolved:
        problems.append("dry_run 下 rerun-release 不该产生 unresolved")
    if len(executed) != 1:
        problems.append(f"dry_run 下 rerun-release 应记录 1 条将执行的命令：{executed}")
    else:
        record = executed[0]
        if BUILD_JOB_PLACEHOLDER not in " ".join(record["command"]):
            problems.append(f"dry_run 的 rerun-release 命令缺 {BUILD_JOB_PLACEHOLDER} 占位：{record}")
        if "执行时才解析" not in str(record.get("note") or ""):
            problems.append(f"dry_run 的 rerun-release 记录没注明「执行时才解析」：{record}")

    # DEF-3：unresolved 必须自带 issueTitle（同源 classify.issue_title），否则 issue.sh 会丢弃。
    if not unresolved and executed:
        executed, unresolved = execute(
            [_item("linux", "install", "D", "rerun-release")],
            tag="v0.2.5",
            repo="o/r",
            run_id="1",
            dry_run=False,
            runner=runner_without_build,
        )
    if not unresolved:
        problems.append("DEF-3 自测前提失败：应产生 unresolved")
    elif unresolved[0].get("issueTitle") != "[release-smoke] v0.2.5 验收失败：linux / install":
        problems.append(f"unresolved 的 issueTitle 与 classify.issue_title 不同源：{unresolved[0]}")
    _, failed_unresolved = execute(
        [_item("windows", "startup", "F", "rerun-job")],
        tag="v0.2.5",
        repo="o/r",
        run_id="88",
        dry_run=False,
        runner=lambda args: (1, "HTTP 403"),
    )
    if not failed_unresolved or not failed_unresolved[0].get("issueTitle"):
        problems.append(f"命令失败产生的 unresolved 也必须有 issueTitle：{failed_unresolved}")

    # DEF-7：白名单外的动作进 rejected（不再静默丢弃），且不进 selected。
    weird = [
        _item("linux", "install", "D", "release-delete"),
        _item("npm", "install", "B", "npm-publish"),
        _item("macos", "install", "F", "issue"),
        _item("linux", "startup", "J", "none"),
    ]
    rejected = reject_items(weird)
    if len(rejected) != 2:
        problems.append(f"白名单外动作应记 2 条 rejected（issue/none 不算）：{rejected}")
    if {item["action"] for item in rejected} != {"release-delete", "npm-publish"}:
        problems.append(f"rejected 的动作不对：{rejected}")
    if not all("白名单" in item["reason"] for item in rejected):
        problems.append(f"rejected 的理由没写清原因：{rejected}")
    selected, _ = plan_dispositions(weird, run_attempt=1)
    if selected:
        problems.append(f"白名单外动作不该被选中执行：{selected}")
    if reject_items([_item("windows", "install", "A", "rerun-job")]):
        problems.append("合法动作被误判为 rejected")

    for message in problems:
        print(f"SELFTEST-FAIL {message}")
    if problems:
        return 1
    print("self-test passed")
    return 0


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def render_text(report: dict[str, Any]) -> str:
    """人读的执行报告（stdout / step summary）。"""
    lines = [
        f"### 处置执行 · **{'dry_run' if report['dryRun'] else '已执行'}**"
        f" · run attempt `{report['runAttempt']}` · 上限 {report['limit']}",
        "",
    ]
    if report["executed"]:
        lines += [
            "| 形态 | 格 | 类别 | 动作 | 命令 | exit |",
            "| --- | --- | --- | --- | --- | --- |",
        ]
        for record in report["executed"]:
            command = " ".join(record["command"])
            lines.append(
                f"| `{record['form']}` | `{record['cell']}` | {record['category']} "
                f"| `{record['action']}` | `{command}` | {record['exit'] if record['exit'] is not None else '—'} |"
            )
    else:
        lines.append("- 没有执行任何处置（无需处置 / 被闸门抑制 / dry_run）。")
    if report["suppressed"]:
        lines += ["", "**已抑制**："]
        for item in report["suppressed"]:
            lines.append(f"- `{item['form']}/{item['cell']}` {item['category']}：{item['reason']}")
    if report["unresolved"]:
        lines += ["", "**未完成（将升级为 issue）**："]
        for item in report["unresolved"]:
            lines.append(f"- `{item['form']}/{item['cell']}` {item['category']}：{item['reason']}")
    notes = [record for record in report["executed"] if record.get("note")]
    if notes:
        lines += ["", "**说明**："]
        for record in notes:
            lines.append(f"- `{record['form']}/{record['cell']}`：{record['note']}")
    if report.get("rejected"):
        lines += ["", "**拒绝执行（动作不在白名单）**："]
        for item in report["rejected"]:
            lines.append(f"- `{item['form']}/{item['cell']}` `{item['action']}`：{item['reason']}")
    return "\n".join(lines) + "\n"


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="dispose.py",
        description="按 triage-plan.json 执行白名单处置（带 attempt / 次数 / 优先级闸门）",
    )
    parser.add_argument("--plan", default=os.path.join("artifacts", "release-smoke", "triage-plan.json"))
    parser.add_argument("--out", default=os.path.join("artifacts", "release-smoke", "dispose.json"))
    parser.add_argument("--tag", default=os.environ.get("SMOKE_VERSION", ""))
    parser.add_argument("--repo", default=os.environ.get("GH_REPO") or os.environ.get("GITHUB_REPOSITORY") or "")
    parser.add_argument("--run-id", default=os.environ.get("GITHUB_RUN_ID") or "")
    parser.add_argument("--run-attempt", type=int, default=int(os.environ.get("GITHUB_RUN_ATTEMPT") or 1))
    parser.add_argument(
        "--used-categories",
        default="",
        help="本版本已经用过的类别（逗号分隔，例如 A,C）——由上一次结论区块解析而来",
    )
    parser.add_argument("--limit", type=int, default=DEFAULT_LIMIT, help="单 run 处置上限（默认 3）")
    parser.add_argument("--dry-run", action="store_true", help="只打印计划，不执行任何 gh 命令")
    parser.add_argument("--selftest", action="store_true", help="离线自测")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    try:
        with open(args.plan, encoding="utf-8") as handle:
            plan = json.load(handle)
    except (OSError, ValueError) as error:
        print(f"ERROR 计划不可读：{error}", file=sys.stderr)
        return 1
    if not isinstance(plan, dict):
        print("ERROR 计划必须是 JSON 对象", file=sys.stderr)
        return 1

    tag = (args.tag or plan.get("tag") or "").strip()
    repo = (args.repo or "").strip()
    run_id = (args.run_id or plan.get("runId") or "").strip()
    run_attempt = max(1, args.run_attempt or int(plan.get("runAttempt") or 1))
    used = [item.strip() for item in (args.used_categories or "").split(",") if item.strip()]

    items = [item for item in (plan.get("items") or []) if isinstance(item, dict)]
    selected, suppressed = plan_dispositions(items, run_attempt, used, args.limit)
    rejected = reject_items(items)

    if not tag or not repo:
        # 缺 tag/repo 时下发命令必然失败（还可能打到错误仓库），所以只做 dry-run 式的记录。
        print("::warning::缺 --tag 或 --repo：本次不做任何处置（只记录计划）", file=sys.stderr)
        selected = []

    executed, unresolved = execute(
        selected,
        tag=tag,
        repo=repo,
        run_id=run_id,
        dry_run=bool(args.dry_run),
        runner=run_gh,
    )

    report = {
        "schemaVersion": SCHEMA_VERSION,
        "generatedAt": _now(),
        "tag": tag,
        "runId": run_id or None,
        "runAttempt": run_attempt,
        "dryRun": bool(args.dry_run),
        "limit": args.limit,
        "usedCategories": used,
        "executed": executed,
        "suppressed": suppressed,
        "unresolved": unresolved,
        "rejected": rejected,
    }
    _write(args.out, report)
    text = render_text(report)
    print(text)

    for record in executed:
        if record["dryRun"]:
            suffix = f"（{record['note']}）" if record.get("note") else ""
            print(f"::notice::[dry_run] 将执行：{' '.join(record['command'])}{suffix}")
        elif record["exit"] == 0:
            print(f"::notice::已执行：{' '.join(record['command'])}")
    for item in unresolved:
        print(f"::warning::处置未完成（{item['form']}/{item['cell']} {item['category']}）：{item['reason']}")
    for item in rejected:
        print(
            f"::warning::拒绝执行白名单外动作（{item['form']}/{item['cell']} {item['action']!r}）："
            f"{item['reason']}"
        )
    return 0


def _now() -> str:
    """ISO 8601 UTC（与 common.py 同格式；此处不 import common，避免 dispose 对契约产生依赖）。"""
    import time

    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def _write(path: str, report: dict[str, Any]) -> None:
    parent = os.path.dirname(os.path.abspath(path))
    if parent:
        os.makedirs(parent, exist_ok=True)
    tmp = f"{path}.tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(report, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    os.replace(tmp, path)


if __name__ == "__main__":
    sys.exit(main())
