//! 诊断包：把排障需要的事实汇总成**一份可直接粘贴进 issue 的脱敏文本**。
//!
//! # 为什么需要
//!
//! 用户来提 issue 时给不出可复现信息（版本？宿主？哪个客户端没装？日志最后一条说什
//! 么？），作者只能来回问三轮。这里把「作者必问的那几项」一次性收齐并脱敏，用户点一下
//! 导出、整段粘贴即可。
//!
//! # 硬边界（结构性保证，不只是注释）
//!
//! 本模块**不得** `use` `account` / `auth_file` / `oauth` / `session` 任何一层：
//!
//! - 账号数量由宿主层（已持有账号库的那一层）以 `usize` 传入，这里只做**区间分桶**；
//! - 客户端探测结果、宿主形态、界面语言同样由宿主以纯值传入；
//! - 设置项只导出**键名 + 值类型**，取值一律不读（`export_settings_bundle()` 的产物
//!   在这里被投影成 `键名 → "boolean" / "string" / ...`，值本身不进入任何字符串）。
//!
//! 因此「不小心把 `accounts.json` 的字段带出来」不是靠评审盯，而是**编译期就不可能**：
//! 本模块的 `use` 列表里没有任何读取账号 / 令牌 / 会话的入口。
//!
//! # 外形
//!
//! 输出**单个 `.txt`**（分段标题 + 缩进），不做 zip：issue 正文里 zip 没法看，
//! 而「能直接看懂的一段文本」才是作者要的。项目本来也没有 zip 依赖。
//!
//! # 与 webui 的关系
//!
//! 落盘只在桌面宿主做（对齐 `canPersistErrorLog()` 的既有约定）；webui 拿到同一份文本
//! 后由前端复制，服务端不写用户机器上的任意路径。

mod redact;
mod report;

pub use self::redact::{
    redact, redact_text, redact_with_user, Redacted, MASK_EMAIL, MASK_NUM, MASK_PATH, MASK_TOKEN,
    MASK_URL, MASK_USER, MASK_UUID,
};
pub use self::report::{
    account_bucket, build_report, build_report_default, save_report, ClientProbe, HookState,
    HostForm, ReportInput, ReportSources, MAX_REPORT_BYTES, REPORT_EXTENSIONS,
};

#[cfg(test)]
mod tests;
