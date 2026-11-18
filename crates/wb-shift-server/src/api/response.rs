//! JSON 响应与 query/body 参数解析的公共 helper。

use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use wb_shift_core::modules::variant::WbVariant;

pub(crate) fn json_ok(v: Value) -> Response {
    Json(v).into_response()
}

pub(crate) fn json_err(e: String, code: StatusCode) -> Response {
    (code, Json(json!({ "ok": false, "error": e }))).into_response()
}

/// 解析 `Json<Dto>` 请求体；失败时把 axum 的 rejection 交给 [`invalid_body`]。
///
/// 分成两步是为了让 `Result` 的错误分支保持轻量（`JsonRejection`），
/// 而不是在每个 handler 里内联一个 128 字节的 `Response`。
pub(crate) fn parse_body<T>(payload: Result<Json<T>, JsonRejection>) -> Result<T, JsonRejection> {
    payload.map(|Json(value)| value)
}

/// 请求体反序列化失败：统一成既有的 `{ ok: false, error }` 信封（400）。
///
/// axum 默认把反序列化失败变成 422 + 纯文本，前端 `call()` 会退化成
/// “请求失败 (422)”；这里统一成 400 + 可读文案，与其它业务错误同一形状。
pub(crate) fn invalid_body(rejection: JsonRejection) -> Response {
    json_err(
        format!("请求参数无效：{}", rejection.body_text()),
        StatusCode::BAD_REQUEST,
    )
}

/// 把 call()/invoke 的 { config: <payload> } 信封拆成 core 需要的配置本身。
///
/// 桌面端 Tauri 按参数名注入 config，WebUI 的 call() 把整个 args 作为 JSON body，
/// 因此两个宿主必须在边界内统一拆信封；core 的 save_* 只接受配置本身。
pub(crate) fn submitted_config(body: &Value) -> &Value {
    body.get("config").unwrap_or(body)
}

/// 从 query string 解析档位（缺省国内版）。与 Tauri 命令的可选 variant 参数同义。
pub(crate) fn query_variant(query: Option<&str>) -> WbVariant {
    WbVariant::parse(query_param(query, "variant").as_deref())
}

/// 从 query string 取指定键的原值（百分号解码、+ 视为空格，与 URLSearchParams 一致）。
///
/// 代理地址这类值可能含 ://，必须解码后再交给 core，否则会把编码串当代理地址。
pub(crate) fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    query.unwrap_or("").split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        (name == key).then(|| percent_decode(value))
    })
}

/// 布尔型 query 参数：force / force=true / force=1 / force=yes 均视为 true。
pub(crate) fn query_flag_enabled(query: Option<&str>, name: &str) -> bool {
    query_param(query, name)
        .map(|value| matches!(value.as_str(), "" | "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                match (hex_value(bytes[index + 1]), hex_value(bytes[index + 2])) {
                    (Some(high), Some(low)) => {
                        out.push(high * 16 + low);
                        index += 3;
                    }
                    _ => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
