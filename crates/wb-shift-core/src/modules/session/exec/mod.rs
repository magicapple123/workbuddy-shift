//! 同步备份与阶段化执行（design §5.4 / §5.5）。
//!
//! 写目标前必须生成可核验的一致备份：SQLite 在线备份 API 快照数据库、逐文件备份目标
//! 正文并记录摘要、写入恢复清单。备份不可信时零写入。执行按「正文 → 数据库 → 组表 →
//! completed」推进阶段，每步先落阶段再推进；正常执行与恢复共用同一段代码。

//
// 子模块职责：`backup` 备份与核验、`write` 写入应用的原子操作、
// `write_replay` 阶段推进与恢复重放；对外仍用 `session::` 入口。

use super::copy::*;
use super::sync::*;
use super::*;

mod backup;
mod write;
mod write_replay;

pub(in crate::modules::session) use backup::{
    create_sync_backup, load_sync_manifest, read_session_row, session_row_snapshot,
    validated_target_body_path, verify_sync_backup, SyncBackupManifest, SyncBackupRow,
};
// 单测需要核对清单里记录的快照方法，生产路径不直接读取该常量。
#[allow(unused_imports)]
pub(in crate::modules::session) use backup::DB_SNAPSHOT_METHOD;
pub(in crate::modules::session) use write::*;
pub(in crate::modules::session) use write_replay::*;
