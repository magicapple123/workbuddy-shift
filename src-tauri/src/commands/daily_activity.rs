//! 国际版每日活跃会话（issue #214）的**手动**触发入口。
//!
//! 唯一 Tauri IPC 边界的一部分：只做「读账号库 + 调 core + 错误字符串化」，
//! 会话流程（建会话 → ACP → 轮询 → 收尾）全部在
//! `wb_shift_core::modules::daily_activity`。
//!
//! # 只手动触发
//!
//! 这里**没有**定时器、没有启动时调用、也不挂到切号 / 签到 / 令牌刷新流程里：
//! 前端不点账号页的「每日活跃」按钮，就一个请求都不会发（core 模块文档同样承诺
//! 这一点，见 `daily_activity/mod.rs` 的「边界」小节）。
//!
//! # 为什么账号由后端读取
//!
//! 每日活跃的去重状态按账号 key 落盘（`daily_activity_state.json`）。让调用方传
//! 账号子集，就能绕过「今天已经做过」的判定重复消耗额度，因此命令不接受参数、
//! 统一读账号库；与 HTTP `POST /api/daily-activity/run` 同源同形。

//! # 为什么账号库读取走 blocking 线程
//!
//! `account::load_accounts()` 是**阻塞 fs**（账号多时 `accounts.json` 可能几十 MB），
//! 直接在 async command 里跑会占住 async worker；与 HTTP 侧
//! `POST /api/daily-activity/run`（`handlers/daily_activity.rs` 的
//! `spawn_blocking(load)`）保持同一标准。会话流程本身是 async（等待在网络），
//! 因此**不**塞进 blocking 池——那需要在里面 `block_on`，是当前运行时的反模式。

use serde_json::Value;
use wb_shift_core::modules::{account, daily_activity};

/// POST /api/daily-activity/run —— 为账号库里的每个账号跑一次每日活跃会话。
///
/// 返回 `{"date": "<本地日期>", "accounts": [逐账号结果...]}`，**原样透传** core 的
/// 结果：单个账号失败只出现在它自己那一行，不会打断其它账号；`Err` 仅用于
/// 「连 HTTP 客户端都构造不出来」这类整体性失败（core 的约定）。
#[tauri::command]
pub async fn run_daily_activity() -> Result<Value, String> {
    // 只把阻塞的账号库读取丢进 blocking 线程（与 `get_status`、`export_accounts_encrypted`
    // 同一写法）；join 失败 = blocking 池里 panic 了，宿主层不能让它外泄，转成可读文案。
    let accounts = tauri::async_runtime::spawn_blocking(account::load_accounts)
        .await
        .map_err(|error| format!("读取账号库失败: {error}"))?;
    // 逐账号会话流程是 async（建会话 / 轮询 / 收尾都在等网络），直接 await，别进 blocking 池。
    daily_activity::run_daily_activity_once(&accounts).await
}
