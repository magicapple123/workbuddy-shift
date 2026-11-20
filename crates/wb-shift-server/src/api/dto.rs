//! HTTP 请求体 DTO：把 WebUI 的 JSON 形状变成明确类型。
//!
//! 为什么需要：`Json<Value>` + `body.get("x").and_then(Value::as_bool).unwrap_or(true)`
//! 会把**类型错误**静默降级成默认值（`restart: "false"` 会被当成 `true` 并真的重启客户端）。
//! DTO 让这类请求在边界就得到 400，而不是带着错误参数继续执行写操作。
//!
//! 规则（见 `AGENTS.md` 与 `docs/ARCHITECTURE.md` 的“API 双通道”）：
//!
//! - 字段名必须与 Tauri command 参数（JS 侧 camelCase）逐字一致，
//!   `node scripts/check-api-contract.mjs` 在 CI 里比对两侧参数名；
//! - 缺省值必须与改造前一致（多为空串 / false / 空数组），保持旧前端与旧脚本可用；
//! - **嵌套的业务结构**（`syncSelections`、`targets`）继续交给 wb-shift-core 校验：
//!   桌面端走的是同一份校验函数，边界这里再写一遍只会产生两套错误文案；
//! - 未迁移的 handler 在 `scripts/api-contract-baseline.json` 登记理由；新增 handler
//!   必须用 DTO，或先登记再迁移。

use serde::{Deserialize, Deserializer};
use serde_json::Value;

// ---------------------------------------------------------------------------
// 账号 / 切换
// ---------------------------------------------------------------------------

/// `POST /api/switch`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SwitchRequest {
    pub(crate) account_id: String,
    pub(crate) restart: bool,
    pub(crate) share_sessions: bool,
    pub(crate) copy_session_ids: Vec<String>,
    /// `[{groupId, previewToken, mode}]`，具体形状由 `session::parse_sync_selections` 校验。
    pub(crate) sync_selections: Option<Value>,
}

impl Default for SwitchRequest {
    fn default() -> Self {
        Self {
            account_id: String::new(),
            // 与改造前一致：未显式关闭时切换完成后重启客户端。
            restart: true,
            share_sessions: false,
            copy_session_ids: Vec::new(),
            sync_selections: None,
        }
    }
}

/// 只带档位的请求体（`import-local` / `oauth/start`），body 可缺省。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct VariantRequest {
    pub(crate) variant: Option<String>,
}

/// `POST /api/delete`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct DeleteAccountRequest {
    pub(crate) account_id: String,
}

/// 展示字段补丁：`note` 的三态语义（缺省 = 不改、null = 清空、字符串 = 设置）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct DisplayPatch {
    /// `Option<Option<String>>` 的默认 serde 语义会把 `null` 也解析成 `None`（说不清
    /// 「没传」还是「传了 null」），这里用自定义反序列化保留三态。
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub(crate) note: Option<Option<String>>,
    pub(crate) display_field: Option<String>,
}

/// 区分「字段缺省」（`None`）与「显式 null」（`Some(None)`）。
fn deserialize_nullable<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

impl DisplayPatch {
    /// 还原成 core 期望的 patch 对象（只包含请求里出现的键）。
    pub(crate) fn to_value(&self) -> Value {
        let mut patch = serde_json::Map::new();
        if let Some(note) = &self.note {
            patch.insert(
                "note".to_string(),
                note.clone().map(Value::String).unwrap_or(Value::Null),
            );
        }
        if let Some(field) = &self.display_field {
            patch.insert("displayField".to_string(), Value::String(field.clone()));
        }
        Value::Object(patch)
    }
}

/// `POST /api/update-account-display`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct UpdateAccountDisplayRequest {
    pub(crate) account_id: String,
    pub(crate) patch: DisplayPatch,
}

/// `POST /api/oauth/status`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct OAuthStatusRequest {
    pub(crate) login_id: String,
}

// ---------------------------------------------------------------------------
// 导入 / 导出
// ---------------------------------------------------------------------------

/// `POST /api/export-accounts`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ExportAccountsRequest {
    pub(crate) account_ids: Vec<String>,
}

/// `POST /api/export-accounts-to-path`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ExportAccountsToPathRequest {
    pub(crate) account_ids: Vec<String>,
    pub(crate) path: String,
}

/// `POST /api/import/preview`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ImportFileRequest {
    pub(crate) file_text: String,
}

