//! 监听地址解析：把 `WB_SHIFT_LISTEN` 与 `--port` 合成一个真实可绑定的 `host:port`。
//!
//! 为什么单独成模块而不是塞进 `main.rs`：解析规则必须能单测（纯函数、不读环境变量），
//! `serve()` 只负责把结果交给 `TcpListener`。默认值必须与改造前逐字一致
//! （`127.0.0.1:57910`），放开监听只能来自显式环境变量。
//!
//! 为什么只接受 IP 字面量：这个值的用途只有两种——容器内绑 `0.0.0.0`、多网卡机器绑
//! 某张网卡。域名要靠 DNS 解析，解析结果会随网络变化，把「绑定失败」变成一个不确定的
//! 时序问题；需要域名访问请用反向代理，不要靠本进程绑域名。

use std::net::IpAddr;

/// 覆盖监听地址的环境变量名。
///
/// 不提供等价的命令行开关是刻意的：`--port` 已经在帮助里说明「只影响端口」，
/// 多一个 `--listen` 就多一次误传成对外暴露的机会。部署形态（Docker）用环境变量表达。
pub const LISTEN_ENV_VAR: &str = "WB_SHIFT_LISTEN";

/// 默认监听地址：只回环。与改造前完全一致。
pub const DEFAULT_LISTEN_HOST: &str = "127.0.0.1";

/// 默认端口。
pub const DEFAULT_PORT: u16 = 57910;

/// 解析 `WB_SHIFT_LISTEN`，与 `--port` 合成最终监听地址。
///
/// 第二项是给调用方打印的告警（取值非法或**显式为空**时非空）。**不 panic**：宿主层是
/// crate 级 `deny(unwrap_used, expect_used)`，而且镜像里写错一个环境变量不应该让服务
/// 起不来——回落到默认回环地址至少还能 `docker exec` 进去排查。
///
/// 「没有这个变量」与「变量给了空值」严格区分：前者是正常默认（无告警），后者多半是
/// 编排里写错了（`-e WB_SHIFT_LISTEN=`），必须留一条日志，否则容器只绑回环、外部连不上，
/// 而容器内的回环探测（健康检查）照样 healthy，现场无从下手。
///
/// 端口优先级：`--port` > `WB_SHIFT_LISTEN` 里的端口 > 默认 57910
/// （`--port` 是操作者当次命令的一部分，比镜像/编排里的固定环境变量更具体）。
pub fn resolve_listen(raw: Option<&str>, cli_port: Option<u16>) -> (String, Option<String>) {
    let fallback_port = cli_port.unwrap_or(DEFAULT_PORT);
    let fallback = format!("{DEFAULT_LISTEN_HOST}:{fallback_port}");
    let Some(raw) = raw else {
        return (fallback, None);
    };
    let text = raw.trim();
    if text.is_empty() {
        // 显式给了变量却是空值，不能当成「没设置」静默放过：`-e WB_SHIFT_LISTEN=` 会让容器
        // 只绑回环，于是外部连不上、而容器内的回环探测（健康检查）照样 healthy——这种
        // 「看起来正常但就是打不开」的现场最难排查，必须留一条日志。
        return (
            fallback.clone(),
            Some(format!(
                "[监听] {LISTEN_ENV_VAR} 为空，已按未设置处理，使用 {fallback}\
                 （要监听所有网卡请写 0.0.0.0）。"
            )),
        );
    }
    match parse_listen_value(text) {
        Ok((ip, env_port)) => (
            format_host_port(ip, cli_port.or(env_port).unwrap_or(DEFAULT_PORT)),
            None,
        ),
        Err(reason) => (
            fallback.clone(),
            Some(format!(
                "[监听] {LISTEN_ENV_VAR}=\"{text}\" 无法解析：{reason}；已回落到 {fallback}。\
                 合法写法：0.0.0.0、0.0.0.0:57910、[::]:57910（只接受 IP 字面量，端口可省略）。"
            )),
        ),
    }
}

/// 监听地址是否只覆盖回环（`127.0.0.1` / `::1`）。
///
/// 解析失败按「非回环」处理：宁可多打一行安全提示，也不要让危险监听静默通过。
pub fn is_loopback(addr: &str) -> bool {
    match parse_listen_value(addr) {
        Ok((ip, _)) => ip.is_loopback(),
        Err(_) => false,
    }
}

