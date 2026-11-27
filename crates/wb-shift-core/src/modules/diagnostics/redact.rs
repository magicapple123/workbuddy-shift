//! 诊断包的脱敏纯函数：**一个原文片段都不许漏进导出文件**。
//!
//! # 与仓库既有两套脱敏的关系
//!
//! 仓库里已经有两套脱敏：`limits::vscode_ext::redact`（邮件 / 32 位以上十六进制 id /
//! `Bearer` token → `<email>` 这类尖括号占位符）与 `daily_activity::scrub` /
//! `redact_url`（URL 截到 `?` / `#` 之前）。
//!
//! **与 `vscode_ext::redact` 只对齐「风格」，不共享实现**：占位符统一是
//! `<email>` / `<token>` 这类尖括号写法（不另造第四套），但两份实现各自独立维护——
//! `limits` 与 `diagnostics` 互不依赖（见 `AGENTS.md` 第 2 节的依赖方向），把其中一个
//! 模块的私有函数抽出来共用会立刻制造一条新的跨模块依赖。三套实现并存是历史原因，
//! 后续收敛时以本文件为基线（它覆盖最全、且是唯一被单测逐片段断言过的）。
//!
//! 本文件多出来的能力（既有实现没有、诊断包必须有）：家目录路径、UUID、长数字、
//! 含空格路径的延续判定、邮箱后的句末句点；URL 则保留 `scheme://host` 便于判断是哪家
//! 网关，路径与 query 抹成 `<url>`。`Bearer` 短令牌两家都有，但判据不同（见下文
//! 「两条看前一个词的规则」）。
//!
//! # 为什么单独一个文件
//!
//! 脱敏是整个诊断包里唯一「错了就等于泄密」的部分，因此它必须是纯函数：
//! 输入字符串、输出字符串，没有 IO、没有全局状态、没有分支依赖宿主形态。
//! 这样它才能被单测直接喂「一条含真实令牌 / 邮箱 / 路径的日志」，并逐片段断言。
//!
//! # 取舍：宁可比必要的更狠
//!
//! 判据刻意**过度匹配**：一段看着像路径 / 令牌的东西一律抹掉，哪怕它其实是无害文案。
//!
//! 一个已知的具体后果：**`KEY=长令牌` 形态会把键名一起替换掉**（`GH_TOKEN=ghp_…`
//! 的 `_` 与 `=` 都是词字符，整词变 `<token>`，`GH_TOKEN=` 也一起没了）。这是**有意取舍**，
//! 不是 bug：少一个键名只让报告作者少一个线索，反方向漏一个令牌是事故。缺键名的场景
//! 请直接问用户要变量名，不要为了「保留键名」放宽这条规则。
//! 诊断包的价值是「能看出哪个环节坏了」，不是「保留全部原始信息」；少一个字符的代价
//! 远小于多泄露一个令牌。方向错了只会让作者多问一句，反方向错了是事故。
//!
//! # 两条「看前一个词」的规则
//!
//! 单看一个词判不出来的两种情况，靠 `TokenContext` 记住前一个词：
//!
//! - `Authorization: Bearer sk-secret-123`：长度规则只认 ≥32 字符，13 字符的短令牌会漏，
//!   因此 `Bearer`（大小写不敏感）后面的词一律按令牌抹掉；
//! - `/Users/a/Library/Application Support/WorkBuddy/`：含空格的路径会被空格切成两个词，
//!   前一个词判成路径后，后续仍含 `/` 的片段继续按路径抹（含 `3/4` 这类短比值不误伤）。
//!
//! # 顺序（敏感，改之前先看单测）
//!
//! 逐「词」判定，**先认最具体的形态，最后才认数字**：
//! URL → 盘符路径 → 邮箱 → UUID → JWT → Unix 路径 → 长令牌 → 长数字 → 行尾长数字 →
//! 用户名。顺序反了会互相抢匹配：`550e8400-e29b-41d4-a716-446655440000` 若先走数字规则
//! 会被拆成 `550e8400-e29b-41d4-a716-` + `<num>`；`https://a.com/u@b.com` 若先走邮箱规则
//! 会把域名切碎。
//!
//! 中文（非 ASCII）不参与「长令牌」判定：中文正文没有空格，一整句会被当成一个词，
//! 若允许非 ASCII 参与长度判定，整段用户可读文案都会被替换成 `<token>`。

