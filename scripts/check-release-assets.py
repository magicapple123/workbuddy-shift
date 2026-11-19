#!/usr/bin/env python3
"""发布后校验：资产完整性、去重、清单与签名密钥一致性、发行说明资源说明。

用法（CI 里在 release job 最后一步）：
    python3 scripts/check-release-assets.py v0.2.1

校验项（对应第三轮第 4 / 18 条）：
1. 必需资产齐全：8 份清单 + 各平台人用安装包 + `.sig`；
2. Windows 只有一份人用安装包（`workbuddy-shift_<ver>_x64-setup.exe`），
   0.2.0 是历史版本，允许保留旧的双名并只告警；
3. 同 sha256 重复资产：只允许 `latest-macos-*` / `latest-darwin-*` 兼容别名；
4. `latest.json` 的 version / 平台键 / 签名 key id 与仓库公钥一致；
5. Release 正文包含「下载哪个？」与「其余文件是什么？」。

退出码非 0 即失败，CI 会拦住发布收尾。
"""

from __future__ import annotations

import base64
import json
import os
import re
import subprocess
import sys
import time
import urllib.request

# 0.2.0 及更早：Windows 双名是已发布事实，不能回改（回改会让 0.2.0 客户端 404）。
LEGACY_DUAL_NAME_MAX = (0, 2, 0)

# 预发布版本也要能匹配：0.2.1-rc.1 的安装包名是
# `workbuddy-shift_0.2.1-rc.1_x64-setup.exe`（tag 与文件名同源）。
VERSIONED_WINDOWS_INSTALLER_RE = re.compile(
    r"workbuddy-shift_\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?_x64-setup\.exe$"
)

REQUIRED_MANIFESTS = [
    "latest.json",
    "latest-windows-x86_64.json",
    "latest-macos-aarch64.json",
    "latest-darwin-aarch64.json",
    "latest-macos-x86_64.json",
    "latest-darwin-x86_64.json",
    "latest-linux-x86_64.json",
    "latest-linux-deb-x86_64.json",
]

PLATFORM_KEYS = [
    "darwin-aarch64",
    "darwin-x86_64",
    "windows-x86_64-nsis",
    "windows-x86_64",
    "linux-x86_64",
    "linux-x86_64-deb",
    "linux-x86_64-appimage",
]

errors: list[str] = []
warnings: list[str] = []
# 0.2.0 及更早的清单用的是 releases/latest/download，且正文没有资源说明：
# 这些是已发布事实，回改会破坏存量客户端，只能降级为告警（用于本地 dry-run）。
LEGACY_STRICT_MAX = (0, 2, 0)
legacy_mode = False


def fail(message: str) -> None:
    if legacy_mode:
        warnings.append(f"[legacy] {message}")
        return
    errors.append(message)


def warn(message: str) -> None:
    warnings.append(message)


def gh_api(path: str) -> dict:
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if not token:
        raise SystemExit("缺少 GH_TOKEN / GITHUB_TOKEN，无法查询 Release")
    result = subprocess.run(
        ["gh", "api", path],
        check=True,
        capture_output=True,
        text=True,
        env={**os.environ, "GH_TOKEN": token},
    )
    return json.loads(result.stdout)


