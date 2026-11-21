#!/usr/bin/env python3
"""发版后验收（release-smoke）的预检：一个 job 把「能不能验」先判掉。

设计依据：`delivery/design-release-smoke.md` §4.1（preflight job）、§8.1 类别 D（资产缺失）、
§9.1–§9.2（结果契约与其单一来源）。

做六件事，任何一件是 `error` 级失败都会让本 job 失败（fail-closed）：

1. **tag → 版本**：`--tag` 优先，其次 `$GITHUB_REF_NAME`，再退 `$GITHUB_REF`；`v` 前缀统一去掉；
2. **本地版本一致性**：`Cargo.toml` 的 `[workspace.package] version`、根 `package.json`、
   `npm/package.json`（含 5 个平台分包的 `optionalDependencies`）、`src-tauri/tauri.conf.json`
   必须全部等于 tag 版本——不一致时后续每一格都会写出「版本不一致（H）」的假失败；
3. **Release 存在且是正式版**：`draft` / `prerelease` 直接失败（PRD P0-1 只验正式版；workflow 侧
   还有一层 `if`，这里是第二道，供 `workflow_dispatch` 演练时同样受约束）；
4. **必需资产齐全**：8 份清单 + 2 个 dmg + 参与自动更新的 5 个包及其 `.sig`。清单常量**复用**
   `scripts/check-release-assets.py` 的 `REQUIRED_MANIFESTS`（不复制第二份真源）；
5. **上一版 tag**：`git tag --sort=-v:refname` 里当前 tag 之后第一个**正式版**；没有上一版时
   `prevVersion=null` + `hasPrev=false`，**不算失败**（首次发版）；
6. **`is_latest`**：`gh release view <tag> --json isLatest` 判断「应用内更新的 `releases/latest`
   端点会不会指向这个 tag」。历史回填（对老 tag 重跑验收）时它通常是 `false`——桌面更新器把
   端点写死在 `releases/latest`，**无法定向升到 vX**，此时 Windows 升级格必须标 `skipped`
   而不是判失败。拿不到（离线 / 无 `gh`）时写 `null`，调用方按 `true` 处理（照旧跑）；
7. **GHCR 可探测**：`:<version>` 与 `:latest` 的 manifests。走匿名 token（不需要本机 Docker、
   也不需要 `docker login`），私有包回落到 `GH_TOKEN`。探测不到只降级为 `warn`——镜像可能还没
   推完，容器格会自己如实报 C 类。

版本号解析、`$GITHUB_OUTPUT` / `$GITHUB_STEP_SUMMARY` 写入、HTTP 探测全部**转发**
`scripts/smoke/common.py`（唯一真源，设计 §9.2），本文件不重复实现。

输出
----
- `--out`（默认 `artifacts/release-smoke/preflight.json`）：给 triage / 结论用的预检快照。
  **不是** `<form>.json 结果契约，不要喂给 `render-conclusion.py --gate`（它只认 form 契约）；
- `$GITHUB_OUTPUT`：`version` / `prev_version` / `has_prev` / `is_latest` / `ok` /
  `missing_assets` / `preflight_json`；`is_latest` 的 `null` 写成**空串**（不是 `false`），
  下游（`win/driver.ps1` 的 `SMOKE_IS_LATEST`）只认字符串 `false` 才是「不能定向升级」；
- `$GITHUB_STEP_SUMMARY`：一张「检查项 / 结果 / 说明」表。

用法
----
    python3 scripts/smoke/preflight.py --tag v0.2.5              # CI（token 由 GitHub 注入）
    python3 scripts/smoke/preflight.py --tag v0.2.5 --network off # 本地只验版本 / 上一版 tag
    python3 scripts/smoke/preflight.py --selftest                 # 离线自测
"""

from __future__ import annotations

import argparse
import base64
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Sequence

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(SCRIPT_DIR, "..", ".."))

SCHEMA_VERSION = 1

#: 预检项严重级别；只有 `error` 会让本 job 失败。
SEVERITIES: tuple[str, ...] = ("error", "warn", "info")

#: 用户可见安装包（不参与自动更新，因此不要求 `.sig`）：两个 dmg 是 macOS 的唯一人用形态
#: （`pkg-upgrade` 用它覆盖安装）。
DMG_TEMPLATE = "workbuddy-shift_{version}_{arch}.dmg"
DMG_ARCHES: tuple[str, ...] = ("aarch64", "x86_64")