/// 路径占位符（家目录、绝对路径、Windows 盘符路径共用）。
pub const MASK_PATH: &str = "<path>";
/// URL 里被抹掉的部分（scheme + host 保留，路径与 query 换成它）。
pub const MASK_URL: &str = "<url>";
/// UUID 占位符。
pub const MASK_UUID: &str = "<uuid>";
/// 疑似令牌（长 base64 / JWT）占位符。
pub const MASK_TOKEN: &str = "<token>";
/// 邮箱占位符。
pub const MASK_EMAIL: &str = "<email>";
/// 家目录用户名占位符。
pub const MASK_USER: &str = "<user>";
/// 长数字串占位符（毫秒时间戳、uid、订单号等）。
pub const MASK_NUM: &str = "<num>";

/// 长数字的判定阈值：7 位足够覆盖毫秒时间戳 / 手机号 / 大多数内部 id，
/// 又不会把 `HTTP 401`、`第 3 条`、`2024` 这类正常数字一起吃掉。
const NUM_MIN_DIGITS: usize = 7;
/// 长令牌的判定阈值：32 位是 base64 令牌（至少 24 字节熵）的下限。
const TOKEN_MIN_CHARS: usize = 32;

/// 脱敏结果：文本 + 替换次数（次数进报告头，用户能看出「这份确实脱敏过」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redacted {
    /// 脱敏后的文本。
    pub text: String,
    /// 一共替换了多少个片段（不是字符数）。
    pub replacements: usize,
}

/// 词与词之间的上下文：两条规则光看单个词判不出来，必须看前一个词。
#[derive(Default)]
struct TokenContext {
    /// 前一个词被判成路径：含空格的路径会被空格切成多个词，后续片段要跟着继续抹。
    previous_path: bool,
    /// 前一个词是 `Bearer`：`Authorization: Bearer sk-secret-123` 里的短令牌只能靠上下文认。
    previous_bearer: bool,
}

/// 按指定用户名脱敏（`None` 时不做用户名整词替换）。
///
/// 生产入口用 [`redact`]；单测用这个显式传用户名，避免依赖运行测试的机器。
pub fn redact_with_user(text: &str, user: Option<&str>) -> Redacted {
    let mut out = String::with_capacity(text.len());
    let mut replacements = 0usize;
    let mut token = String::new();
    let mut context = TokenContext::default();

    for ch in text.chars() {
        if is_token_char(ch) {
            token.push(ch);
            continue;
        }
        flush(&mut token, user, &mut context, &mut out, &mut replacements);
        out.push(ch);
    }
    flush(&mut token, user, &mut context, &mut out, &mut replacements);

    Redacted {
        text: out,
        replacements,
    }
}

/// 生产入口：按 `home_dir()` 的最后一段做用户名脱敏。
pub fn redact(text: &str) -> Redacted {
    redact_with_user(text, current_user().as_deref())
}

/// 只要文本、不要替换计数的便捷入口。
pub fn redact_text(text: &str) -> String {
    redact(text).text
}

/// 当前用户家目录的最后一段（`None` 表示取不到）。
///
/// 只取最后一段：整条家目录路径已经会被路径规则抹掉，这里补的是「用户名单独出现在正文里」
/// 那种情况（例如日志里写了一句 `user alex is not logged in`）。
fn current_user() -> Option<String> {
    let home = crate::modules::config::home_dir();
    let name = home.file_name()?.to_str()?.trim().to_string();
    // 太短的名字（`a` / `ab`）整词替换会误伤正常英文单词，宁可不替换。
    (name.len() >= 3).then_some(name)
}

/// 词字符：ASCII 字母数字 + 常见分隔符，以及任意非 ASCII 字母数字（中文、日文……）。
///
/// 中文进词集合是为了让 `路径:C:\Users\alex\x.txt` 这种混排整体落到同一个词里被路径规则
/// 一次性抹掉；中文本身不会被「长令牌」规则命中（那条要求全 ASCII）。
fn is_token_char(ch: char) -> bool {
    if ch.is_ascii() {
        return ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                '-' | '_' | '.' | '/' | ':' | '\\' | '@' | '+' | '%' | '=' | '&' | '?' | '#' | '~'
            );
    }
    ch.is_alphanumeric()
}