/// 打开浏览器时该用的 `host:port`。
///
/// `0.0.0.0` / `::` 是「所有网卡」而不是可访问的主机名，浏览器打不开；换成同端口的
/// 回环地址。绑定了具体网卡地址时原样使用（此时回环可能根本不通）。
pub fn open_target(addr: &str) -> String {
    match parse_listen_value(addr) {
        Ok((ip, port)) => {
            let port = port.unwrap_or(DEFAULT_PORT);
            if ip.is_unspecified() {
                format!("127.0.0.1:{port}")
            } else {
                format_host_port(ip, port)
            }
        }
        // 走到这里说明调用方传了非本模块解析出的地址；原样返回，交给调用方自己处理。
        Err(_) => addr.to_string(),
    }
}

/// 解析 `[地址][:端口]`。
///
/// 顺序有意义：先整体当 IP 试（这样裸 `::` / `::1` 也能用），再处理带方括号的 IPv6
/// （`[::]:57910`），最后才按 `地址:端口` 拆。反过来的话 `::1` 会被拆成 `::` + `1`。
fn parse_listen_value(text: &str) -> Result<(IpAddr, Option<u16>), String> {
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Ok((ip, None));
    }
    if let Some(rest) = text.strip_prefix('[') {
        let Some((inner, tail)) = rest.split_once(']') else {
            return Err("IPv6 地址缺少右方括号（应写作 [::]:57910）".to_string());
        };
        let ip = parse_ip(inner)?;
        if tail.is_empty() {
            return Ok((ip, None));
        }
        let Some(port) = tail.strip_prefix(':') else {
            return Err("右方括号之后只能是 :端口".to_string());
        };
        return Ok((ip, Some(parse_port(port)?)));
    }
    let Some((host, port)) = text.rsplit_once(':') else {
        return Err(format!("{text} 不是合法的 IP 地址"));
    };
    Ok((parse_ip(host)?, Some(parse_port(port)?)))
}

fn parse_ip(text: &str) -> Result<IpAddr, String> {
    text.parse::<IpAddr>()
        .map_err(|_| format!("{text} 不是合法的 IP 地址"))
}

fn parse_port(text: &str) -> Result<u16, String> {
    if text.is_empty() {
        return Err("端口为空".to_string());
    }
    match text.parse::<u16>() {
        // 端口 0 会让系统随机分配，横幅和健康检查都会失准，按非法值处理。
        Ok(0) => Err("端口 0 会让系统随机分配，无法得到固定访问地址".to_string()),
        Ok(port) => Ok(port),
        Err(_) => Err(format!("{text} 不是 0-65535 的端口号")),
    }
}