/// `POST /api/import`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ImportAccountsRequest {
    pub(crate) file_text: String,
    pub(crate) indexes: Vec<usize>,
}

// ---------------------------------------------------------------------------
// 会话复制 / 同步
// ---------------------------------------------------------------------------

/// `POST /api/sessions/copy`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct CopySessionsRequest {
    pub(crate) target_account_id: String,
    pub(crate) session_ids: Vec<String>,
    /// 显式忽略会话账本（用量 + 中断记录）：只由「仍然复制」按钮传 true。
    pub(crate) ignore_aux_data: bool,
}

/// `POST /api/sessions/copy-cross`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct CopySessionsCrossRequest {
    pub(crate) source_account_id: String,
    pub(crate) target_account_id: String,
    pub(crate) session_ids: Vec<String>,
    /// 显式忽略会话账本（用量 + 中断记录）：只由「仍然复制」按钮传 true。
    pub(crate) ignore_aux_data: bool,
}

/// `POST /api/session-links/preview`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SessionLinksPreviewRequest {
    pub(crate) target_account_id: String,
    pub(crate) variant: Option<String>,
}

/// `POST /api/session-links/preview-cross`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SessionLinksPreviewCrossRequest {
    pub(crate) source_account_id: String,
    pub(crate) target_account_id: String,
}

/// `POST /api/session-sync/cross`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SessionSyncCrossRequest {
    pub(crate) source_account_id: String,
    pub(crate) target_account_id: String,
    /// 形状由 `session::parse_sync_selections` 校验（与桌面端同一份实现）。
    pub(crate) sync_selections: Option<Value>,
}

// ---------------------------------------------------------------------------
// 会话组（client / variantScope 是共同前缀）
// ---------------------------------------------------------------------------

/// `POST /api/session-groups/list`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SessionGroupListRequest {
    pub(crate) client: String,
    pub(crate) variant_scope: Option<String>,
}

/// `POST /api/session-groups/detail`、`.../delete`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SessionGroupRequest {
    pub(crate) client: String,
    pub(crate) variant_scope: Option<String>,
    pub(crate) group_id: String,
}

/// `POST /api/session-groups/sync-safe`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SessionGroupSyncSafeRequest {
    pub(crate) client: String,
    pub(crate) variant_scope: Option<String>,
    pub(crate) group_id: String,
    pub(crate) restart: bool,
}

/// `POST /api/session-groups/preview`。
///
/// 预览不接受写入参数（previewToken / mode / restart），与 Tauri command 同形；
/// 不要为了复用 sync 的 DTO 把写入字段塞进来，否则契约检查会把它们当成可下发参数。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SessionGroupPairPreviewRequest {
    pub(crate) client: String,
    pub(crate) variant_scope: Option<String>,
    pub(crate) group_id: String,
    pub(crate) source_member_id: String,
    pub(crate) target_member_id: String,
}

/// `POST /api/session-groups/sync`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SessionGroupPairSyncRequest {
    pub(crate) client: String,
    pub(crate) variant_scope: Option<String>,
    pub(crate) group_id: String,
    pub(crate) source_member_id: String,
    pub(crate) target_member_id: String,
    pub(crate) preview_token: String,
    pub(crate) mode: String,
    pub(crate) restart: bool,
}

/// `POST /api/session-groups/unify`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SessionGroupUnifyRequest {
    pub(crate) client: String,
    pub(crate) variant_scope: Option<String>,
    pub(crate) group_id: String,
    pub(crate) source_member_id: String,
    /// 形状由 `session_groups::sync_unify_batch` 校验（错误文案在 core）。
    pub(crate) targets: Vec<Value>,
    pub(crate) restart: bool,
}

/// `POST /api/session-groups/add`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SessionGroupAddMemberRequest {
    pub(crate) client: String,
    pub(crate) variant_scope: Option<String>,
    pub(crate) group_id: String,
    pub(crate) source_member_id: String,
    pub(crate) target_account_id: String,
    pub(crate) restart: bool,
}

/// `POST /api/session-groups/unlink`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SessionGroupUnlinkRequest {
    pub(crate) client: String,
    pub(crate) variant_scope: Option<String>,
    pub(crate) group_id: String,
    pub(crate) member_id: String,
}

