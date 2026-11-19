//! 定位（reveal）scope 的单测：只测纯函数 [`reveal_api::resolve_target`]，不启动进程。
//!
//! 覆盖第三轮第 5 条方案 A 的三条安全边界：
//!
//! 1. **兼容**：不传 `scope` 与显式传 `storage` 完全等价（老调用方零改动）；
//! 2. **放开只读**：白名单外的绝对目录在 `cwd` 下可打开，但 `storage` 仍拒绝；
//! 3. **拒绝危险路径**：UNC（`\\server\share`）、设备 / 长路径前缀（`\\.\`、`\\?\`）、
//!    相对路径、`..` 穿越。
//!
//! 从 `tests.rs` 拆出（单文件规模控制），模块内部按「一条边界一组断言」组织。

use super::super::reveal as reveal_api;
use super::super::reveal::{SCOPE_CWD, SCOPE_STORAGE};
use super::fixture::Sandbox;

#[test]
fn compat_no_scope_is_identical_to_explicit_storage_scope() {
    let sandbox = Sandbox::new("reveal-compat");
    let inside = sandbox.data_root.join("logs");
    std::fs::create_dir_all(&inside).expect("建日志目录");
    std::fs::write(inside.join("app.log"), b"log").expect("写日志");

    // 目录：不传 scope == 传 storage（路径 / 回退标志 / mode / scope 全部一致）。
    let legacy = reveal_api::resolve_target(&sandbox.roots, &inside.to_string_lossy(), "open", None)
        .expect("白名单内目录必须允许");
    let explicit = reveal_api::resolve_target(
        &sandbox.roots,
        &inside.to_string_lossy(),
        "open",
        Some(SCOPE_STORAGE),
    )
    .expect("白名单内目录必须允许");
    assert_eq!(legacy.path, explicit.path, "不传 scope 必须与 storage 同路径");
    assert_eq!(legacy.path, inside);
    assert_eq!(legacy.fell_back, explicit.fell_back);
    assert_eq!(legacy.fell_back, false);
    assert_eq!(legacy.mode, explicit.mode);
    assert_eq!(legacy.mode, "open");
    assert_eq!(legacy.scope, SCOPE_STORAGE);
    assert_eq!(explicit.scope, SCOPE_STORAGE);

    // 文件：`select` 保持选中文件本身（storage 是破坏性入口，不改语义）。
    let file = inside.join("app.log");
    let legacy =
        reveal_api::resolve_target(&sandbox.roots, &file.to_string_lossy(), "select", None)
            .expect("白名单内文件必须允许");
    let explicit = reveal_api::resolve_target(
        &sandbox.roots,
        &file.to_string_lossy(),
        "select",
        Some(SCOPE_STORAGE),
    )
    .expect("白名单内文件必须允许");
    assert_eq!(legacy.path, file, "storage scope 不得把文件归一成目录");
    assert_eq!(legacy.mode, "select");
    assert_eq!(legacy.path, explicit.path);
    assert_eq!(legacy.mode, explicit.mode);

    // 空串 scope 也按缺省处理（老前端可能传 `scope: ""`）。
    let empty = reveal_api::resolve_target(&sandbox.roots, &inside.to_string_lossy(), "open", Some(""))
        .expect("空 scope 必须按缺省处理");
    assert_eq!(empty.scope, SCOPE_STORAGE);
    assert_eq!(empty.path, inside);

    // 白名单外：不传 scope 与传 storage 的拒绝文案完全一致。
    let outside = sandbox.base.join("project");
    std::fs::create_dir_all(&outside).expect("建外部目录");
    let legacy_error =
        reveal_api::resolve_target(&sandbox.roots, &outside.to_string_lossy(), "open", None)
            .expect_err("默认 scope 必须拒绝白名单外路径");
    let explicit_error = reveal_api::resolve_target(
        &sandbox.roots,
        &outside.to_string_lossy(),
        "open",
        Some(SCOPE_STORAGE),
    )
    .expect_err("storage scope 必须拒绝白名单外路径");
    assert_eq!(legacy_error, explicit_error, "兼容性：拒绝文案必须一致");
    assert!(legacy_error.contains("白名单"), "{legacy_error}");
}

