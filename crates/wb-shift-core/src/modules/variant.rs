//! WorkBuddy 档位（国内版 / 国际版）的单一事实来源。
//!
//! 档位差异（域名、登录态文件、应用身份、数据根、能力声明）只允许从这里取，
//! 其它模块不得再出现档位相关字面量。缺省档位为 `Cn`，历史数据零迁移。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::modules::config::{
    home_dir, CHECKIN_API_PREFIX, WORKBUDDY_API_ENDPOINT, WORKBUDDY_API_PREFIX, WORKBUDDY_PLATFORM,
};

/// 国际版 API 基址。
const AI_API_ENDPOINT: &str = "https://www.workbuddy.ai";
/// 国际版 OAuth `platform` 参数。
const AI_OAUTH_PLATFORM: &str = "workbuddy-ai";
/// 账号 / 响应 `domain` 判据：这些域的 apex 与其子域都视为国际版。
///
/// 两个产品域都要认：WorkBuddy 国际版写 `www.workbuddy.ai`，CodeBuddy 系（IDE / 插件 /
/// CLI）写 `www.codebuddy.ai`，而导入的第三方号池里 `domain` 常常就是**唯一**档位线索。
/// 只认 `.workbuddy.ai` 时，`www.codebuddy.ai` 会被判成国内版并写进国内桶，切号就会打到
/// 错误的客户端。
const AI_DOMAIN_APEXES: [&str; 2] = ["workbuddy.ai", "codebuddy.ai"];
/// 国际版 billing 接口前缀（实测形态；国内版为 config 里的 `/v2/billing/meter`）。
const AI_BILLING_PREFIX: &str = "/billing/meter";

/// CodeBuddy 系产品域：国内版（客户端 `product.json` 的 internalDomain 成员）。
const CN_CODEBUDDY_DOMAIN: &str = "www.codebuddy.cn";
/// CodeBuddy 系产品域：国际版（客户端 `product.json` 的 externalDomain 成员）。
const AI_CODEBUDDY_DOMAIN: &str = "www.codebuddy.ai";

const CN_AUTH_FILE_NAME: &str = "workbuddy-desktop.info";
const AI_AUTH_FILE_NAME: &str = "workbuddy-desktop-ai.info";
const CN_DATA_ROOT: &str = ".workbuddy";
const AI_DATA_ROOT: &str = ".workbuddy-ai";

const CN_WINDOWS_IMAGES: [&str; 1] = ["WorkBuddy"];
const AI_WINDOWS_IMAGES: [&str; 1] = ["WorkBuddyAI"];

const CN_MACOS_APPS: [&str; 1] = ["WorkBuddy.app"];
const AI_MACOS_APPS: [&str; 1] = ["WorkBuddy AI.app"];
const CN_MACOS_BUNDLE_ID: &str = "com.tencent.workbuddy.mac";
const AI_MACOS_BUNDLE_ID: &str = "com.workbuddy.workbuddy-ai";

const CN_LINUX_APP: &str = "/usr/bin/workbuddy";
const AI_LINUX_APP: &str = "/usr/bin/workbuddy-ai";

/// 目标平台。登录态文件目录按平台分支；显式取值便于单测覆盖三平台。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // 非当前平台的变体只在本模块单测中构造
pub(crate) enum HostOs {
    Macos,
    Windows,
    Linux,
}

impl HostOs {
    pub(crate) fn current() -> Self {
        #[cfg(target_os = "macos")]
        return Self::Macos;
        #[cfg(target_os = "windows")]
        return Self::Windows;
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        return Self::Linux;
    }
}

/// CodeBuddy 扩展数据根（`…/CodeBuddyExtension/Data`，按平台不同）。
///
/// 两个消费者必须同源：登录态文件（[`WbVariant::auth_file_path_at`]）与 Token 统计里的
/// IDE 用量。家目录由调用方注入，因此 `WB_SHIFT_HOME` 的沙箱重定向对两者同时生效——
/// 早期统计侧用 `dirs::data_local_dir()` 自己拼路径，沙箱里 IDE 一栏读的是真实用户数据。
pub(crate) fn extension_data_dir_at(home: &Path, os: HostOs) -> PathBuf {
    let dir = match os {
        HostOs::Macos => "Library/Application Support/CodeBuddyExtension/Data",
        HostOs::Windows => "AppData/Local/CodeBuddyExtension/Data",
        HostOs::Linux => ".local/share/CodeBuddyExtension/Data",
    };
    home.join(dir)
}