/// `POST /api/session-groups/prune-dead`。
///
/// **刻意不写 `default`**：`dryRun` 是破坏性操作的显式开关，缺字段必须被拒成 400，
/// 不能静默降级成 `false`（那会让一个漏传参数的脚本直接删掉用户全部失效关联组）。
/// 与 Tauri command 的 `dry_run: bool`（必填）保持同一语义：桌面端漏传是调用错误，
/// WebUI 漏传同样是 400。`Option` 字段（`variantScope`）本身仍可缺省。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionGroupPruneRequest {
    pub(crate) client: String,
    pub(crate) variant_scope: Option<String>,
    pub(crate) dry_run: bool,
}

/// `POST /api/session-groups/copy-linked`。
///
/// 不带 `variantScope`：复制由显式来源/目标账号决定，档位取自账号本身；
/// 桌面端 command 也没有这个参数（WebUI 多传的旧字段会被 serde 忽略）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct CopyLinkedSessionsRequest {
    pub(crate) client: String,
    pub(crate) source_account_id: String,
    pub(crate) target_account_id: String,
    pub(crate) session_ids: Vec<String>,
    pub(crate) restart: bool,
}

/// `POST /api/vscode-ext/restart-precheck`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct VscodeRestartPrecheckRequest {
    pub(crate) target_account_ids: Vec<String>,
}

/// `POST /api/jetbrains/restart-precheck`。
///
/// 字段与 [`VscodeRestartPrecheckRequest`] 同形（前端共用一套判定）；JetBrains 侧的判定
/// 与目标账号无关（IDE 内存里只有一个账号，且多个产品共用同一份数据目录）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct JetbrainsRestartPrecheckRequest {
    pub(crate) target_account_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// 存储管理（T3）
// ---------------------------------------------------------------------------

/// `POST /api/storage/sessions`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct StorageSessionsRequest {
    pub(crate) variant: Option<String>,
}

/// `POST /api/storage/clean`。
///
/// `targets` 是嵌套业务结构（`category` / `session` / `redundant` 三种形状），由
/// `storage::clean` 统一校验；边界这里只保证它是数组并拒绝未知顶层字段。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub(crate) struct StorageCleanRequest {
    pub(crate) variant: Option<String>,
    /// 缺省 `true`：没有显式传 false 时只做计划，不做任何写入。
    pub(crate) dry_run: Option<bool>,
    pub(crate) targets: Value,
}

/// `POST /api/storage/trash/restore`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct StorageTrashRequest {
    pub(crate) id: String,
}

/// `POST /api/storage/trash/purge`（`id` 缺省 = 清空回收站）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct StorageTrashPurgeRequest {
    pub(crate) id: Option<String>,
}

/// `POST /api/storage/reveal`。
///
/// `scope` 是第三轮第 5 条新增的可选字段（`storage` 缺省 / `cwd`），与 Tauri command
/// 的同名参数一一对应；缺省时行为与改造前一致。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct StorageRevealRequest {
    pub(crate) path: String,
    pub(crate) mode: Option<String>,
    pub(crate) scope: Option<String>,
}

/// `POST /api/storage/revalidate`（`targets` 是 `kind:id` 字符串数组）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct StorageRevalidateRequest {
    pub(crate) variant: Option<String>,
    pub(crate) targets: Value,
}

// ---------------------------------------------------------------------------
// 设置跨账号同步（T5）
// ---------------------------------------------------------------------------

/// `POST /api/settings-sync/config`（缺省字段保持原值）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SettingsSyncConfigRequest {
    pub(crate) enabled: Option<bool>,
    pub(crate) auto_on_switch: Option<bool>,
}

/// `POST /api/settings-sync/preview`。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SettingsSyncPreviewRequest {
    pub(crate) source_account_id: Option<String>,
}

/// `POST /api/settings-sync/apply`（`dryRun` 缺省 true）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SettingsSyncApplyRequest {
    pub(crate) source_account_id: Option<String>,
    pub(crate) dry_run: Option<bool>,
}

/// `POST /api/settings-sync/restore`：uid 与备份时间戳都走 core 的白名单校验。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SettingsSyncRestoreRequest {
    pub(crate) uid: String,
    pub(crate) stamp: String,
}

/// `POST /api/refresh-official-usage`：无参数，占位 DTO 以满足「POST 必须有明确类型」。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct RefreshOfficialUsageRequest {}

/// `POST /api/save-text-file`（CSV 落盘；扩展名与体积在 core 校验）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SaveTextFileRequest {
    pub(crate) path: String,
    pub(crate) content: String,
}
