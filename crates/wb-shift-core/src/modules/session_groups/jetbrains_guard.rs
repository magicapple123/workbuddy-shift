//! JetBrains IDE 侧写操作的 IDE 生命周期窗口（关闭 → 写入 → 重开）。
//!
//! 与 [`super::sync::VscodeEditorGuard`] 同语义，只是宿主从 VS Code 换成 IDEA / PyCharm：
//!
//! - 未运行（没有任何装了插件且在运行的实例）→ 不关闭、不主动拉起；
//! - 运行中 + `restart = false` → `Err(manual_hint)`（各入口保留自己的既有文案）；
//! - 运行中 + `restart = true` → 优雅关闭**所有**装了插件且在运行的实例（IDEA 与 PyCharm
//!   共用同一份 `Data/<uid>/JetBrains`，只关一个仍会被另一个覆盖），窗口结束后逐个重开。
//!
//! 关闭 / 重开本身由 `jetbrains` 模块提供（`close_running_plugin_ides` /
//! `relaunch_closed_ides`），这里只负责「什么时候关、关完必须开回来」。

use super::*;

/// 关闭决策（纯函数）：`Ok(true)` = 需要关闭，`Ok(false)` = 没在运行（不关也不主动拉起）。
///
/// 与 VS Code 侧 `plan_editor_restart` 的差别只在「是否需要按目标账号过滤」：
/// JetBrains 不做过滤——多个产品共用同一份插件数据目录，只要还有装了插件且在运行的实例，
/// 写入就可能被它的内存态覆盖。
pub(super) fn plan_ide_restart(
    running: bool,
    restart: bool,
    manual_hint: &str,
) -> Result<bool, String> {
    if !running {
        return Ok(false);
    }
    if !restart {
        return Err(manual_hint.to_string());
    }
    Ok(true)
}

/// JetBrains 侧的生命周期守卫。
pub(super) struct JetbrainsIdeGuard {
    /// 本次由我们关闭的实例；`None` = 本来就没运行（不主动拉起）。
    closed: Option<jetbrains::ClosedIdes>,
}

impl JetbrainsIdeGuard {
    /// 打开窗口：判定 →（必要时）关闭。校验类错误必须在此前完成，
    /// 否则会为注定失败的请求打断用户。
    pub(super) fn prepare(restart: bool, manual_hint: &str) -> Result<Self, String> {
        let running = !jetbrains::running_plugin_ide_labels().is_empty();
        if !plan_ide_restart(running, restart, manual_hint)? {
            return Ok(Self { closed: None });
        }
        let closed = jetbrains::close_running_plugin_ides()?;
        Ok(Self {
            closed: Some(closed),
        })
    }

    /// 本次窗口是否真的关闭了 IDE（报告字段只认「我们关的」）。
    pub(super) fn closed_by_us(&self) -> bool {
        self.closed.as_ref().is_some_and(|closed| !closed.is_empty())
    }

    /// 成功收尾：重开（仅本次由我们关闭时）。`Err` = 重开失败，操作本身已成功。
    pub(super) fn finish(self) -> Result<(), String> {
        match self.closed {
            None => Ok(()),
            Some(closed) => jetbrains::relaunch_closed_ides(closed),
        }
    }

    /// 失败收尾：best-effort 重开，并把重开失败信息追加到原错误之后（不吞原错误）。
    pub(super) fn fail(self, error: String) -> String {
        match self.finish() {
            Ok(()) => error,
            Err(launch_error) => format!("{error}\n\n{launch_error}"),
        }
    }
}