#[test]
fn cwd_allows_outside_root_that_storage_rejects() {
    let sandbox = Sandbox::new("reveal-outside");
    let project = sandbox.base.join("ResumeForge");
    std::fs::create_dir_all(project.join("src")).expect("建项目目录");
    std::fs::write(project.join("src/main.ts"), b"x").expect("写文件");

    // 同一个路径：`storage` 拒绝（不在白名单），`cwd` 放行（只读定位）。
    let error = reveal_api::resolve_target(
        &sandbox.roots,
        &project.to_string_lossy(),
        "open",
        Some(SCOPE_STORAGE),
    )
    .expect_err("storage 必须拒绝白名单外的工作目录");
    assert!(error.contains("白名单"), "{error}");

    let target =
        reveal_api::resolve_target(&sandbox.roots, &project.to_string_lossy(), "open", Some(SCOPE_CWD))
            .expect("cwd 必须允许白名单外目录");
    assert_eq!(target.scope, SCOPE_CWD);
    assert_eq!(target.path, project);
    assert!(!target.fell_back);

    // 白名单内的路径两种 scope 都允许（cwd 只多开一扇门，不关掉原来的）。
    let inside = sandbox.data_root.join("logs");
    std::fs::create_dir_all(&inside).expect("建日志目录");
    assert!(reveal_api::resolve_target(&sandbox.roots, &inside.to_string_lossy(), "open", Some(SCOPE_CWD)).is_ok());
    assert!(reveal_api::resolve_target(&sandbox.roots, &inside.to_string_lossy(), "open", Some(SCOPE_STORAGE)).is_ok());
}

#[test]
fn cwd_normalizes_files_to_their_directory() {
    let sandbox = Sandbox::new("reveal-normalize");
    let project = sandbox.base.join("proj");
    std::fs::create_dir_all(project.join("src")).expect("建目录");
    std::fs::write(project.join("src/main.ts"), b"x").expect("写文件");

    // 目录：原样打开。
    let dir = reveal_api::resolve_target(
        &sandbox.roots,
        &project.join("src").to_string_lossy(),
        "select",
        Some(SCOPE_CWD),
    )
    .expect("目录必须能定位");
    assert_eq!(dir.path, project.join("src"));
    assert_eq!(dir.mode, "open", "cwd 一律 open：绝不执行目标文件");
    assert!(!dir.fell_back);

    // 文件：归一成所在目录，`select` 也被强制改成 `open`。
    let file = reveal_api::resolve_target(
        &sandbox.roots,
        &project.join("src/main.ts").to_string_lossy(),
        "select",
        Some(SCOPE_CWD),
    )
    .expect("文件必须归一成目录");
    assert_eq!(file.path, project.join("src"));
    assert_eq!(file.mode, "open");
    assert_eq!(file.scope, SCOPE_CWD);
}

#[test]
fn cwd_rejects_unc_device_and_long_path_prefixes() {
    // 两段反斜杠开头的 UNC / 设备 / 长路径前缀：会带上 Windows 凭据或绕过常规路径语义。
    // 其中 `//server/share` 在 Windows 上同样是 UNC 前缀写法。
    for raw in [
        r"\\server\share",
        r"\\server\share\folder",
        r"\\?\C:\Windows",
        r"\\.\PhysicalDrive0",
        "//server/share",
    ] {
        let sandbox = Sandbox::new("reveal-unc");
        let error = reveal_api::resolve_target(&sandbox.roots, raw, "open", Some(SCOPE_CWD))
            .expect_err("UNC / 设备路径必须拒绝");
        assert!(!error.is_empty());
        #[cfg(windows)]
        assert!(
            error.contains("UNC"),
            "Windows 上应命中 UNC / 设备分支，实际为：{error}"
        );
    }
}