/// 注入 CodeBuddy 系目标（VS Code 扩展 / CN IDE / 国际版 IDE）前的产品域规范化。
///
/// WorkBuddy 客户端首次登录写下的 `www.workbuddy.{cn,ai}` 不在 CodeBuddy 客户端
/// `product.json` 的 internalDomain / externalDomain 列表里，扩展会把它归类为
/// `selfhosted`——该分支会读取「企业版端点」设置（`enterpriseEndpoint`），用户一旦配置
/// 就会把请求打到那个端点。这里只映射两个已知产品域；其它域（企业自建 / iOA /
/// cloudHosted / 已是 codebuddy 域）原样透传；空域按档位补产品默认域（避免国际版账号
/// 空域回落到国内默认端点）。WorkBuddy 桌面目标不经过这里，保持原域。
pub fn codebuddy_domain_for(domain: &str, variant: WbVariant) -> String {
    let trimmed = domain.trim();
    if trimmed.is_empty() {
        return variant.codebuddy_domain().to_string();
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "www.workbuddy.cn" | "workbuddy.cn" => CN_CODEBUDDY_DOMAIN.to_string(),
        "www.workbuddy.ai" | "workbuddy.ai" => AI_CODEBUDDY_DOMAIN.to_string(),
        _ => trimmed.to_string(),
    }
}

