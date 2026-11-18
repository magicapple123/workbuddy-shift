//! 常量、路径与通用工具函数（对照 server.py 常量区与工具区）

mod configs;
mod http;
mod paths;
mod store;
#[cfg(test)]
mod tests;

pub use self::configs::*;
pub use self::http::*;
pub use self::paths::*;
pub use self::store::*;

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

// 以下三个常量是「国内版」档位的取值来源（档位取值统一见 modules/variant.rs）；
// 新增档位差异不要再新增同类常量。
pub const WORKBUDDY_API_ENDPOINT: &str = "https://www.codebuddy.cn";

pub const WORKBUDDY_API_PREFIX: &str = "/v2/plugin";

pub const WORKBUDDY_PLATFORM: &str = "workbuddy";

pub const OAUTH_TIMEOUT_SECONDS: i64 = 600;

pub const CHECKIN_API_PREFIX: &str = "/v2/billing/meter";

pub const CHECKIN_LOG_KEEP_DAYS: i64 = 30;

pub const CHECKIN_LOG_MAX_RECORDS: usize = 500;

/// 派猫猫旅行接口前缀（成长中心，非 /v2/plugin 体系，直接挂在 API 域名下）。
pub const TRAVEL_API_PREFIX: &str = "/activity/growth/buddy/travel";