#: 参与自动更新的包 → 必须有同名 `.sig`（规则与 `check-release-assets.py` 的 `has_sig` 一致）。
SIGNED_ASSET_TEMPLATES: tuple[str, ...] = (
    "workbuddy-shift_{version}_x64-setup.exe",
    "workbuddy-shift-macos-aarch64.app.tar.gz",
    "workbuddy-shift-macos-x86_64.app.tar.gz",
    "workbuddy-shift-linux-x86_64.deb",
    "workbuddy-shift-linux-x86_64.AppImage",
)

#: 8 份清单资产：运行时从 `scripts/check-release-assets.py` 读，这里只是读不到时的兜底副本
#: （两份不一致时 `selftest` 会报错，见 [`MANIFEST_ASSETS`]）。
FALLBACK_MANIFESTS: tuple[str, ...] = (
    "latest.json",
    "latest-windows-x86_64.json",
    "latest-macos-aarch64.json",
    "latest-darwin-aarch64.json",
    "latest-macos-x86_64.json",
    "latest-darwin-x86_64.json",
    "latest-linux-x86_64.json",
    "latest-linux-deb-x86_64.json",
)

#: 本地必须与 tag 版本一致的文件（`path` 相对仓库根，`label` 进报告）。
VERSION_SOURCES: tuple[tuple[str, str], ...] = (
    ("package.json", "packageJson"),
    ("npm/package.json", "npm"),
    ("src-tauri/tauri.conf.json", "tauriConf"),
)

#: GHCR 镜像名（`owner/repo` 形态，与 `GITHUB_REPOSITORY` 同构）。
GHCR_IMAGE = "magicapple123/workbuddy-shift"

#: GHCR 的 manifests Accept 头：OCI index / docker manifest list / 单平台 manifest 都要认。
GHCR_ACCEPT = (
    "application/vnd.oci.image.index.v1+json,"
    "application/vnd.docker.distribution.manifest.list.v2+json,"
    "application/vnd.docker.distribution.manifest.v2+json"
)

_TAG_RE = re.compile(r"^v?(\d+\.\d+\.\d+(?:-[0-9A-Za-z.\-]+)?)$")


class PreflightError(RuntimeError):
    """预检无法继续（缺文件 / 参数非法 / 网络不可用）。CI 里直接失败，不猜。"""