/// 处理一个完整的词：需要脱敏就写占位符，否则原样写出。
///
/// 上下文两条规则（见 `TokenContext`）：
/// - `Bearer` 后面的词**一律**当令牌抹掉（哪怕只有十来个字符）；
/// - 前一个词是路径时，后续仍含 `/` 的片段继续按路径抹（`/Users/a/Library/Application
///   Support/WorkBuddy/` 被空格切开后，后半截不能漏出去）。
fn flush(
    token: &mut String,
    user: Option<&str>,
    context: &mut TokenContext,
    out: &mut String,
    replacements: &mut usize,
) {
    if token.is_empty() {
        return;
    }
    let is_bearer_word = token.eq_ignore_ascii_case("Bearer");
    let masked: Option<String> = if is_bearer_word {
        // 标签词本身不是机密，保留它才能看懂「这是哪种认证头」。
        None
    } else if let Some(value) = mask_token(token, user) {
        Some(value)
    } else if context.previous_bearer {
        Some(MASK_TOKEN.to_string())
    } else if context.previous_path && token.len() > 3 && token.contains('/') {
        Some(MASK_PATH.to_string())
    } else {
        None
    };

    context.previous_path = masked.as_deref() == Some(MASK_PATH);
    context.previous_bearer = is_bearer_word;

    match masked {
        Some(value) => {
            out.push_str(&value);
            *replacements += 1;
        }
        None => out.push_str(token),
    }
    token.clear();
}

/// 单个词的脱敏判定；返回 `None` 表示「照原样保留」。
fn mask_token(token: &str, user: Option<&str>) -> Option<String> {
    if token.contains("://") {
        return Some(mask_url(token));
    }
    // Windows 盘符路径 / UNC：`C:\...`、`C:/...`、`\\server\share`。
    // 混合文本里前面可能还挂着别的东西，因此用「包含」而不是「以…开头」。
    if token.contains(":\\") || token.contains(":/") || token.contains("\\\\") {
        return Some(MASK_PATH.to_string());
    }
    if let Some(masked) = mask_email_with_suffix(token) {
        return Some(masked);
    }
    if is_uuid(token) {
        return Some(MASK_UUID.to_string());
    }
    if is_jwt(token) {
        return Some(MASK_TOKEN.to_string());
    }
    if is_unix_path(token) {
        return Some(MASK_PATH.to_string());
    }
    // 令牌先于数字：长 base64 里可能整段都是数字开头，先认数量级更长的那类形态。
    if is_long_token(token) {
        return Some(MASK_TOKEN.to_string());
    }
    if is_long_number(token) {
        return Some(MASK_NUM.to_string());
    }
    if let Some(masked) = mask_trailing_number(token) {
        return Some(masked);
    }
    if let Some(name) = user {
        if token.eq_ignore_ascii_case(name) {
            return Some(MASK_USER.to_string());
        }
    }
    None
}

/// URL：保留 `scheme://host`（用户据此判断是哪家网关），路径 / 查询串整体抹掉。
///
/// userinfo（`https://user:pass@host/…`）里的口令同样属于机密，一并丢掉。
fn mask_url(token: &str) -> String {
    let (scheme, rest) = match token.split_once("://") {
        Some(parts) => parts,
        None => return MASK_PATH.to_string(),
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let path = &rest[authority_end..];
    let host = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    if host.is_empty() {
        return MASK_PATH.to_string();
    }
    if path.is_empty() {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}/{MASK_URL}")
    }
}

/// 邮箱 + 句末的 ASCII 句点：`alice@example.com.` → `<email>.`
///
/// 英文日志里「句末点」很常见；直接判 `ends_with('.')` 会让整段邮箱漏出去，
/// 因此先把尾部句点剥掉再判，抹完把句点原样接回去（可读性不变）。
fn mask_email_with_suffix(token: &str) -> Option<String> {
    let trimmed = token.trim_end_matches('.');
    if trimmed.len() == token.len() {
        return mask_email(token).map(str::to_string);
    }
    mask_email(trimmed).map(|mask| format!("{mask}{}", &token[trimmed.len()..]))
}

/// 邮箱：`local@domain.tld`。`@` 前后都要非空，域名部分必须含 `.`。
fn mask_email(token: &str) -> Option<&'static str> {
    let (local, domain) = token.split_once('@')?;
    if local.is_empty() || domain.is_empty() {
        return None;
    }
    if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
        return None;
    }
    if domain.contains('/') || domain.contains('\\') {
        return None;
    }
    Some(MASK_EMAIL)
}

/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`（8-4-4-4-12 十六进制）。
fn is_uuid(token: &str) -> bool {
    let groups: Vec<&str> = token.split('-').collect();
    let sizes = [8usize, 4, 4, 4, 12];
    if groups.len() != sizes.len() {
        return false;
    }
    groups
        .iter()
        .zip(sizes)
        .all(|(group, size)| group.len() == size && group.chars().all(|ch| ch.is_ascii_hexdigit()))
}

/// JWT：三段 base64url，头部以 `eyJ`（`{"` 的 base64）开头。
fn is_jwt(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    parts.len() == 3
        && parts[0].starts_with("eyJ")
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(is_base64_url_char))
}

