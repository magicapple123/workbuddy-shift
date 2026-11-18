//! Token 统计的沙箱守卫（端到端）。
//!
//! 单测只能覆盖注入式纯函数（`token_stats::stats_roots_at`）。`get_statistics()` 里
//! 「家目录从哪来」那一步（`config::home_dir()`）没有参数可注入，只能在真实环境变量下
//! 验证。`WB_SHIFT_HOME` 是进程级的，和其它测试跑在同一个二进制里会互相打架，因此
//! 单开这个集成测试文件——**一个文件一条用例**，进程天然隔离，`set_var` 安全。
//!
//! 失败时的含义：沙箱里点开「Token 统计」看到的不是沙箱副本，而是真实的
//! `~/.workbuddy`、`~/.codebuddy`、`~/…/CodeBuddyExtension`（历史缺陷：两处早先分别用
//! `dirs::home_dir()` / `dirs::data_local_dir()` 自己拼路径，绕过了 `home_dir()`）。

use std::path::PathBuf;

use serde_json::Value;
use wb_shift_core::modules::config::HOME_ENV_VAR;
use wb_shift_core::modules::token_stats::get_statistics;

/// 探针沙箱：析构时还原环境变量并删目录。
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-token-stats-sandbox-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        Self { root }
    }

    /// `~/.workbuddy/projects/<project>/<session>.jsonl` —— 国内版客户端的最小数据形态。
    ///
    /// `cwd` 与目录名都用同一个唯一标记：`project_name` 取根下第一层目录名，
    /// `record_project` 取 `cwd` 的末段，两条路径都会把它带进响应的 `projects[].key`。
    fn seed_cn_session(&self, project: &str, session_id: &str) {
        let dir = self.root.join(".workbuddy").join("projects").join(project);
        std::fs::create_dir_all(&dir).expect("create probe projects dir");
        let record = serde_json::json!({
            "timestamp": wb_shift_core::modules::config::now_ms(),
            "cwd": format!("/fixture/{project}"),
            "message": { "usage": { "input_tokens": 11, "output_tokens": 5 } }
        });
        std::fs::write(
            dir.join(format!("{session_id}.jsonl")),
            record.to_string() + "\n",
        )
        .expect("write probe jsonl");
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        std::env::remove_var(HOME_ENV_VAR);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// `sources` 里按 `source` 字段取一条（顺序变了也不影响断言）。
fn source<'a>(statistics: &'a Value, name: &str) -> &'a Value {
    statistics["sources"]
        .as_array()
        .expect("sources must be an array")
        .iter()
        .find(|entry| entry["source"] == Value::String(name.to_string()))
        .unwrap_or_else(|| panic!("缺少 source: {name}"))
}

fn files_scanned(statistics: &Value, name: &str) -> u64 {
    source(statistics, name)["filesScanned"]
        .as_u64()
        .unwrap_or_else(|| panic!("{name} 缺少 filesScanned"))
}

#[test]
fn statistics_only_scan_the_sandbox_home() {
    let sandbox = Sandbox::new();
    let project = format!("probe-project-{}", std::process::id());
    let session_id = format!("probe-session-{}", std::process::id());
    sandbox.seed_cn_session(&project, &session_id);

    // 探针目录落在 `temp_dir()` 下（唯一性由进程号保证）。注意它**可能**就在真实家目录
    // 里面（Windows 的 `%LOCALAPPDATA%\Temp` 就是），所以下面的断言不能写成
    // 「结果不以真实家目录开头」——那会在正确的实现上误红。
    std::env::set_var(HOME_ENV_VAR, &sandbox.root);

    let statistics = get_statistics(None);

    // 1) 四个来源都在（国内版 / 国际版 / CLI / IDE）。
    let sources = statistics["sources"]
        .as_array()
        .expect("sources must be an array");
    assert_eq!(sources.len(), 4, "来源数量变了: {statistics}");
    for name in [
        "workbuddy",
        "workbuddy-ai",
        "codebuddy-cli",
        "codebuddy-ide",
    ] {
        // `source()` 缺项即 panic，这里只是把四个名字都过一遍。
        let _ = source(&statistics, name);
    }

    // 2) 探针数据真的被扫到了：不是「全是 0 所以看起来没问题」的空转。
    assert_eq!(
        files_scanned(&statistics, "workbuddy"),
        1,
        "沙箱里的 .workbuddy/projects 未被扫描: {statistics}"
    );
    assert_eq!(
        source(&statistics, "workbuddy")["summary"]["records"],
        1,
        "沙箱里的用量记录未被计入: {statistics}"
    );
    assert_eq!(
        source(&statistics, "workbuddy")["projects"][0]["key"],
        Value::String(project.clone()),
        "项目归属没跟着沙箱数据走: {statistics}"
    );
    assert_eq!(
        source(&statistics, "workbuddy")["sessions"][0]["sessionId"],
        Value::String(session_id.clone()),
        "会话标识没跟着沙箱数据走: {statistics}"
    );

    // 3) 另外三个来源在沙箱里没有任何对应数据：必须是 0。
    //    任何一条非 0，都说明那一路仍然在扫真实家目录（探针里没有这些目录）。
    for name in ["workbuddy-ai", "codebuddy-cli", "codebuddy-ide"] {
        assert_eq!(
            files_scanned(&statistics, name),
            0,
            "{name} 扫到了沙箱之外的文件: {statistics}"
        );
    }

    // 4) 响应里不出现真实家目录的路径（JSON 里的 `\` 是转义过的，先按 JSON 规则还原）。
    let real_home = wb_shift_core::modules::config::real_home_dir();
    let needle = serde_json::to_string(&real_home.to_string_lossy().to_string())
        .expect("encode real home")
        .trim_matches('"')
        .to_string();
    let payload = statistics.to_string();
    assert!(
        !payload.contains(&needle),
        "响应里出现了真实家目录 {}: {statistics}",
        real_home.display()
    );
}
