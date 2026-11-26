//! 批量卸载：把本工具注册进宿主配置的 hook 全部摘掉。
//!
//! 从 `integrations.rs` 拆出来（`AGENTS.md` 第 4 节的 500 行阈值）：「批量卸载」与
//! 「单个来源的接入状态 / 安装」是两份独立职责，对外 API 未变 —— 父模块用
//! `pub use uninstall::uninstall_all_hooks;` 转发，调用方仍写
//! `crate::integrations::uninstall_all_hooks`。
//!
//! 子模块本来就能访问父模块的私有项，所以 `edit` / `read` / `strip_owned` /
//! `legacy_owned` 的可见性**不需要**放宽：`use super::…` 直接用即可，不给它们加
//! `pub(super)`，免得把「只在 `integrations` 内部可用」扩大到整棵树。

use super::{edit, legacy_owned, read, strip_owned};
use companion_core::{adapters::Collector, atomic_json};
use companion_runtime::hook_script;

/// 汇总一条错误，跳过已经出现过的同一条（P4-2）。
///
/// 脚本删除失败原本会被报两遍：`edit` 里的 [`hook_script::remove_source`] 一次、
/// 末尾兜底的 [`hook_script::remove_all`] 一次。文案里带完整路径，所以**完全相同的
/// 文案只可能来自同一次失败**，直接去重即可。
///
/// `edit` 返回的是 `；` 拼起来的多条错误，所以按分隔符拆开逐个比对 —— 否则
/// 「配置写盘失败；脚本删不掉」里的后半句会漏过去重。最终拼回去的文案与去重前
/// 逐字节相同，只是不再重复。
fn push_unique(errors: &mut Vec<String>, error: String) {
    for part in error.split('；') {
        let part = part.trim();
        if !part.is_empty() && !errors.iter().any(|existing| existing == part) {
            errors.push(part.to_string());
        }
    }
}

/// 批量移除全部已注册的 companion hook，并写入「不再自动接入」策略。
///
/// 供 `companion-runtime uninstall-hooks --home <dir>`（安装器卸载宏 / 用户手动）
/// 调用。三种形态（新的 `bash '<脚本>'`、v0.4.2 的 exe 命令、旧 `.agent-studio`
/// 命令）都会被摘掉，生成的脚本文件也一并删除；积分签到的
/// `hook.sh` / `hook-events.jsonl`、第三方 hook、备份文件、spool 队列与其它数据
/// 一律保留。
pub fn uninstall_all_hooks(c: &mut Collector) -> Result<(), String> {
    let mut errors = Vec::new();
    for source in ["codex", "workbuddy", "codebuddy-ide"] {
        if let Err(e) = edit(c, source, false) {
            push_unique(&mut errors, e);
        }
        // 防止下次 serve 启动 / settings_set 时静默重装（issue #20 的「关掉还在拦」）。
        if let Err(e) = c.set_integration_automatic(source, false) {
            push_unique(&mut errors, e);
        }
    }
    // 兜底：即使某个来源的配置文件这次读不到（目录被删 / JSON 损坏），也不把
    // 「本工具生成的脚本」留在用户目录里。
    for e in hook_script::remove_all(&c.home) {
        push_unique(&mut errors, e);
    }
    // 旧品牌自己的配置目录也可能留有注册项；它不在 files() 的扫描范围内。
    let legacy_file = c.home.join(".agent-studio/settings.json");
    if legacy_file.exists() {
        match read(&legacy_file) {
            Ok(before) => {
                let mut doc = before.clone();
                strip_owned(&mut doc, &|value| legacy_owned(&c.home, value));
                if doc != before {
                    if let Err(e) = atomic_json(&legacy_file, &doc) {
                        errors.push(e);
                    }
                }
            }
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}