def _load_by_path(module_name: str, path: str) -> Any:
    """按路径加载模块（兼容文件名带 `-` 的脚本与同目录的 `common.py`）。"""
    if not os.path.exists(path):
        raise PreflightError(f"缺少依赖模块：{path}")
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise PreflightError(f"无法加载模块：{path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


#: 契约唯一真源：版本解析、GitHub 输出、HTTP 探测都从这里来。
common = _load_by_path("wb_smoke_common", os.path.join(SCRIPT_DIR, "common.py"))


def _load_manifest_assets() -> tuple[str, ...]:
    """复用 `scripts/check-release-assets.py` 的 `REQUIRED_MANIFESTS`；读不到就用兜底副本。"""
    path = os.path.join(REPO_ROOT, "scripts", "check-release-assets.py")
    try:
        module = _load_by_path("wb_check_release_assets", path)
    except PreflightError:
        return FALLBACK_MANIFESTS
    manifests = getattr(module, "REQUIRED_MANIFESTS", None)
    if isinstance(manifests, list) and manifests:
        return tuple(str(name) for name in manifests)
    return FALLBACK_MANIFESTS


MANIFEST_ASSETS: tuple[str, ...] = _load_manifest_assets()


# ---------------------------------------------------------------------------
# tag / 版本（解析转发 common.py）
# ---------------------------------------------------------------------------


def parse_version(value: str | None) -> tuple[int, int, int, str]:
    """`X.Y.Z[-pre]` → 四元组；非法版本抛 :class:`PreflightError`（不猜）。"""
    try:
        return common.parse_version(value)
    except common.SmokeContractError as error:
        raise PreflightError(str(error)) from error


def resolve_tag(explicit: str | None) -> tuple[str, str]:
    """返回 `(tag, version)`；`--tag` → `$GITHUB_REF_NAME` → `$GITHUB_REF`。"""
    candidates = [explicit, os.environ.get("GITHUB_REF_NAME")]
    ref = os.environ.get("GITHUB_REF") or ""
    if ref.startswith("refs/tags/"):
        candidates.append(ref[len("refs/tags/") :])
    for candidate in candidates:
        text = (candidate or "").strip()
        if not text:
            continue
        match = _TAG_RE.match(text)
        if not match:
            raise PreflightError(
                f"tag 不像发版 tag：{text!r}（期望 vX.Y.Z，可带 -rc.N 预发布后缀）"
            )
        version = match.group(1)
        return (text if text.startswith("v") else f"v{version}", version)
    raise PreflightError("没有拿到 tag：传 --tag，或设置 GITHUB_REF_NAME / GITHUB_REF")


def repo_slug(explicit: str | None) -> str:
    """`owner/repo`：`--repo` → `$GITHUB_REPOSITORY` → 本仓库默认值（与 `GHCR_IMAGE` 同源）。"""
    for candidate in (explicit, os.environ.get("GITHUB_REPOSITORY")):
        text = (candidate or "").strip()
        if "/" in text:
            return text
    return GHCR_IMAGE


def is_prerelease_version(version: str) -> bool:
    """`0.2.1-rc.1` 这类带后缀的版本是预发布。"""
    return bool(parse_version(version)[3])


# ---------------------------------------------------------------------------
# 本地版本一致性
# ---------------------------------------------------------------------------


def workspace_cargo_version() -> str | None:
    """从 `Cargo.toml` 的 `[workspace.package]` 段读 `version`。"""
    path = os.path.join(REPO_ROOT, "Cargo.toml")
    if not os.path.exists(path):
        return None
    in_section = False
    with open(path, encoding="utf-8") as handle:
        for raw in handle:
            line = raw.strip()
            if line.startswith("["):
                in_section = line.replace(" ", "") == "[workspace.package]"
                continue
            if not in_section:
                continue
            match = re.match(r'^version\s*=\s*"([^"]+)"', line)
            if match:
                return match.group(1)
    return None


def _read_json(rel_path: str) -> Any:
    with open(os.path.join(REPO_ROOT, rel_path), encoding="utf-8") as handle:
        return json.load(handle)


def check_local_versions(version: str) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    """本地版本与 tag 版本逐项比对；返回 `(checks, 各来源的实测值)`。"""
    actual: dict[str, Any] = {}
    checks: list[dict[str, Any]] = []

    cargo_version = workspace_cargo_version()
    actual["cargo"] = cargo_version
    checks.append(
        _check(
            "version.cargo",
            cargo_version == version,
            f"Cargo.toml [workspace.package].version={cargo_version!r}，tag={version}",
        )
    )

    for rel_path, label in VERSION_SOURCES:
        if not os.path.exists(os.path.join(REPO_ROOT, rel_path)):
            actual[label] = None
            checks.append(_check(f"version.{label}", False, f"缺少 {rel_path}"))
            continue
        value = _read_json(rel_path).get("version")
        actual[label] = value
        checks.append(
            _check(
                f"version.{label}",
                common.normalize_version(value) == version,
                f"{rel_path} version={value!r}，tag={version}",
            )
        )

    # 5 个 npm 平台分包版本必须跟主包一起走，否则 N1 那一格必然 404。
    platforms: dict[str, Any] = {}
    if os.path.exists(os.path.join(REPO_ROOT, "npm/package.json")):
        platforms = _read_json("npm/package.json").get("optionalDependencies") or {}
    actual["npmPlatforms"] = platforms
    drifted = sorted(
        name for name, value in platforms.items() if common.normalize_version(value) != version
    )
    checks.append(
        _check(
            "version.npmPlatforms",
            bool(platforms) and not drifted,
            f"npm 平台分包 {len(platforms)} 个，版本漂移：{drifted}"
            if drifted or not platforms
            else f"npm 平台分包 {len(platforms)} 个均为 {version}",
        )
    )
    return checks, actual


# ---------------------------------------------------------------------------
# 上一版 tag
# ---------------------------------------------------------------------------


def previous_stable_tag(tags: Sequence[str], current_tag: str, version: str) -> str | None:
    """当前 tag 之后第一个**正式版** tag；没有则 None（首次发版）。

    预发布 tag（`-rc.N`）不作为升级起点：验收要比的是「用户手上那一版」，rc 版不进 Release 的
    正式资产清单。
    """
    current = common.normalize_version(current_tag)
    passed_current = False
    for tag in tags:
        candidate = common.normalize_version(tag)
        try:
            parsed = parse_version(candidate)
        except PreflightError:
            continue
        if not passed_current:
            passed_current = candidate == current
            continue
        if parsed[3]:
            continue
        return tag if tag.startswith("v") else f"v{candidate}"
    # 当前 tag 不在列表里（例如本地还没 fetch 到）：退化为「比 version 小的最大正式版」。
    target = parse_version(version)
    best: str | None = None
    for tag in tags:
        candidate = common.normalize_version(tag)
        try:
            parsed = parse_version(candidate)
        except PreflightError:
            continue
        if parsed[3] or parsed[:3] >= target[:3]:
            continue
        if best is None or parse_version(best)[:3] < parsed[:3]:
            best = tag
    return best


def git_tags() -> list[str]:
    """按 `--sort=-v:refname` 取全部 tag（等价 `git tag --sort=-v:refname | ...`）。"""
    try:
        result = subprocess.run(
            ["git", "tag", "--sort=-v:refname"],
            cwd=REPO_ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise PreflightError(f"git tag 读取失败：{error}") from error
    return [line.strip() for line in result.stdout.splitlines() if line.strip()]


# ---------------------------------------------------------------------------
# 网络：Release / 资产 / GHCR
# ---------------------------------------------------------------------------


def _token() -> str | None:
    for name in ("GH_TOKEN", "GITHUB_TOKEN"):
        value = (os.environ.get(name) or "").strip()
        if value:
            return value
    return None


def _api_json(url: str, headers: dict[str, str]) -> tuple[int, Any]:
    """GET 一个 JSON 端点，返回 `(status, body)`；网络不可用抛 :class:`PreflightError`。

    复用 `common.http_probe`（不抛异常、统一超时），这里只把 body 解析成 JSON。
    """
    probe = common.http_probe(url, expect_status=(200,), headers=headers, want_body=True)
    status = probe.get("status")
    if status is None:
        raise PreflightError(f"请求 {url} 失败：{probe.get('error')}")
    body = probe.get("body") or ""
    try:
        return int(status), (json.loads(body) if body else {})
    except (TypeError, ValueError):
        return int(status), {"raw": body[:400]}


def fetch_release(repo: str, tag: str) -> dict[str, Any]:
    """读 Release 元数据（含 assets）。"""
    token = _token()
    if not token:
        raise PreflightError("缺少 GH_TOKEN / GITHUB_TOKEN，无法查询 Release")
    status, body = _api_json(
        f"https://api.github.com/repos/{repo}/releases/tags/{tag}",
        {"Authorization": f"Bearer {token}"},
    )
    if status == 404:
        raise PreflightError(f"Release 不存在：{repo} {tag}")
    if status != 200 or not isinstance(body, dict):
        raise PreflightError(f"读取 Release 失败：HTTP {status} {str(body)[:200]}")
    return body


def fetch_is_latest(repo: str, tag: str) -> bool | None:
    """`gh release view <tag> --json isLatest` → `True` / `False` / `None`（拿不到）。

    为什么不用 REST：`/releases/tags/<tag>` 的响应里**没有** `is_latest` 字段，该字段只在
    `gh` 的 `--json isLatest` 视图（内部读 `/releases/latest` 比对）里存在。拿不到时返回
    `None` 而不是 `False`——**「不知道」和「不是 latest」是两件事**，后者要让 Windows 升级格
    跳过，前者按 `true` 处理（照旧跑，宁可多验一次）。
    """
    if not shutil.which("gh"):
        return None
    try:
        result = subprocess.run(
            ["gh", "release", "view", tag, "--json", "isLatest", "--repo", repo],
            cwd=REPO_ROOT,
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    try:
        payload = json.loads(result.stdout or "{}")
    except (TypeError, ValueError):
        return None
    value = payload.get("isLatest") if isinstance(payload, dict) else None
    return value if isinstance(value, bool) else None


def is_latest_check(is_latest: bool | None, tag: str) -> dict[str, Any]:
    """把 `isLatest` 变成一条预检项。

    - `True`（✅）：应用内升级会升到本 tag，Windows 升级格照跑；
    - `False`（⚠️ warn，**不是** error）：历史回填时 `releases/latest` 指向更新的版本，
      桌面更新器无法定向升级 → Windows 升级格标 `skipped`。这既不是产品缺陷也不是流水线
      缺陷，所以不能红；
    - `None`（➖ info）：拿不到就按 `true` 处理，并在结论里说明「未验证」而不是「通过」。
    """
    if is_latest is None:
        return _check(
            "release.isLatest",
            False,
            "拿不到 isLatest（离线 / 无 gh / 查询失败）：按 true 处理，Windows 升级格照跑",
            "info",
        )
    if is_latest:
        return _check("release.isLatest", True, f"{tag} 是 Release 的 latest（应用内升级会升到它）")
    return _check(
        "release.isLatest",
        False,
        f"{tag} 不是 Release 的 latest：更新器端点固定 releases/latest，无法定向升级到它"
        "（Windows 升级格将标 skipped）",
        "warn",
    )


def expected_assets(version: str) -> list[str]:
    """必需资产清单：8 份清单 + 2 个 dmg + 参与自动更新的 5 个包（含 `.sig`）。"""
    names = list(MANIFEST_ASSETS)
    names += [DMG_TEMPLATE.format(version=version, arch=arch) for arch in DMG_ARCHES]
    for template in SIGNED_ASSET_TEMPLATES:
        name = template.format(version=version)
        names += [name, f"{name}.sig"]
    return names


def check_release_flags(release: dict[str, Any]) -> list[dict[str, Any]]:
    """draft / prerelease 都不该被验收。"""
    return [
        _check(
            "release.draft",
            not bool(release.get("draft")),
            "Release 是 draft" if release.get("draft") else "非 draft",
        ),
        _check(
            "release.prerelease",
            not bool(release.get("prerelease")),
            "Release 是 prerelease（只验正式版）"
            if release.get("prerelease")
            else "非 prerelease",
        ),
    ]


def check_assets(release: dict[str, Any], version: str) -> tuple[list[dict[str, Any]], list[str]]:
    """比对资产清单；返回 `(checks, 缺失资产名)`。"""
    assets = release.get("assets") or []
    names = {str(asset.get("name")) for asset in assets}
    required = expected_assets(version)
    missing = [name for name in required if name not in names]
    extra = sorted(names - set(required))
    # 资产没有 digest 时，T03 的 sha256 断言无从比对（D 类），但不阻断验收。
    no_digest = sorted(
        str(asset.get("name", "?"))
        for asset in assets
        if asset.get("name") in required and not asset.get("digest")
    )
    return (
        [
            _check(
                "assets.required",
                not missing,
                f"必需资产 {len(required)} 个，缺失 {len(missing)} 个：{missing}"
                if missing
                else f"必需资产 {len(required)} 个齐全",
            ),
            _check(
                "assets.digest",
                not no_digest,
                f"以下资产没有 digest：{no_digest}" if no_digest else "必需资产均带 digest",
                "warn",
            ),
            _check(
                "assets.extra",
                True,
                f"清单外资产 {len(extra)} 个（不算失败）：{extra}" if extra else "无清单外资产",
                "info",
            ),
        ],
        missing,
    )


def _basic_auth_header() -> dict[str, str]:
    """GHCR 的 Basic 认证：用户名可任意，密码用 token。"""
    token = _token()
    if not token:
        return {}
    raw = base64.b64encode(f"x-access-token:{token}".encode()).decode()
    return {"Authorization": f"Basic {raw}"}


def ghcr_token(image: str) -> str | None:
    """拿一个只读 token：先匿名，失败再用 `GH_TOKEN` 的 Basic 认证换。"""
    scope = urllib.parse.quote(f"repository:{image}:pull", safe=":/-")
    url = f"https://ghcr.io/token?scope={scope}&service=ghcr.io"
    for headers in ({}, _basic_auth_header()):
        try:
            status, body = _api_json(url, headers)
        except PreflightError:
            continue
        if status == 200 and isinstance(body, dict):
            token = body.get("token") or body.get("access_token")
            if token:
                return str(token)
    return None


def ghcr_manifest(image: str, reference: str, token: str | None) -> dict[str, Any]:
    """探测 `:<reference>` 的 manifests；不抛异常，结果进返回值。

    这里直接用 `urllib`（而不是 `common.http_probe`）：要读 `Docker-Content-Digest` 响应头，
    而 probe 的返回值里没有 header。
    """
    headers = {"Accept": GHCR_ACCEPT, "User-Agent": "workbuddy-shift-release-smoke/1"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(f"https://ghcr.io/v2/{image}/manifests/{reference}", method="GET")
    for key, value in headers.items():
        request.add_header(key, value)
    result: dict[str, Any] = {"reference": reference, "status": None, "digest": None, "error": None}
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            result["status"] = int(response.status)
            result["digest"] = response.headers.get("Docker-Content-Digest")
            response.read(1)
    except urllib.error.HTTPError as error:
        result["status"] = int(error.code)
        result["error"] = f"HTTP {error.code}"
    except Exception as error:  # noqa: BLE001 - 探测失败只影响 warn，不该让预检崩掉
        result["error"] = f"{type(error).__name__}: {error}"
    return result


def check_ghcr(image: str, version: str, token: str | None) -> tuple[list[dict[str, Any]], dict]:
    latest = ghcr_manifest(image, "latest", token)
    versioned = ghcr_manifest(image, version, token)
    probes = (latest, versioned)
    present = {item["reference"]: item.get("status") == 200 for item in probes}
    detail = ", ".join(
        f"{item['reference']}={item.get('status')}" + (f"({item['error']})" if item.get("error") else "")
        for item in probes
    )
    # 两个 tag 指向同一 digest 才是「latest 与版本号同步」的证据（K1）。
    same_digest = (
        latest.get("digest") is not None and latest.get("digest") == versioned.get("digest")
    )
    return (
        [
            # 镜像可能晚于 Release 推完；容器格会自己报 C 类，这里只告警。
            _check(
                "ghcr.manifests",
                all(present.values()),
                f"ghcr.io/{image} 探测：{detail}",
                "warn",
            ),
            _check(
                "ghcr.digest",
                same_digest,
                f"latest digest={latest.get('digest')}，{version} digest={versioned.get('digest')}",
                "warn",
            ),
        ],
        {"latest": latest, "version": versioned, "present": present},
    )


# ---------------------------------------------------------------------------
# 汇总 / 输出
# ---------------------------------------------------------------------------


def _check(check_id: str, ok: bool, detail: str, severity: str = "error") -> dict[str, Any]:
    """构造一条预检项；`severity` 只描述**失败时**的严重级别。"""
    if severity not in SEVERITIES:
        raise PreflightError(f"未知严重级别：{severity!r}")
    return {"id": check_id, "ok": bool(ok), "severity": severity, "detail": detail}


def has_error(checks: Sequence[dict[str, Any]]) -> bool:
    return any(item["severity"] == "error" and not item["ok"] for item in checks)


def _is_latest_text(value: Any) -> str:
    """`isLatest` 的三态文案：`true` / `false` / `null`（未知，按 true 处理）。"""
    if value is None:
        return "null（未知，按 true 处理）"
    return "true" if value else "false"


def is_latest_output(value: bool | None) -> str:
    """`$GITHUB_OUTPUT` 里的 `is_latest`：`true` / `false` / **空串**。

    空串是「未知」，不是 `false`：`win/driver.ps1` 只认字符串 `false` 才跳过升级格，
    这样「preflight 没装 gh」不会把 Windows 升级格误标成 skipped。
    """
    if value is None:
        return ""
    return "true" if value else "false"


def render_summary(report: dict[str, Any]) -> str:
    """GitHub step summary / 本地 stdout 共用的一张小表。"""
    lines = [
        f"### release-smoke 预检 · {report['tag']}",
        "",
        f"- 版本：`{report['version']}` · 上一版：`{report['prevTag'] or '—'}`"
        f"（`has_prev={str(report['hasPrev']).lower()}`）",
        f"- 仓库：`{report['repo']}` · 联网检查：`{report['network']}`",
        f"- 是否 latest：`{_is_latest_text(report.get('isLatest'))}`"
        "（`false` = 应用内升级无法定向到本 tag，Windows 升级格标 ⏭）",
        f"- **结论：{'PASS' if report['ok'] else 'FAIL'}**",
        "",
        "| 检查项 | 结果 | 说明 |",
        "| --- | --- | --- |",
    ]
    for item in report["checks"]:
        if item["ok"]:
            mark = "✅"
        elif item["severity"] == "warn":
            mark = "⚠️"
        elif item["severity"] == "info":
            # info 级「不成立」只是事实陈述（例如没有上一版），不是失败。
            mark = "➖"
        else:
            mark = "❌"
        detail = str(item["detail"]).replace("|", "\\|")
        lines.append(f"| `{item['id']}` | {mark} | {detail} |")
    return "\n".join(lines) + "\n"


def write_report(path: str, report: dict[str, Any]) -> None:
    """原子写预检快照（先写 `.tmp` 再 replace）。"""
    parent = os.path.dirname(os.path.abspath(path))
    if parent:
        os.makedirs(parent, exist_ok=True)
    tmp = f"{path}.tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(report, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    os.replace(tmp, path)


# ---------------------------------------------------------------------------
# 主流程
# ---------------------------------------------------------------------------


def run(tag_arg: str | None, repo_arg: str | None, network: str, image: str) -> dict[str, Any]:
    """跑完所有预检项并返回报告（不写文件）。"""
    tag, version = resolve_tag(tag_arg)
    repo = repo_slug(repo_arg)
    checks: list[dict[str, Any]] = []

    local_checks, versions = check_local_versions(version)
    checks += local_checks
    checks.append(
        _check(
            "tag.prerelease",
            not is_prerelease_version(version),
            f"tag {tag} 带预发布后缀，只验正式版"
            if is_prerelease_version(version)
            else f"tag {tag} 是正式版",
        )
    )

    prev_tag = previous_stable_tag(git_tags(), tag, version)
    checks.append(
        _check(
            "tag.previous",
            prev_tag is not None,
            f"上一版 tag={prev_tag}" if prev_tag else "没有上一版（首次发版，升级格将标 skipped）",
            # 首次发版没有上一版是**正常**的：只是 info，不失败。
            "info",
        )
    )

    report: dict[str, Any] = {
        "schemaVersion": SCHEMA_VERSION,
        "tag": tag,
        "version": version,
        "prevTag": prev_tag,
        "prevVersion": common.normalize_version(prev_tag) if prev_tag else None,
        "hasPrev": prev_tag is not None,
        "repo": repo,
        "network": network != "off",
        "isLatest": None,
        "versions": versions,
        "checks": checks,
        "release": None,
        "missingAssets": [],
        "ghcr": None,
    }

    if network == "off":
        checks.append(_check("release.exists", True, "按 --network off 跳过联网检查", "info"))
        checks.append(
            _check("release.isLatest", False, "按 --network off 跳过 isLatest 判定（按 true 处理）", "info")
        )
    else:
        is_latest = fetch_is_latest(repo, tag)
        report["isLatest"] = is_latest
        checks.append(is_latest_check(is_latest, tag))
        try:
            release = fetch_release(repo, tag)
        except PreflightError as error:
            checks.append(_check("release.exists", False, str(error)))
        else:
            assets = release.get("assets") or []
            checks.append(
                _check("release.exists", True, f"Release {tag} 存在（{len(assets)} 个资产）")
            )
            checks += check_release_flags(release)
            asset_checks, missing = check_assets(release, version)
            checks += asset_checks
            report["release"] = {
                "id": release.get("id"),
                "draft": bool(release.get("draft")),
                "prerelease": bool(release.get("prerelease")),
                "publishedAt": release.get("published_at"),
                "assetCount": len(assets),
            }
            report["missingAssets"] = missing
            ghcr_checks, ghcr = check_ghcr(image, version, ghcr_token(image))
            checks += ghcr_checks
            report["ghcr"] = ghcr

    report["ok"] = not has_error(checks)
    report["failed"] = [item["id"] for item in checks if not item["ok"]]
    return report


def selftest() -> int:
    """离线自测：版本解析、上一版选取、必需资产清单、严重级别语义。"""
    problems: list[str] = []

    if common.normalize_version("v0.2.5") != "0.2.5" or parse_version("v0.2.5")[3] != "":
        problems.append("版本归一化/解析失败")
    if not is_prerelease_version("0.2.1-rc.1"):
        problems.append("预发布判定失败")
    if is_prerelease_version("0.2.5"):
        problems.append("正式版被误判为预发布")
    try:
        parse_version("not-a-version")
        problems.append("非法版本未被拒绝")
    except PreflightError:
        pass

    tags = ["v0.2.5", "v0.2.4", "v0.2.3", "v0.2.1-rc.1", "v0.2.0"]
    if previous_stable_tag(tags, "v0.2.5", "0.2.5") != "v0.2.4":
        problems.append("上一版 tag 选取失败（应跳过正式版之间的 rc）")
    if previous_stable_tag(["v0.1.0"], "v0.1.0", "0.1.0") is not None:
        problems.append("首次发版应无上一版")
    if previous_stable_tag(["v0.2.5", "v0.2.4"], "v0.2.5", "0.2.5") != "v0.2.4":
        problems.append("上一版 tag 退化路径失败")
    if previous_stable_tag(tags, "v0.2.3", "0.2.3") != "v0.2.0":
        problems.append("从中间版本取上一版失败")

    required = expected_assets("0.2.5")
    for name in (
        "latest.json",
        "latest-linux-deb-x86_64.json",
        "workbuddy-shift_0.2.5_x64-setup.exe",
        "workbuddy-shift_0.2.5_x64-setup.exe.sig",
        "workbuddy-shift_0.2.5_aarch64.dmg",
        "workbuddy-shift_0.2.5_x86_64.dmg",
        "workbuddy-shift-linux-x86_64.AppImage.sig",
        "workbuddy-shift-macos-aarch64.app.tar.gz.sig",
    ):
        if name not in required:
            problems.append(f"必需资产清单缺 {name}")
    if len(required) != 20:
        problems.append(f"必需资产应有 20 个（8 清单 + 2 dmg + 5 包 ×2），实际 {len(required)}")
    if MANIFEST_ASSETS != FALLBACK_MANIFESTS:
        problems.append(
            f"check-release-assets.py 的 REQUIRED_MANIFESTS 与兜底副本不一致：{list(MANIFEST_ASSETS)}"
        )

    if has_error([_check("x", True, "ok")]):
        problems.append("通过的检查项被判失败")
    if not has_error([_check("y", False, "boom")]):
        problems.append("error 级失败未被判失败")
    if has_error([_check("z", False, "warn only", "warn")]):
        problems.append("warn 级失败不应让预检失败")
    if has_error([_check("w", False, "info only", "info")]):
        problems.append("info 级不成立不应让预检失败")

    # R2（is_latest 三态）：`null` 是「未知」不是「不是 latest」。
    if not is_latest_check(True, "v0.2.5")["ok"]:
        problems.append("isLatest=true 的检查项应成立")
    false_check = is_latest_check(False, "v0.2.5")
    if false_check["ok"] or false_check["severity"] != "warn":
        problems.append("isLatest=false 应记 warn 而不是 error（历史回填不是失败）")
    if has_error([false_check]):
        problems.append("isLatest=false 不该让预检失败")
    unknown_check = is_latest_check(None, "v0.2.5")
    if unknown_check["ok"] or unknown_check["severity"] != "info":
        problems.append("isLatest=null 应记 info（按 true 处理）")
    if is_latest_output(True) != "true" or is_latest_output(False) != "false":
        problems.append("is_latest 的 GITHUB_OUTPUT 三态映射错误（true/false）")
    if is_latest_output(None) != "":
        problems.append("is_latest=null 必须写成空串（否则 driver 会误判为 false）")

    for message in problems:
        print(f"SELFTEST-FAIL {message}")
    if problems:
        return 1
    print("self-test passed")
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="preflight.py",
        description="release-smoke 预检：tag/版本/资产/上一版/GHCR",
    )
    parser.add_argument("--tag", default=None, help="要验收的 tag（缺省读 GITHUB_REF_NAME）")
    parser.add_argument("--repo", default=None, help="owner/repo（缺省读 GITHUB_REPOSITORY）")
    parser.add_argument("--image", default=GHCR_IMAGE, help=f"GHCR 镜像名（默认 {GHCR_IMAGE}）")
    parser.add_argument(
        "--network",
        default="on",
        choices=("on", "off"),
        help="off = 只做本地检查（版本一致性 / 上一版 tag）",
    )
    parser.add_argument(
        "--out",
        default=os.path.join("artifacts", "release-smoke", "preflight.json"),
        help="预检快照输出路径",
    )
    parser.add_argument("--selftest", action="store_true", help="离线自测")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    try:
        report = run(args.tag, args.repo, args.network, args.image)
    except PreflightError as error:
        print(f"ERROR {error}", file=sys.stderr)
        return 1

    write_report(args.out, report)
    summary = render_summary(report)
    print(summary)
    common.append_step_summary(summary)
    common.write_github_output(
        {
            "version": report["version"],
            "prev_version": report["prevVersion"] or "",
            "has_prev": str(report["hasPrev"]).lower(),
            "is_latest": is_latest_output(report.get("isLatest")),
            "ok": str(report["ok"]).lower(),
            "missing_assets": ",".join(report["missingAssets"]),
            "preflight_json": args.out,
        }
    )
    for item in report["checks"]:
        if item["ok"]:
            continue
        label = f"[preflight:{item['severity']}] {item['id']}: {item['detail']}"
        if item["severity"] == "error":
            print(label, file=sys.stderr)
            print(f"::error::{item['id']}: {item['detail']}")
        else:
            print(label)
            if item["severity"] == "warn":
                print(f"::warning::{item['id']}: {item['detail']}")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