/// 绝对 / 相对 Unix 路径。
///
/// 要求至少两段，且（以 `/` 开头 或 至少一段带扩展名）：`3/4`、`and/or` 这类正常文本
/// 不能被当成路径，否则日志正文会被抹得读不出意思。
fn is_unix_path(token: &str) -> bool {
    if !token.contains('/') || token.chars().any(char::is_whitespace) {
        return false;
    }
    let segments: Vec<&str> = token.split('/').filter(|part| !part.is_empty()).collect();
    if segments.len() < 2 {
        return false;
    }
    if token.starts_with('/') {
        return true;
    }
    segments
        .iter()
        .any(|part| part.len() > 1 && part.contains('.'))
}

/// 长数字串（毫秒时间戳、uid、手机号……）。
fn is_long_number(token: &str) -> bool {
    token.len() >= NUM_MIN_DIGITS && token.chars().all(|ch| ch.is_ascii_digit())
}

/// 行尾长数字：`uid=1700000000123` → `uid=<num>`。
///
/// 键名保留是为了让报告还能看出「这是哪一项」，值一律抹掉。
fn mask_trailing_number(token: &str) -> Option<String> {
    let bytes = token.as_bytes();
    let mut start = bytes.len();
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    if bytes.len() - start < NUM_MIN_DIGITS {
        return None;
    }
    let prefix = &token[..start];
    if prefix.is_empty() {
        return Some(MASK_NUM.to_string());
    }
    let prefix_ok = prefix.len() <= 24
        && prefix.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '=' | '-' | ':' | '?')
        });
    if !prefix_ok {
        return None;
    }
    Some(format!("{prefix}{MASK_NUM}"))
}

/// 疑似令牌：全 ASCII、长度足够、字母与数字混合（纯字母的长单词是正常英文 / 驼峰标识）。
fn is_long_token(token: &str) -> bool {
    if token.len() < TOKEN_MIN_CHARS || !token.is_ascii() {
        return false;
    }
    let has_digit = token.chars().any(|ch| ch.is_ascii_digit());
    let has_alpha = token.chars().any(|ch| ch.is_ascii_alphabetic());
    has_digit && has_alpha
}

