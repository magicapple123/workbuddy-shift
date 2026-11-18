//! 模型限额台账：从**本机日志**与**客户端 hook 信号**还原「哪个账号的哪个模型被限流、
//! 官方给出的恢复时刻」。
//!
//! 两条通路合并（`get_rate_limits()`）：
//! - **hook 通路**（`rate_limit_events.rs`）：CLI 与 WorkBuddy 两档位的 `Stop` / `FinalStop`
//!   payload 里直接带限额文案与模型，实时归因、秒级可见；这些来源已接 hook 时不扫日志。
//! - **日志扫描**（本模块）：两个 CodeBuddy IDE 只走这条（受 `scanIdeLogs` 开关约束）；CLI /
//!   WorkBuddy 只在**该处没接上 hook**（未安装 / 条目被删 / 配置写坏）时按来源逐个回退。
//!   扫描按 5 分钟节流并缓存，`scannedAt` 是最近一次真实扫描的时刻。
//!
//! 扫描范围逐来源判定：**客户端数据根不存在**（本机没装）的来源既不安装 hook 也不扫日志。
//!
//! 不新增任何网络请求，也不解析客户端 UI 文案。日志窗口固定为最近 2 天（WorkBuddy 两档位
//! 与 CLI 是最近 2 个日期目录，两个 IDE 按文件 mtime 收窗）；重置时刻直接采用日志原文 / payload
//! 原文给出的官方值，不自建限流窗口模型。
//!
//! 五个来源各扫一遍（PRD D1：插件宿主共享根 `CodeBuddyExtension/Logs/CodeBuddyIDE/` 与 CN
//! IDE 真身重复记录同一次 429，**不扫**）：
//!
//! | 来源 | 日志根 | 格式 | 枚举 | 归因 |
//! |---|---|---|---|---|
//! | WorkBuddy 国内版 / 国际版 | `~/.workbuddy*/logs` | `WorkBuddy` | 日期目录 | `sessions` 表 |
//! | CodeBuddy CLI | `~/.codebuddy/logs` | `WorkBuddy` | 日期目录 | 日志内鉴权 uid → 轮换状态文件 |
//! | CodeBuddy IDE | `<data_dir>/logs` | `Ide` | 会话目录 + mtime | 日志内鉴权 uid → IDE 状态文件 |
//! | CodeBuddy CN IDE | `<data_dir>/logs` | `Ide` | 会话目录 + mtime | 同上 |
//!
//! 一次返回全部账号的当前受限状态——扫描本身就是全局的，按账号调用会把同一份日志扫 N 遍。
//!
//! 数据流：目录枚举 → 字节级粗筛 → 命中才逐行解码 → 事件与模型解析 → 两步去重 →
//! 账号归因 → 按 (账号, 模型) 聚合 → 过滤掉已过官方重置时刻的条目。

mod ledger;
mod parse;
mod payload;
mod scan;
#[cfg(test)]
mod tests;

pub use self::ledger::*;
pub(crate) use self::parse::*;
pub(crate) use self::payload::*;
pub(crate) use self::scan::*;
