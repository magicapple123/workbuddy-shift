//! HTTP API 层：把 wb-shift-core 暴露为本地 REST 接口，供 webui（浏览器）调用。
//!
//! 路由设计对应 Python 版 server.py 与桌面端 commands.rs。仅绑定 127.0.0.1；
//! token 不出本机。
//!
//! 按业务域拆分为 handlers 子目录；router 是唯一装配点，对外暴露 `router(state)`。

mod dto;
mod handlers;
mod host_guard;
mod response;
mod router;
mod state;
mod static_assets;

pub use router::router;
use state::SharedState;

#[cfg(test)]
mod tests;

// handlers 用 crate::api::* 取 response/state/dto 的公共类型；下面几个只服务 tests.rs，
// 非测试构建不应看到它们（否则 unused_imports 会让 -D warnings 失败）。
pub(crate) use dto::*;
#[cfg(test)]
pub(crate) use handlers::checkin_status_item;
#[cfg(test)]
pub(crate) use host_guard::{host_allowed, origin_allowed};
pub(crate) use response::{
    invalid_body, json_err, json_ok, parse_body, query_flag_enabled, query_param, query_variant,
    submitted_config,
};
pub(crate) use state::{cached_workbuddy_running, AppState};
#[cfg(test)]
pub(crate) use static_assets::static_handler;

/// 测试与 `serve()` 都从这里构造状态：每个 router 拿到独立的切换状态。
pub(crate) fn shared_state() -> SharedState {
    std::sync::Arc::new(AppState::default())
}