#[test]
fn cwd_rejects_relative_paths() {
    let sandbox = Sandbox::new("reveal-relative");
    for raw in ["relative/dir", "logs/app.log", ".", "..", "./x"] {
        let error = reveal_api::resolve_target(&sandbox.roots, raw, "open", Some(SCOPE_CWD))
            .expect_err("相对路径必须拒绝");
        assert!(
            error.contains("绝对路径"),
            "应命中「只接受绝对路径」分支，实际为：{error}"
        );
    }
}

#[test]
fn cwd_rejects_parent_traversal() {
    let sandbox = Sandbox::new("reveal-traversal");
    let dir = sandbox.base.join("proj");
    std::fs::create_dir_all(&dir).expect("建目录");

    // 绝对路径里夹 `..` 同样拒绝：不能靠父级引用跑出工作目录。
    let traversal = format!("{}/../etc", dir.to_string_lossy());
    let error = reveal_api::resolve_target(&sandbox.roots, &traversal, "open", Some(SCOPE_CWD))
        .expect_err(".. 穿越必须拒绝");
    assert!(error.contains(".."), "{error}");

    // 多层穿越 / 前导穿越也算。
    for raw in [
        format!("{}/a/../../b", dir.to_string_lossy()),
        format!("{}/..", dir.to_string_lossy()),
    ] {
        let error = reveal_api::resolve_target(&sandbox.roots, &raw, "open", Some(SCOPE_CWD))
            .expect_err(".. 穿越必须拒绝");
        assert!(error.contains(".."), "{error}");
    }
}

#[test]
fn cwd_falls_back_to_existing_ancestor() {
    let sandbox = Sandbox::new("reveal-fallback");
    let dir = sandbox.base.join("gone");
    std::fs::create_dir_all(&dir).expect("建目录");

    let missing = dir.join("deleted-project");
    let target =
        reveal_api::resolve_target(&sandbox.roots, &missing.to_string_lossy(), "open", Some(SCOPE_CWD))
            .expect("不存在的目录要回退而不是报错");
    assert!(target.fell_back, "必须标记回退");
    assert_eq!(target.path, dir, "回退到最近的上层目录");
    assert_eq!(target.mode, "open");
    assert_eq!(target.scope, SCOPE_CWD);
}

#[test]
fn unknown_scope_and_bad_mode_are_rejected() {
    let sandbox = Sandbox::new("reveal-bad-input");
    let dir = sandbox.base.join("proj");
    std::fs::create_dir_all(&dir).expect("建目录");
    let path = dir.to_string_lossy().to_string();

    let error = reveal_api::resolve_target(&sandbox.roots, &path, "open", Some("any"))
        .expect_err("未知 scope 必须拒绝");
    assert!(error.contains("scope"), "{error}");

    let error = reveal_api::resolve_target(&sandbox.roots, &path, "delete", Some(SCOPE_CWD))
        .expect_err("未知 mode 必须拒绝");
    assert!(error.contains("mode"), "{error}");

    let error = reveal_api::resolve_target(&sandbox.roots, "   ", "open", Some(SCOPE_CWD))
        .expect_err("空路径必须拒绝");
    assert!(error.contains("缺少路径"), "{error}");
}

#[test]
fn parse_scope_defaults_to_storage() {
    assert_eq!(reveal_api::parse_scope(None).expect("缺省合法"), SCOPE_STORAGE);
    assert_eq!(
        reveal_api::parse_scope(Some("")).expect("空串按缺省"),
        SCOPE_STORAGE
    );
    assert_eq!(
        reveal_api::parse_scope(Some("  storage  ")).expect("trim 后合法"),
        SCOPE_STORAGE
    );
    assert_eq!(
        reveal_api::parse_scope(Some(" cwd ")).expect("trim 后合法"),
        SCOPE_CWD
    );
    assert!(
        reveal_api::parse_scope(Some("CWD")).is_err(),
        "scope 区分大小写，未知值必须报错"
    );
}

/// 编译期断言：这两个常量就是对外契约里的字符串值（改字面量会破坏老前端）。
#[test]
fn scope_constants_are_stable() {
    assert_eq!(SCOPE_STORAGE, "storage");
    assert_eq!(SCOPE_CWD, "cwd");
}