def repo_slug() -> str:
    explicit = os.environ.get("GITHUB_REPOSITORY")
    if explicit:
        return explicit
    result = subprocess.run(
        ["gh", "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def parse_version(tag: str) -> tuple[int, int, int]:
    match = re.match(r"v?(\d+)\.(\d+)\.(\d+)", tag)
    if not match:
        raise SystemExit(f"无法解析 tag：{tag}")
    return tuple(int(part) for part in match.groups())  # type: ignore[return-value]


def key_id(base64_blob: str) -> str | None:
    """minisign：算法 2 字节 + key id 8 字节（小端）在前 10 字节。"""
    try:
        raw = base64.b64decode(base64_blob.strip(), validate=True)
    except Exception:
        return None
    if len(raw) < 10:
        return None
    return raw[2:10].hex().upper()


def download_manifest(owner_repo: str, tag: str, attempts: int = 12) -> dict:
    url = f"https://github.com/{owner_repo}/releases/download/{tag}/latest.json"
    last_error = ""
    for _ in range(attempts):
        try:
            with urllib.request.urlopen(url, timeout=15) as response:
                return json.load(response)
        except Exception as error:  # noqa: BLE001 - 网络错误统一重试
            last_error = str(error)
            time.sleep(5)
    raise SystemExit(f"拉取 {url} 失败：{last_error}")


def is_versioned_windows_installer(name: str) -> bool:
    return VERSIONED_WINDOWS_INSTALLER_RE.match(name) is not None


def check_assets(tag: str, version: tuple[int, int, int]) -> list[dict]:
    owner_repo = repo_slug()
    release = gh_api(f"repos/{owner_repo}/releases/tags/{tag}")
    assets = release.get("assets", [])
    names = {asset["name"] for asset in assets}

    for manifest in REQUIRED_MANIFESTS:
        if manifest not in names:
            fail(f"缺少清单资产：{manifest}")

    # 只有被 updater 清单引用的资产需要 .sig；dmg / msi 是纯人用安装包，不参与自动更新。
    has_sig = {name for name in names if name.endswith(".sig")}
    for name in sorted(names):
        if name.endswith((".exe", ".deb", ".AppImage", ".app.tar.gz")) and f"{name}.sig" not in has_sig:
            fail(f"资产缺少签名：{name}.sig")

    windows_installers = sorted(
        name for name in names if name.endswith("-setup.exe")
    )
    versioned = [name for name in windows_installers if is_versioned_windows_installer(name)]
    if len(versioned) != 1:
        fail(f"Windows 人用安装包应恰好 1 份带版本号的，实际：{versioned}")
    if "workbuddy-shift-windows-x86_64-setup.exe" in windows_installers:
        if version <= LEGACY_DUAL_NAME_MAX:
            warn("历史版本保留了 Windows 稳定名副本（0.2.0 客户端仍依赖它，不回改）")
        else:
            fail("Windows 仍存在重复的稳定名安装包 workbuddy-shift-windows-x86_64-setup.exe")

    # 同 sha256 分组：只允许 macOS 清单别名。
    groups: dict[str, list[str]] = {}
    for asset in assets:
        digest = asset.get("digest")
        if digest:
            groups.setdefault(digest, []).append(asset["name"])
    for digest, group in groups.items():
        if len(group) < 2:
            continue
        if all(re.match(r"latest-(macos|darwin)-", name) for name in group):
            continue
        fail(f"重复资产（sha256 {digest[:19]}…）：{sorted(group)}")

    body = release.get("body") or ""
    for heading in ("下载哪个？", "其余文件是什么？"):
        if heading not in body:
            fail(f"发行说明缺少「{heading}」说明段")

    return assets


def check_manifest(tag: str, config_path: str = "src-tauri/tauri.conf.json") -> None:
    owner_repo = repo_slug()
    manifest = download_manifest(owner_repo, tag)
    expected = tag.lstrip("v")
    if str(manifest.get("version", "")).lstrip("v") != expected:
        fail(f"latest.json version={manifest.get('version')} 与 tag {tag} 不一致")

    with open(config_path, encoding="utf-8") as handle:
        config = json.load(handle)
    pubkey = (
        config.get("plugins", {}).get("updater", {}).get("pubkey")
        or config.get("tauri", {}).get("plugins", {}).get("updater", {}).get("pubkey")
    )
    expected_key = key_id(pubkey) if pubkey else None
    if not expected_key:
        fail("tauri.conf.json 中读不到 updater 公钥")

    platforms = manifest.get("platforms") or {}
    for key in PLATFORM_KEYS:
        if key not in platforms:
            fail(f"latest.json 缺少平台键：{key}")
            continue
        entry = platforms[key]
        url = entry.get("url", "")
        if "/releases/download/" not in url or f"/download/{tag}/" not in url:
            fail(f"{key} 的资产 URL 不是 tag 固定路径：{url}")
        signature = entry.get("signature", "")
        actual = key_id(signature)
        if expected_key and actual != expected_key:
            fail(f"{key} 签名 key id={actual} 与公钥 key id={expected_key} 不一致")

    notes = manifest.get("notes") or ""
    if not notes.strip():
        warn("latest.json 的 notes 为空（发版时未提供 release-notes 文件）")


def selftest() -> int:
    """不联网的名字规则自测，CI 里跑，防止预发布版本再被误判。"""
    accepted = [
        "workbuddy-shift_0.2.1_x64-setup.exe",
        "workbuddy-shift_0.2.1-rc.1_x64-setup.exe",
        "workbuddy-shift_0.2.1-rc.10_x64-setup.exe",
        "workbuddy-shift_1.0.0-beta.2_x64-setup.exe",
    ]
    rejected = [
        "workbuddy-shift_x64-setup.exe",
        "workbuddy-shift_0.2.1_x64-setup.exe.sig",
        "workbuddy-shift_0.2_x64-setup.exe",
        "WorkBuddy Shift_0.2.1_x64-setup.exe",
    ]
    bad = [name for name in accepted if not is_versioned_windows_installer(name)]
    bad += [name for name in rejected if is_versioned_windows_installer(name)]
    if bad:
        print(f"self-test failed: {bad}")
        return 1
    print("self-test passed")
    return 0


def main() -> int:
    if sys.argv[1:2] == ["--selftest"]:
        return selftest()
    tag = sys.argv[1] if len(sys.argv) > 1 else os.environ.get("GITHUB_REF_NAME", "")
    if not tag:
        raise SystemExit("用法：check-release-assets.py <tag>")
    version = parse_version(tag)
    global legacy_mode
    legacy_mode = version <= LEGACY_STRICT_MAX
    if legacy_mode:
        warnings.append("[legacy] 历史版本：URL 固定/正文资源说明等新规则降级为告警")
    check_assets(tag, version)
    check_manifest(tag)

    for message in warnings:
        print(f"WARN  {message}")
    for message in errors:
        print(f"ERROR {message}")
    if errors:
        print(f"发布校验失败：{len(errors)} 项错误")
        return 1
    print(f"发布校验通过：{tag}（{len(warnings)} 条告警）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