fn is_base64_url_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '=')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked(input: &str) -> String {
        redact_with_user(input, None).text
    }

    /// 路径：家目录、绝对路径、Windows 盘符、UNC 全部抹掉。
    #[test]
    fn masks_paths_in_every_platform_shape() {
        assert_eq!(
            masked("读取 /Users/alex/Library/x.json 失败"),
            "读取 <path> 失败"
        );
        assert_eq!(
            masked(r"写入 C:\Users\alex\AppData\Roaming\a.txt 失败"),
            "写入 <path> 失败"
        );
        assert_eq!(masked(r"\\fileserver\share\a.txt"), "<path>");
        assert_eq!(masked("/home/dev/project/src/main.rs"), "<path>");
    }

    /// URL 保留 scheme + host（判断是哪家网关用），路径 / query 抹成 `<url>`；userinfo 也要丢掉。
    #[test]
    fn masks_url_path_but_keeps_scheme_and_host() {
        assert_eq!(
            masked("POST https://api.codebuddy.cn/v2/plugin/login?x=1 返回 502"),
            "POST https://api.codebuddy.cn/<url> 返回 502"
        );
        assert_eq!(masked("https://a.example.com"), "https://a.example.com");
        assert_eq!(
            masked("https://user:pass@gateway.internal/proxy/path"),
            "https://gateway.internal/<url>"
        );
    }

    /// 规则顺序：UUID / 令牌先于「长数字」，否则会被拆碎成 `550e8400-…#<num>`。
    #[test]
    fn uuid_and_token_win_over_the_number_rule() {
        // 末尾 12 位是纯数字，若先走数字规则就会变成 `<uuid 前半>-<num>`。
        assert_eq!(
            masked("550e8400-e29b-41d4-a716-446655440000"),
            "<uuid>",
            "UUID 必须整体替换"
        );
        // 纯数字开头 + 字母的长令牌要整体成 <token>，不能被数字规则切一刀。
        assert_eq!(masked("20251120000000abcdef0123456789abcdef"), "<token>");
        // 短数字、版本号、状态码、查询参数一律原样保留（过度脱敏会让报告失去意义）。
        for keep in ["0.5.0", "HTTP 502", "code=429", "第 3 条", "3/4"] {
            assert_eq!(masked(keep), keep, "不该抹掉 {keep}");
        }
    }

    /// UUID / 邮箱 / 令牌 / 长数字各自成占位符。
    #[test]
    fn masks_uuid_email_token_and_numbers() {
        assert_eq!(
            masked("session 3f2a1b4c-5d6e-7f80-9a1b-2c3d4e5f6071 已失效"),
            "session <uuid> 已失效"
        );
        assert_eq!(
            masked("联系 alex.chen@example.com 处理"),
            "联系 <email> 处理"
        );
        assert_eq!(
            masked("Authorization: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl"),
            "Authorization: <token>"
        );
        assert_eq!(
            masked("令牌 abcdef0123456789abcdef0123456789ABCD 无效"),
            "令牌 <token> 无效"
        );
        assert_eq!(masked("1700000000123"), "<num>");
        assert_eq!(masked("uid=1700000000123 不存在"), "uid=<num> 不存在");
    }

    /// 不该被抹掉的东西：状态码、普通英文、中文正文、短数字、比值。
    #[test]
    fn keeps_ordinary_text_intact() {
        assert_eq!(masked("HTTP 502 Bad Gateway"), "HTTP 502 Bad Gateway");
        assert_eq!(
            masked("登录已失效，需要重新登录"),
            "登录已失效，需要重新登录"
        );
        assert_eq!(masked("3/4 的会话已复制"), "3/4 的会话已复制");
        assert_eq!(
            masked("返回格式异常（HTTP 401）"),
            "返回格式异常（HTTP 401）"
        );
        assert_eq!(masked(""), "");
    }

    /// 用户名整词替换（大小写不敏感），不做子串替换（否则 `alex` 会吃掉 `alexa`）。
    #[test]
    fn masks_home_user_name_as_whole_word() {
        let outcome = redact_with_user("user Alex is not logged in; alexa is fine", Some("alex"));
        assert_eq!(outcome.text, "user <user> is not logged in; alexa is fine");
        assert_eq!(outcome.replacements, 1);
    }

    /// 替换计数用于报告头，必须与真实替换数一致（重复片段逐个计数）。
    #[test]
    fn counts_every_replacement() {
        let outcome = redact_with_user("/a/b/c 与 /d/e/f 都失败", None);
        assert_eq!(outcome.text, "<path> 与 <path> 都失败");
        assert_eq!(outcome.replacements, 2);
        assert_eq!(redact_with_user("没有机密", None).replacements, 0);
    }

    /// `Bearer` 后面的短令牌靠上下文认（长度规则只有 ≥32 才命中，13 字符会漏）。
    #[test]
    fn masks_short_token_after_bearer() {
        assert_eq!(
            masked("Authorization: Bearer sk-secret-123"),
            "Authorization: Bearer <token>"
        );
        // 大小写不敏感；标签词自身保留（否则看不出是哪种认证头）。
        assert_eq!(
            masked("authorization: bearer abc"),
            "authorization: bearer <token>"
        );
        // 没有 Bearer 上下文的短词不动（不能把所有短词都抹掉）。
        assert_eq!(masked("字段 sk-secret-123 缺失"), "字段 sk-secret-123 缺失");
    }

    /// 邮箱紧跟英文句点：句点原样保留，邮箱本体抹掉。
    #[test]
    fn masks_email_followed_by_period() {
        assert_eq!(
            masked("保存账号失败: alice@example.com."),
            "保存账号失败: <email>."
        );
        assert_eq!(
            masked("contact: alex.chen@example.com.."),
            "contact: <email>.."
        );
    }

    /// 含空格的路径：后半截（`Support/WorkBuddy/`）也必须继续抹掉，不能留残片。
    #[test]
    fn masks_path_segments_after_spaces() {
        assert_eq!(
            masked("/Users/alice/Library/Application Support/WorkBuddy/"),
            "<path> <path>"
        );
        assert_eq!(
            masked("读取 /Users/alice/Library/Application Support/WorkBuddy/ 失败"),
            "读取 <path> <path> 失败"
        );
        // 路径之后的普通比值（`3/4`）不被误伤（token 长度 > 3 才算延续）。
        assert_eq!(masked("/a/b 3/4 完成"), "<path> 3/4 完成");
    }

    /// 幂等：对已脱敏文本再跑一遍不产生副作用（报告会经过多轮处理）。
    #[test]
    fn redaction_is_idempotent() {
        let once = masked("读取 /Users/alex/x 与 alex@example.com 失败");
        assert_eq!(masked(&once), once);
    }
}