/// 域名是否为国际版域：命中 apex 本身或其子域，其余一律不算。
///
/// 判据是**标签边界**的后缀匹配（`workbuddy.ai` / `codebuddy.ai`），因此
/// `www.workbuddy.ai.evil.com`、`notworkbuddy.ai` 都不会被误判成国际版；
/// 大小写与前后空白先归一。
fn has_ai_domain_suffix(domain: &str) -> bool {
    let domain = domain.trim().to_ascii_lowercase();
    if domain.is_empty() {
        return false;
    }
    AI_DOMAIN_APEXES.iter().any(|apex| {
        domain == *apex
            || domain
                .strip_suffix(apex)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

/// WorkBuddy 档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WbVariant {
    /// 国内版。
    Cn,
    /// 国际版。
    Ai,
}

impl WbVariant {
    /// 全部档位（遍历入口，如按档位轮询/展示）。
    pub const ALL: [WbVariant; 2] = [WbVariant::Cn, WbVariant::Ai];

    /// 档位名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cn => "cn",
            Self::Ai => "ai",
        }
    }

    /// 解析档位字符串；未知/空/缺省一律回落国内版（向后兼容）。
    pub fn parse(raw: Option<&str>) -> Self {
        match raw
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "ai" | "intl" | "global" => Self::Ai,
            _ => Self::Cn,
        }
    }

    /// 从账号记录判定档位：显式 `variant` 字段优先，其次 `domain` 后缀兜底。
    pub fn from_account(acc: &Value) -> Self {
        if let Some(raw) = acc.get("variant").and_then(Value::as_str).map(str::trim) {
            if !raw.is_empty() {
                return Self::parse(Some(raw));
            }
        }
        if has_ai_domain_suffix(acc.get("domain").and_then(Value::as_str).unwrap_or("")) {
            Self::Ai
        } else {
            Self::Cn
        }
    }

    /// 域名是否与本档位相符。
    ///
    /// 国际版期望 `workbuddy.ai` / `codebuddy.ai` 的 apex 或子域，国内版期望不是这两个
    /// （包括 `www.codebuddy.cn` 与企业自建域）；空域名无法判定，视为相符（由调用方按
    /// 各自的兼容策略决定是否放行）。
    pub fn matches_domain(self, domain: &str) -> bool {
        let domain = domain.trim();
        if domain.is_empty() {
            return true;
        }
        match self {
            Self::Ai => has_ai_domain_suffix(domain),
            Self::Cn => !has_ai_domain_suffix(domain),
        }
    }

    // -----------------------------------------------------------------------
    // 网络
    // -----------------------------------------------------------------------

    /// API 基址。
    pub fn api_endpoint(self) -> &'static str {
        match self {
            Self::Cn => WORKBUDDY_API_ENDPOINT,
            Self::Ai => AI_API_ENDPOINT,
        }
    }

    /// 设备码流程接口前缀（两档位相同）。
    pub fn api_prefix(self) -> &'static str {
        WORKBUDDY_API_PREFIX
    }

    /// CodeBuddy 系目标的规范产品域（注入会话时使用）。
    pub fn codebuddy_domain(self) -> &'static str {
        match self {
            Self::Cn => CN_CODEBUDDY_DOMAIN,
            Self::Ai => AI_CODEBUDDY_DOMAIN,
        }
    }

    /// OAuth `platform` 参数。
    pub fn oauth_platform(self) -> &'static str {
        match self {
            Self::Cn => WORKBUDDY_PLATFORM,
            Self::Ai => AI_OAUTH_PLATFORM,
        }
    }

    /// billing 接口路径候选（顺序即回落顺序）。
    ///
    /// 国内版只返回给定路径本身，与改造前逐字一致；国际版实测走
    /// `/billing/meter/...`，故先试去掉 `/v2` 的形态，再回落 `/v2/...`。
    /// **只有 HTTP 404 才允许换下一个候选**（401/10085/传输错误都不是路径问题）。
    pub fn billing_paths(self, path: &str) -> Vec<String> {
        match self {
            Self::Cn => vec![path.to_string()],
            Self::Ai => {
                let primary = path
                    .strip_prefix(CHECKIN_API_PREFIX)
                    .map(|rest| format!("{AI_BILLING_PREFIX}{rest}"))
                    .unwrap_or_else(|| path.to_string());
                let fallback = format!("/v2{primary}");
                if fallback == primary {
                    vec![primary]
                } else {
                    vec![primary, fallback]
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // 本地
    // -----------------------------------------------------------------------
    /// 官方登录态文件路径（两档位同目录、不同文件）。
    pub fn auth_file_path(self) -> PathBuf {
        self.auth_file_path_at(&home_dir(), HostOs::current())
    }

    fn auth_file_path_at(self, home: &Path, os: HostOs) -> PathBuf {
        // 平台目录口径与 IDE 用量统计共用 `extension_data_dir_at`：两处必须同源，
        // 否则沙箱里会变成「登录态读沙箱、统计读真实家目录」的一半隔离。
        extension_data_dir_at(home, os)
            .join("Public")
            .join("auth")
            .join(self.auth_file_name())
    }

    /// 登录态文件名。
    fn auth_file_name(self) -> &'static str {
        match self {
            Self::Cn => CN_AUTH_FILE_NAME,
            Self::Ai => AI_AUTH_FILE_NAME,
        }
    }

    /// 切换前备份文件名（两档位可区分，避免同名互相覆盖）。
    pub fn backup_file_name(self, ts: &str) -> String {
        let stem = self.auth_file_name().trim_end_matches(".info");
        format!("{stem}.{ts}.info")
    }

    /// 客户端数据根。
    pub fn data_root(self) -> PathBuf {
        self.data_root_at(&home_dir())
    }

    /// 客户端数据根（家目录由调用方注入）。
    ///
    /// 与 [`Self::data_root`] 分开是为了让「数据根怎么算」能脱离环境变量单测：统计等
    /// 模块注入一个临时家目录后，可以断言结果全部落在注入的目录下，而不是真的去读
    /// 用户家目录（见 `token_stats::stats_roots_at` 的沙箱断言）。
    pub fn data_root_at(self, home: &Path) -> PathBuf {
        home.join(match self {
            Self::Cn => CN_DATA_ROOT,
            Self::Ai => AI_DATA_ROOT,
        })
    }

    /// exe 路径缓存的分键（与档位名同形，独立方法便于将来单独演进盘上格式）。
    pub fn exe_cache_key(self) -> &'static str {
        self.as_str()
    }

    // -----------------------------------------------------------------------
    // 应用身份
    // -----------------------------------------------------------------------

    /// Windows 映像名（精确等价匹配，忽略 `.exe` 与大小写）。
    pub fn windows_image_names(self) -> &'static [&'static str] {
        match self {
            Self::Cn => &CN_WINDOWS_IMAGES,
            Self::Ai => &AI_WINDOWS_IMAGES,
        }
    }

    /// macOS app 名（候选目录与进程模式的主名）。
    pub fn macos_app_names(self) -> &'static [&'static str] {
        match self {
            Self::Cn => &CN_MACOS_APPS,
            Self::Ai => &AI_MACOS_APPS,
        }
    }

    /// macOS 优雅退出用的 bundle id。
    pub fn macos_bundle_id(self) -> &'static str {
        match self {
            Self::Cn => CN_MACOS_BUNDLE_ID,
            Self::Ai => AI_MACOS_BUNDLE_ID,
        }
    }

    /// macOS app 路径探测全失败时的回落路径。
    pub fn macos_default_app_path(self) -> PathBuf {
        Path::new("/Applications").join(self.macos_app_names()[0])
    }

    /// Linux 默认可执行文件路径。
    pub fn linux_app_path(self) -> PathBuf {
        PathBuf::from(match self {
            Self::Cn => CN_LINUX_APP,
            Self::Ai => AI_LINUX_APP,
        })
    }

    /// Linux 进程查询模式。
    pub fn linux_process_pattern(self) -> &'static str {
        match self {
            Self::Cn => "workbuddy",
            Self::Ai => "workbuddy-ai",
        }
    }

    // -----------------------------------------------------------------------
    // 能力声明
    // -----------------------------------------------------------------------

    /// 成长中心（派猫猫旅行）仅国内版开放。
    pub fn supports_travel(self) -> bool {
        matches!(self, Self::Cn)
    }

    /// 该档位是否支持签到，**同时门控请求与待签到集合**。
    ///
    /// 国际版没有签到接口：签到链路（自动周期、一键签到、单账号）在发起任何请求前
    /// 一律跳过国际版账号（见 `checkin::checkin_account`）。它们因此永不产生签到
    /// 日志，也不计入「今天是否全部已签到」的待签到集合，否则托盘会一直显示
    /// 「可签到」（见 `checkin::accounts_checked_in_today`）。
    pub fn supports_checkin(self) -> bool {
        matches!(self, Self::Cn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_is_backward_compatible() {
        assert_eq!(WbVariant::parse(None), WbVariant::Cn);
        assert_eq!(WbVariant::parse(Some("")), WbVariant::Cn);
        assert_eq!(WbVariant::parse(Some("   ")), WbVariant::Cn);
        assert_eq!(WbVariant::parse(Some("cn")), WbVariant::Cn);
        assert_eq!(WbVariant::parse(Some(" CN ")), WbVariant::Cn);
        assert_eq!(WbVariant::parse(Some("unknown")), WbVariant::Cn);
        assert_eq!(WbVariant::parse(Some("ai")), WbVariant::Ai);
        assert_eq!(WbVariant::parse(Some("AI")), WbVariant::Ai);
        assert_eq!(WbVariant::parse(Some("intl")), WbVariant::Ai);
        assert_eq!(WbVariant::parse(Some("global")), WbVariant::Ai);
    }

    #[test]
    fn from_account_prefers_explicit_field_then_domain() {
        assert_eq!(WbVariant::from_account(&json!({})), WbVariant::Cn);
        assert_eq!(
            WbVariant::from_account(&json!({"domain": "www.codebuddy.cn"})),
            WbVariant::Cn
        );
        assert_eq!(
            WbVariant::from_account(&json!({"variant": "ai"})),
            WbVariant::Ai
        );
        assert_eq!(
            WbVariant::from_account(&json!({"variant": "cn", "domain": "www.workbuddy.ai"})),
            WbVariant::Cn,
            "显式字段优先于域名兜底"
        );
        assert_eq!(
            WbVariant::from_account(&json!({"variant": "  ", "domain": "www.workbuddy.ai"})),
            WbVariant::Ai,
            "字段为空时回落域名判定"
        );
        assert_eq!(
            WbVariant::from_account(&json!({"domain": "www.workbuddy.ai"})),
            WbVariant::Ai
        );
        assert_eq!(
            WbVariant::from_account(&json!({"domain": "www.workbuddy.ai.evil.com"})),
            WbVariant::Cn,
            "只认后缀，不允许相似域名误判"
        );
    }

    /// issue #232 导出号池兼容：`domain` 是导入时唯一的档位线索，CodeBuddy 国际版域
    /// （含 apex）也必须判成国际版，否则账号会被写进国内桶、切号打到错误的客户端。
    #[test]
    fn from_account_recognizes_both_ai_product_domains() {
        for domain in [
            "www.codebuddy.ai",
            "codebuddy.ai",
            "workbuddy.ai",
            " www.codebuddy.ai ",
            "WWW.CODEBUDDY.AI",
        ] {
            assert_eq!(
                WbVariant::from_account(&json!({ "domain": domain })),
                WbVariant::Ai,
                "`{domain}` 应为国际版"
            );
        }

        // 相似域名仍不得误判（后缀必须落在标签边界上）。
        for domain in [
            "www.codebuddy.ai.evil.com",
            "www.workbuddy.ai.evil.com",
            "notcodebuddy.ai",
        ] {
            assert_eq!(
                WbVariant::from_account(&json!({ "domain": domain })),
                WbVariant::Cn,
                "`{domain}` 不应被判成国际版"
            );
        }

        // 回归：显式 `variant` 仍优先于域名兜底。
        assert_eq!(
            WbVariant::from_account(&json!({"variant": "cn", "domain": "www.codebuddy.ai"})),
            WbVariant::Cn,
            "显式字段优先于域名判定"
        );
    }

    #[test]
    fn serde_round_trip_uses_lowercase_names() {
        assert_eq!(serde_json::to_string(&WbVariant::Cn).unwrap(), "\"cn\"");
        assert_eq!(serde_json::to_string(&WbVariant::Ai).unwrap(), "\"ai\"");
        assert_eq!(
            serde_json::from_str::<WbVariant>("\"ai\"").unwrap(),
            WbVariant::Ai
        );
        assert_eq!(WbVariant::ALL.map(|v| v.as_str()), ["cn", "ai"]);
    }

    #[test]
    fn auth_file_path_covers_three_platforms() {
        let home = Path::new("/home/tester");
        // 比较 Path 而非 to_string_lossy：Windows 的 Path::join 产出 `\` 分隔符，
        // 写死正斜杠的字符串断言会在 Windows 上失败——分隔符不是被测行为的一部分。
        let cn = WbVariant::Cn.auth_file_path_at(home, HostOs::Macos);
        assert_eq!(
            cn,
            home.join("Library/Application Support/CodeBuddyExtension/Data/Public/auth")
                .join("workbuddy-desktop.info")
        );
        let cn_win = WbVariant::Cn.auth_file_path_at(home, HostOs::Windows);
        assert_eq!(
            cn_win,
            home.join("AppData/Local/CodeBuddyExtension/Data/Public/auth")
                .join("workbuddy-desktop.info")
        );
        let cn_linux = WbVariant::Cn.auth_file_path_at(home, HostOs::Linux);
        assert_eq!(
            cn_linux,
            home.join(".local/share/CodeBuddyExtension/Data/Public/auth")
                .join("workbuddy-desktop.info")
        );

        for os in [HostOs::Macos, HostOs::Windows, HostOs::Linux] {
            let ai = WbVariant::Ai.auth_file_path_at(home, os);
            assert!(
                ai.to_string_lossy().ends_with("workbuddy-desktop-ai.info"),
                "国际版文件名: {ai:?}"
            );
            assert_eq!(
                ai.parent(),
                WbVariant::Cn.auth_file_path_at(home, os).parent(),
                "两档位同目录不同文件"
            );
        }
    }

    /// 扩展数据根的三平台口径。
    ///
    /// `extension_data_dir_at` 是从 `auth_file_path_at` 里搬出来的共用口径：这里逐条
    /// 断言它等于登录态用例里写死的同一批期望值，再反向确认登录态文件仍落在它下面
    /// （搬动零行为变化 + 统计侧从此与登录态同源）。
    #[test]
    fn extension_data_dir_matches_platform_conventions() {
        let home = Path::new("/home/tester");
        for (os, expected) in [
            (
                HostOs::Macos,
                "Library/Application Support/CodeBuddyExtension/Data",
            ),
            (HostOs::Windows, "AppData/Local/CodeBuddyExtension/Data"),
            (HostOs::Linux, ".local/share/CodeBuddyExtension/Data"),
        ] {
            let dir = extension_data_dir_at(home, os);
            assert_eq!(dir, home.join(expected), "扩展数据根口径不符: {os:?}");

            for (variant, file_name) in [
                (WbVariant::Cn, CN_AUTH_FILE_NAME),
                (WbVariant::Ai, AI_AUTH_FILE_NAME),
            ] {
                let auth = variant.auth_file_path_at(home, os);
                assert!(
                    auth.starts_with(&dir),
                    "登录态不在数据根下: {}",
                    auth.display()
                );
                assert_eq!(auth, dir.join("Public").join("auth").join(file_name));
            }
        }
    }

    /// 回归：国内版路径与本模块改造前逐字一致。
    #[test]
    fn current_platform_auth_path_keeps_cn_default() {
        #[cfg(target_os = "macos")]
        let legacy = home_dir()
            .join("Library/Application Support/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info");
        #[cfg(target_os = "windows")]
        let legacy = home_dir()
            .join("AppData/Local/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info");
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let legacy = home_dir()
            .join(".local/share/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info");

        assert_eq!(WbVariant::Cn.auth_file_path(), legacy);
        assert_ne!(
            WbVariant::Cn.auth_file_path(),
            WbVariant::Ai.auth_file_path()
        );
    }

    #[test]
    fn backup_file_names_are_distinguishable() {
        let cn = WbVariant::Cn.backup_file_name("2026-09-15T00-00-00Z");
        let ai = WbVariant::Ai.backup_file_name("2026-09-15T00-00-00Z");
        assert_eq!(cn, "workbuddy-desktop.2026-09-15T00-00-00Z.info");
        assert_eq!(ai, "workbuddy-desktop-ai.2026-09-15T00-00-00Z.info");
        assert_ne!(cn, ai);
    }

    #[test]
    fn data_root_and_cache_keys_are_distinguishable() {
        assert!(WbVariant::Cn.data_root().ends_with(CN_DATA_ROOT));
        assert!(WbVariant::Ai.data_root().ends_with(AI_DATA_ROOT));
        assert_ne!(WbVariant::Cn.data_root(), WbVariant::Ai.data_root());
        assert_eq!(WbVariant::Cn.exe_cache_key(), "cn");
        assert_eq!(WbVariant::Ai.exe_cache_key(), "ai");
    }

    #[test]
    fn network_and_identity_differ_by_variant() {
        assert_eq!(WbVariant::Cn.api_endpoint(), "https://www.codebuddy.cn");
        assert_eq!(WbVariant::Ai.api_endpoint(), "https://www.workbuddy.ai");
        assert_eq!(WbVariant::Cn.api_prefix(), "/v2/plugin");
        assert_eq!(WbVariant::Ai.api_prefix(), "/v2/plugin");
        assert_eq!(WbVariant::Cn.oauth_platform(), "workbuddy");
        assert_eq!(WbVariant::Ai.oauth_platform(), "workbuddy-ai");

        assert_eq!(
            WbVariant::Cn.windows_image_names(),
            ["WorkBuddy"].as_slice()
        );
        assert_eq!(
            WbVariant::Ai.windows_image_names(),
            ["WorkBuddyAI"].as_slice()
        );
        assert_eq!(
            WbVariant::Cn.macos_app_names(),
            ["WorkBuddy.app"].as_slice()
        );
        assert_eq!(
            WbVariant::Ai.macos_app_names(),
            ["WorkBuddy AI.app"].as_slice()
        );
        assert_eq!(WbVariant::Cn.macos_bundle_id(), "com.tencent.workbuddy.mac");
        assert_eq!(
            WbVariant::Ai.macos_bundle_id(),
            "com.workbuddy.workbuddy-ai"
        );
        // 断言父目录与文件名，而不是整串字符串：Windows 上 join 产出 `\`。
        let cn_app = WbVariant::Cn.macos_default_app_path();
        assert_eq!(cn_app.parent(), Some(Path::new("/Applications")));
        assert_eq!(cn_app.file_name().unwrap(), "WorkBuddy.app");
        let ai_app = WbVariant::Ai.macos_default_app_path();
        assert_eq!(ai_app.parent(), Some(Path::new("/Applications")));
        assert_eq!(ai_app.file_name().unwrap(), "WorkBuddy AI.app");
    }

    #[test]
    fn capability_declarations() {
        assert!(WbVariant::Cn.supports_travel());
        assert!(!WbVariant::Ai.supports_travel());
        // 国际版无签到接口：既不参与签到请求，也不参与「今天是否全部已签到」判定。
        assert!(WbVariant::Cn.supports_checkin());
        assert!(!WbVariant::Ai.supports_checkin());
    }

    #[test]
    fn billing_paths_keep_cn_single_and_order_ai_fallback() {
        // 国内版：与改造前逐字一致，只有一个候选。
        assert_eq!(
            WbVariant::Cn.billing_paths("/v2/billing/meter/daily-checkin"),
            vec!["/v2/billing/meter/daily-checkin"]
        );
        assert_eq!(
            WbVariant::Cn.billing_paths("/billing/meter/get-user-resource-summary"),
            vec!["/billing/meter/get-user-resource-summary"]
        );

        // 国际版：先 /billing/meter/...，404 才回落 /v2/billing/meter/...
        assert_eq!(
            WbVariant::Ai.billing_paths("/v2/billing/meter/daily-checkin"),
            vec![
                "/billing/meter/daily-checkin",
                "/v2/billing/meter/daily-checkin"
            ]
        );
        assert_eq!(
            WbVariant::Ai.billing_paths("/billing/meter/get-user-resource-summary"),
            vec![
                "/billing/meter/get-user-resource-summary",
                "/v2/billing/meter/get-user-resource-summary"
            ]
        );
    }

    #[test]
    fn matches_domain_rejects_cross_variant_responses() {
        assert!(WbVariant::Ai.matches_domain("www.workbuddy.ai"));
        assert!(WbVariant::Ai.matches_domain(" WWW.WORKBUDDY.AI "));
        // CodeBuddy 系国际版域（issue #232）：apex 与子域都算国际版。
        assert!(WbVariant::Ai.matches_domain("www.codebuddy.ai"));
        assert!(WbVariant::Ai.matches_domain("codebuddy.ai"));
        assert!(WbVariant::Ai.matches_domain("workbuddy.ai"));
        assert!(!WbVariant::Ai.matches_domain("www.workbuddy.ai.evil.com"));
        assert!(!WbVariant::Ai.matches_domain("www.codebuddy.ai.evil.com"));
        assert!(!WbVariant::Ai.matches_domain("www.codebuddy.cn"));

        assert!(WbVariant::Cn.matches_domain("www.codebuddy.cn"));
        assert!(WbVariant::Cn.matches_domain("www.workbuddy.cn"));
        assert!(!WbVariant::Cn.matches_domain("www.workbuddy.ai"));
        // 行为修正（旧口径把它当国内版，导致国际版账号被拒/分错桶）。
        assert!(!WbVariant::Cn.matches_domain("www.codebuddy.ai"));

        // 空域名无法判定：不阻断（由调用方决定兼容策略）。
        assert!(WbVariant::Cn.matches_domain(""));
        assert!(WbVariant::Ai.matches_domain("   "));
    }

    #[test]
    fn codebuddy_domain_normalizes_workbuddy_product_domains() {
        // WorkBuddy 产品域 → 对应 CodeBuddy 产品域（带不带 www 都映射）。
        assert_eq!(
            codebuddy_domain_for("www.workbuddy.cn", WbVariant::Cn),
            "www.codebuddy.cn"
        );
        assert_eq!(
            codebuddy_domain_for("workbuddy.cn", WbVariant::Cn),
            "www.codebuddy.cn"
        );
        assert_eq!(
            codebuddy_domain_for("WWW.WORKBUDDY.AI", WbVariant::Ai),
            "www.codebuddy.ai"
        );

        // 已是 CodeBuddy 域 / 企业自建域 / iOA 域 → 原样透传（不改客户私有部署）。
        assert_eq!(
            codebuddy_domain_for("www.codebuddy.cn", WbVariant::Cn),
            "www.codebuddy.cn"
        );
        assert_eq!(
            codebuddy_domain_for("corp.example.com", WbVariant::Cn),
            "corp.example.com"
        );
        assert_eq!(
            codebuddy_domain_for("tencent.sso.codebuddy.cn", WbVariant::Cn),
            "tencent.sso.codebuddy.cn"
        );

        // 空域 / 纯空白 → 按档位补产品默认域。
        assert_eq!(codebuddy_domain_for("", WbVariant::Cn), "www.codebuddy.cn");
        assert_eq!(
            codebuddy_domain_for("   ", WbVariant::Ai),
            "www.codebuddy.ai"
        );
    }
}