fn format_host_port(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(v4) => format!("{v4}:{port}"),
        // IPv6 必须带方括号，否则 `::1:57910` 会被再解析成一个合法但错误的 IPv6 地址。
        IpAddr::V6(v6) => format!("[{v6}]:{port}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{is_loopback, open_target, resolve_listen};

    /// 缺省（环境变量完全没设置）必须与改造前完全一致，且不产生告警。
    #[test]
    fn resolve_listen_defaults_to_loopback() {
        let (addr, warning) = resolve_listen(None, None);
        assert_eq!(addr, "127.0.0.1:57910");
        assert!(warning.is_none(), "未设置环境变量时不应有告警");
    }

    /// 显式空值（`-e WB_SHIFT_LISTEN=` / 只有空白）不算「未设置」：地址照旧回落，
    /// 但必须留一条告警——否则容器只绑回环、外部连不上而容器内探测健康，无法定位。
    #[test]
    fn resolve_listen_warns_on_explicitly_empty_value() {
        for raw in ["", "   ", "\t"] {
            let (addr, warning) = resolve_listen(Some(raw), None);
            assert_eq!(addr, "127.0.0.1:57910", "{raw:?}");
            let warning = warning.unwrap_or_else(|| panic!("{raw:?} 应给出告警"));
            assert!(warning.contains("WB_SHIFT_LISTEN"), "{raw:?}: {warning}");
            assert!(warning.contains("为空"), "{raw:?}: {warning}");
        }
        // 空值回落仍尊重 --port。
        let (addr, warning) = resolve_listen(Some(""), Some(58003));
        assert_eq!(addr, "127.0.0.1:58003");
        assert!(warning.is_some());
    }

    /// `--port` 单独生效时地址仍是回环。
    #[test]
    fn resolve_listen_applies_cli_port_only() {
        let (addr, warning) = resolve_listen(None, Some(58000));
        assert_eq!(addr, "127.0.0.1:58000");
        assert!(warning.is_none());
    }

    /// 环境变量里的三种写法：只给地址、地址+端口、IPv6 带括号。
    #[test]
    fn resolve_listen_accepts_ip_literals() {
        let cases = [
            ("0.0.0.0", "0.0.0.0:57910"),
            ("0.0.0.0:8080", "0.0.0.0:8080"),
            ("[::]:57910", "[::]:57910"),
            ("[::]", "[::]:57910"),
            ("::1", "[::1]:57910"),
            (" 192.168.1.5:57910 ", "192.168.1.5:57910"),
        ];
        for (raw, expected) in cases {
            let (addr, warning) = resolve_listen(Some(raw), None);
            assert_eq!(addr, expected, "{raw}");
            assert!(warning.is_none(), "{raw} 不应产生告警");
        }
    }

    /// 端口优先级：`--port` > 环境变量里的端口 > 默认。
    #[test]
    fn resolve_listen_prefers_cli_port() {
        let (addr, warning) = resolve_listen(Some("0.0.0.0:8080"), Some(58000));
        assert_eq!(addr, "0.0.0.0:58000");
        assert!(warning.is_none());
        // 环境变量只给地址时，--port 也照常生效。
        let (addr, _) = resolve_listen(Some("0.0.0.0"), Some(58001));
        assert_eq!(addr, "0.0.0.0:58001");
    }

    /// 非法取值：回落 127.0.0.1 + 可读告警，绝不 panic。
    #[test]
    fn resolve_listen_falls_back_on_invalid_values() {
        for raw in [
            "nas.local",           // 域名不支持
            "0.0.0.0:abc",         // 端口不是数字
            "0.0.0.0:0",           // 随机端口
            "0.0.0.0:",            // 空端口
            "[::1",                // 缺右括号
            "[::1]evil",           // 括号后有多余字符
            "999.0.0.1:57910",     // 非法 IPv4
            "0.0.0.0:57910:extra", // 多余的冒号
        ] {
            let (addr, warning) = resolve_listen(Some(raw), None);
            assert_eq!(addr, "127.0.0.1:57910", "{raw}");
            let warning = warning.unwrap_or_else(|| panic!("{raw} 应给出告警"));
            assert!(warning.contains("WB_SHIFT_LISTEN"), "{raw}: {warning}");
            assert!(warning.contains("127.0.0.1:57910"), "{raw}: {warning}");
        }
    }

    /// 非法取值回落时仍尊重 `--port`：否则容器里写错地址会连带改掉端口。
    #[test]
    fn resolve_listen_fallback_keeps_cli_port() {
        let (addr, warning) = resolve_listen(Some("nas.local"), Some(58002));
        assert_eq!(addr, "127.0.0.1:58002");
        assert!(warning.is_some());
    }

    /// 回环判定决定横幅是否打安全提示，宁可判「非回环」也不要漏。
    #[test]
    fn is_loopback_only_for_loopback_addresses() {
        for addr in ["127.0.0.1:57910", "[::1]:57910", "127.0.0.1"] {
            assert!(is_loopback(addr), "{addr}");
        }
        for addr in ["0.0.0.0:57910", "[::]:57910", "192.168.1.5:57910", ""] {
            assert!(!is_loopback(addr), "{addr}");
        }
    }

    /// 浏览器打开的目标地址：通配地址换回环，具体地址原样保留。
    #[test]
    fn open_target_replaces_unspecified_host() {
        assert_eq!(open_target("0.0.0.0:57910"), "127.0.0.1:57910");
        assert_eq!(open_target("[::]:57910"), "127.0.0.1:57910");
        assert_eq!(open_target("127.0.0.1:57910"), "127.0.0.1:57910");
        assert_eq!(open_target("192.168.1.5:57910"), "192.168.1.5:57910");
    }
}
