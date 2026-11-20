//! 会话复制：数据库行写入、空间映射与复制流程编排（design §2 / §4）。
//!
//! 复制把「正文 + sessions 行」写进目标账号；来源始终只读。目标 UUID 在写任何正文前
//! 持久化到操作日志，失败后由 `recovery` 按同一 UUID 补完，绝不产生第二个副本。
//! 数据库写入沿用 SQLite 一致性快照备份（`backup_workbuddy_db`），不直接复制活动库。

//
// 子模块职责：`db` / `db_spaces` 写库与空间映射、`body` 正文、`plan` 顶层编排
//（`plan_prepare` 准备与校验、`plan_execute` 执行与恢复重放）；对外入口仍在下方
// re-export，外部继续用 `session::copy_sessions_*`。

use super::*;

mod body;
mod db;
mod db_spaces;
mod plan;
mod plan_execute;
mod plan_prepare;
mod repair;

pub(in crate::modules::session) use body::*;
pub(in crate::modules::session) use db::*;
pub(in crate::modules::session) use db_spaces::*;
pub use plan::{copy_sessions_cross, copy_sessions_for_switch};
pub(in crate::modules::session) use plan_execute::*;
pub(in crate::modules::session) use plan_prepare::*;
// 源正文兜底（严格校验失败时的容错修复）：只被 `copy_one_session` 调用。
pub(in crate::modules::session) use repair::repair_source_snapshot;
// 单测注入入口 / 测试夹具：生产路径走同模块的公开入口与内部编排，这里只为 `session::tests` 转出。
#[allow(unused_imports)]
pub(in crate::modules::session) use plan::{
    copy_sessions_cross_at, copy_sessions_for_switch_at, CopySide,
};
