//! 积分本地账本（第四轮 07）：把每次官方窗口抓到的逐请求明细落 SQLite。
//!
//! 官方用量接口只给「近 30 天」（服务端可能只接受 31 天窗口），而用户要的是「总计」。
//! 官方没有历史归档，唯一诚实的做法是**从本工具开始运行那天起**自己累计：
//!
//! - 表 `requests` 以 `request_id` 为主键，重复抓取用 `INSERT OR IGNORE` 去重；
//! - 首次启用时当前 30 天窗口的明细会一次性写入（回填），所以「全部」一上线就有
//!   约 30 天数据，UI 如实标注起点（不假装是账号创建日）；
//! - 账本是本地累计，可能与官方账单有差异（官方修正历史、退款/补偿都不回写）。
//!
//! 写失败**不能影响页面**：调用方把 `ledgerStatus` 降级为 `degraded`，页面继续用
//! 官方口径，只是「全部」不可用。

use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::modules::config::store_dir;

/// 账本数据库文件名（与 accounts / backups 同级）。
const LEDGER_FILE: &str = "credit_ledger.db";

/// 账本路径：`~/.workbuddy-shift/credit_ledger.db`（认 `WB_SHIFT_HOME`）。
pub fn ledger_path() -> PathBuf {
    store_dir().join(LEDGER_FILE)
}

fn open(path: &Path) -> Result<Connection, String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| format!("创建账本目录失败：{error}"))?;
    let conn = Connection::open(path).map_err(|error| format!("打开账本失败：{error}"))?;
    // WAL：统计页读账本时不会阻塞下一次抓取的批量写。
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    ensure_schema(&conn)?;
    Ok(conn)
}

fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS requests (
            request_id   TEXT PRIMARY KEY,
            account_id   TEXT NOT NULL,
            account_name TEXT,
            request_ts   INTEGER NOT NULL,
            date         TEXT NOT NULL,
            model        TEXT,
            client       TEXT,
            credit       REAL NOT NULL,
            first_seen_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_requests_date ON requests(date);
        CREATE INDEX IF NOT EXISTS idx_requests_client_date ON requests(client, date);
        CREATE INDEX IF NOT EXISTS idx_requests_account_date ON requests(account_id, date);",
    )
    .map_err(|error| format!("建账本表失败：{error}"))
}

fn text_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 把 `requestTime`（`YYYY-MM-DD HH:mm:ss`）解析成 `(date, ts)`；解析不出返回 None。
///
/// 官方明细里该字段是本地时间文本，没有时区；按本地时区解释与页面展示一致。
fn parse_request_time(text: &str) -> Option<(String, i64)> {
    let text = text.trim();
    if let Ok(value) = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S") {
        let date = value.date().format("%Y-%m-%d").to_string();
        let ts = Local
            .from_local_datetime(&value)
            .earliest()
            .map(|value| value.timestamp_millis())
            .unwrap_or(0);
        return Some((date, ts));
    }
    let date = NaiveDate::parse_from_str(text.get(..10).unwrap_or(text), "%Y-%m-%d").ok()?;
    let midnight = date.and_hms_opt(0, 0, 0)?;
    let ts = Local
        .from_local_datetime(&midnight)
        .earliest()
        .map(|value| value.timestamp_millis())
        .unwrap_or(0);
    Some((date.format("%Y-%m-%d").to_string(), ts))
}

/// 批量写入官方明细（按 `request_id` 去重），返回真正新增的条数。
///
/// 单条明细缺 `requestId` / 时间不可解析时跳过：账本的分组维度是日期，
/// 没有日期就无法落库，宁可少记也不写一条归不了类的记录。
pub fn record_requests(requests: &[Value], at_ms: i64) -> Result<usize, String> {
    record_requests_at(&ledger_path(), requests, at_ms)
}

/// [`record_requests`] 的可注入路径版本（单测用临时目录）。
pub fn record_requests_at(path: &Path, requests: &[Value], at_ms: i64) -> Result<usize, String> {
    if requests.is_empty() {
        return Ok(0);
    }
    let mut conn = open(path)?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("账本事务开启失败：{error}"))?;
    let mut inserted = 0usize;
    {
        let mut statement = tx
            .prepare(
                "INSERT OR IGNORE INTO requests
                 (request_id, account_id, account_name, request_ts, date, model, client, credit, first_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .map_err(|error| format!("账本写入语句准备失败：{error}"))?;
        // 官方会修正「当天仍在结算」的请求：同一 request_id 再次抓到时分值可能变大。
        // 只用 INSERT OR IGNORE 会让账本永远停留在首次见到的旧值（实测今日差 59.13 分）。
        // 这里对已存在的行做 UPDATE（保留 first_seen_at），行数不变但金额跟上官方。
        let mut update_statement = tx
            .prepare(
                "UPDATE requests SET
                   account_id = ?1, account_name = ?2, request_ts = ?3, date = ?4,
                   model = ?5, client = ?6, credit = ?7
                 WHERE request_id = ?8",
            )
            .map_err(|error| format!("账本更新语句准备失败：{error}"))?;
        for request in requests {
            let request_id = text_field(request, "requestId");
            if request_id.is_empty() {
                continue;
            }
            let Some((date, request_ts)) = parse_request_time(&text_field(request, "requestTime"))
            else {
                continue;
            };
            let credit = request
                .get("credit")
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite())
                .unwrap_or(0.0);
            let account_id = text_field(request, "accountId");
            if account_id.is_empty() {
                continue;
            }
            let account_name = text_field(request, "accountName");
            let model = text_field(request, "model");
            let client = text_field(request, "client");
            let changed = statement
                .execute(params![
                    request_id,
                    account_id,
                    account_name,
                    request_ts,
                    date,
                    model,
                    client,
                    credit,
                    at_ms,
                ])
                .map_err(|error| format!("账本写入失败：{error}"))?;
            if changed == 0 {
                update_statement
                    .execute(params![
                        account_id,
                        account_name,
                        request_ts,
                        date,
                        model,
                        client,
                        credit,
                        request_id,
                    ])
                    .map_err(|error| format!("账本更新失败：{error}"))?;
                continue;
            }
            inserted += 1;
        }
    }
    tx.commit()
        .map_err(|error| format!("账本提交失败：{error}"))?;
    Ok(inserted)
}

/// 账本快照：总计 + 日/模型/客户端/账号四个分组（「全部」口径的唯一数据源）。
pub fn snapshot(now: i64) -> Value {
    snapshot_at(&ledger_path(), now)
}

/// [`snapshot`] 的可注入路径版本。任何失败都降级为 `status="degraded"`，不 panic。
pub fn snapshot_at(path: &Path, now: i64) -> Value {
    match read_snapshot(path, now) {
        Ok(value) => value,
        Err(error) => json!({
            "status": "degraded",
            "error": error,
            "rows": 0,
            "startAt": Value::Null,
            "endAt": Value::Null,
            "summary": { "usageToday": 0.0, "usage7Days": 0.0, "usageThisMonth": 0.0 },
            "daily": [],
            "models": [],
            "clients": [],
            "byClient": [],
            "accounts": [],
        }),
    }
}

fn read_snapshot(path: &Path, now: i64) -> Result<Value, String> {
    if !path.is_file() {
        // 还没跑过任何一次官方抓取：账本为空，但不是错误。
        return Ok(json!({
            "status": "empty",
            "rows": 0,
            "startAt": Value::Null,
            "endAt": Value::Null,
            "summary": { "usageToday": 0.0, "usage7Days": 0.0, "usageThisMonth": 0.0 },
            "daily": [],
            "models": [],
            "clients": [],
            "accounts": [],
        }));
    }
    let conn = Connection::open(path).map_err(|error| format!("打开账本失败：{error}"))?;
    ensure_schema(&conn)?;
    let (rows, total, start_at, end_at): (i64, f64, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(credit), 0), MIN(date), MAX(date) FROM requests",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|error| format!("账本汇总失败：{error}"))?;

    let today = Local
        .timestamp_millis_opt(now)
        .single()
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    let month = today.get(..7).unwrap_or_default().to_string();
    let week_start: String = Local
        .timestamp_millis_opt(now - 6 * 86_400_000)
        .single()
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    let sum_between = |minimum: &str, maximum: &str| -> Result<f64, String> {
        conn.query_row(
            "SELECT COALESCE(SUM(credit), 0) FROM requests WHERE date >= ?1 AND date <= ?2",
            params![minimum, maximum],
            |row| row.get::<_, f64>(0),
        )
        .map_err(|error| format!("账本窗口汇总失败：{error}"))
    };
    let usage_today = sum_between(&today, &today)?;
    let usage_week = sum_between(&week_start, &today)?;
    let usage_month = conn
        .query_row(
            "SELECT COALESCE(SUM(credit), 0) FROM requests WHERE date LIKE ?1",
            params![format!("{month}-%")],
            |row| row.get::<_, f64>(0),
        )
        .map_err(|error| format!("账本月汇总失败：{error}"))?;

    let mut daily_stmt = conn
        .prepare("SELECT date, SUM(credit) FROM requests GROUP BY date ORDER BY date")
        .map_err(|error| format!("账本日序列查询失败：{error}"))?;
    let daily = daily_stmt
        .query_map([], |row| {
            Ok(json!({ "date": row.get::<_, String>(0)?, "usage": row.get::<_, f64>(1)? }))
        })
        .map_err(|error| format!("账本日序列读取失败：{error}"))?
        .flatten()
        .collect::<Vec<Value>>();

    let mut model_stmt = conn
        .prepare(
            "SELECT COALESCE(NULLIF(model, ''), '未知模型'), COUNT(*), SUM(credit)
             FROM requests GROUP BY 1 ORDER BY 3 DESC, 2 DESC, 1",
        )
        .map_err(|error| format!("账本模型查询失败：{error}"))?;
    let models = model_stmt
        .query_map([], |row| {
            Ok(json!({
                "model": row.get::<_, String>(0)?,
                "requestCount": row.get::<_, i64>(1)?,
                "credit": row.get::<_, f64>(2)?,
            }))
        })
        .map_err(|error| format!("账本模型读取失败：{error}"))?
        .flatten()
        .collect::<Vec<Value>>();

    let mut client_stmt = conn
        .prepare(
            "SELECT COALESCE(NULLIF(client, ''), '—'), COUNT(*), SUM(credit)
             FROM requests GROUP BY 1 ORDER BY 3 DESC, 2 DESC, 1",
        )
        .map_err(|error| format!("账本客户端查询失败：{error}"))?;
    let clients = client_stmt
        .query_map([], |row| {
            Ok(json!({
                "client": row.get::<_, String>(0)?,
                "requestCount": row.get::<_, i64>(1)?,
                "credit": row.get::<_, f64>(2)?,
            }))
        })
        .map_err(|error| format!("账本客户端读取失败：{error}"))?
        .flatten()
        .collect::<Vec<Value>>();

    let mut account_stmt = conn
        .prepare(
            "SELECT account_id, COALESCE(MAX(account_name), ''), COUNT(*), SUM(credit)
             FROM requests GROUP BY account_id ORDER BY 4 DESC, 3 DESC, 1",
        )
        .map_err(|error| format!("账本账号查询失败：{error}"))?;
    let mut accounts = account_stmt
        .query_map([], |row| {
            Ok(json!({
                "accountId": row.get::<_, String>(0)?,
                "accountName": row.get::<_, String>(1)?,
                "requestCount": row.get::<_, i64>(2)?,
                "credit": row.get::<_, f64>(3)?,
            }))
        })
        .map_err(|error| format!("账本账号读取失败：{error}"))?
        .flatten()
        .collect::<Vec<Value>>();
    // 每个账号补三个窗口值：「账号消耗构成」按账号分窗口展示，缺了会把有数据的账号
    // 显示成「官方用量暂不可用」。
    for account in &mut accounts {
        let Some(account_id) = account.get("accountId").and_then(Value::as_str) else {
            continue;
        };
        let account_id = account_id.to_string();
        let window_sum = |sql: &str, parameters: &[&dyn rusqlite::ToSql]| -> f64 {
            conn.query_row(sql, parameters, |row| row.get::<_, f64>(0))
                .unwrap_or(0.0)
        };
        let today_sum = window_sum(
            "SELECT COALESCE(SUM(credit), 0) FROM requests WHERE account_id = ?1 AND date = ?2",
            &[&account_id, &today],
        );
        let week_sum = window_sum(
            "SELECT COALESCE(SUM(credit), 0) FROM requests WHERE account_id = ?1 AND date >= ?2 AND date <= ?3",
            &[&account_id, &week_start, &today],
        );
        let month_sum = window_sum(
            "SELECT COALESCE(SUM(credit), 0) FROM requests WHERE account_id = ?1 AND date LIKE ?2",
            &[&account_id, &format!("{month}-%")],
        );
        account["usageToday"] = json!(today_sum);
        account["usage7Days"] = json!(week_sum);
        account["usageThisMonth"] = json!(month_sum);
    }

    // 按客户端分组：「全部」范围下选具体客户端时，四卡/趋势/模型都必须留在账本口径上。
    let mut client_groups: Vec<Value> = Vec::new();
    for item in &clients {
        let client = item.get("client").and_then(Value::as_str).unwrap_or("—");
        let group_today = conn
            .query_row(
                "SELECT COALESCE(SUM(credit), 0) FROM requests WHERE client = ?1 AND date = ?2",
                params![client, today],
                |row| row.get::<_, f64>(0),
            )
            .unwrap_or(0.0);
        let group_week = conn
            .query_row(
                "SELECT COALESCE(SUM(credit), 0) FROM requests WHERE client = ?1 AND date >= ?2 AND date <= ?3",
                params![client, week_start, today],
                |row| row.get::<_, f64>(0),
            )
            .unwrap_or(0.0);
        let group_month = conn
            .query_row(
                "SELECT COALESCE(SUM(credit), 0) FROM requests WHERE client = ?1 AND date LIKE ?2",
                params![client, format!("{month}-%")],
                |row| row.get::<_, f64>(0),
            )
            .unwrap_or(0.0);
        let mut daily_stmt = conn
            .prepare("SELECT date, SUM(credit) FROM requests WHERE client = ?1 GROUP BY date ORDER BY date")
            .map_err(|error| format!("账本客户端日序列查询失败：{error}"))?;
        let group_daily = daily_stmt
            .query_map(params![client], |row| {
                Ok(json!({ "date": row.get::<_, String>(0)?, "usage": row.get::<_, f64>(1)? }))
            })
            .map_err(|error| format!("账本客户端日序列读取失败：{error}"))?
            .flatten()
            .collect::<Vec<Value>>();
        let mut model_stmt = conn
            .prepare(
                "SELECT COALESCE(NULLIF(model, ''), '未知模型'), COUNT(*), SUM(credit)
                 FROM requests WHERE client = ?1 GROUP BY 1 ORDER BY 3 DESC, 2 DESC, 1",
            )
            .map_err(|error| format!("账本客户端模型查询失败：{error}"))?;
        let group_models = model_stmt
            .query_map(params![client], |row| {
                Ok(json!({
                    "model": row.get::<_, String>(0)?,
                    "requestCount": row.get::<_, i64>(1)?,
                    "credit": row.get::<_, f64>(2)?,
                }))
            })
            .map_err(|error| format!("账本客户端模型读取失败：{error}"))?
            .flatten()
            .collect::<Vec<Value>>();
        client_groups.push(json!({
            "client": client,
            "rows": item.get("requestCount").cloned().unwrap_or_else(|| json!(0)),
            "total": item.get("credit").cloned().unwrap_or_else(|| json!(0.0)),
            "summary": {
                "usageToday": group_today,
                "usage7Days": group_week,
                "usageThisMonth": group_month,
            },
            "daily": group_daily,
            "models": group_models,
        }));
    }

    Ok(json!({
        "status": if rows > 0 { "ok" } else { "empty" },
        "rows": rows,
        "total": total,
        "startAt": start_at,
        "endAt": end_at,
        "summary": {
            "usageToday": usage_today,
            "usage7Days": usage_week,
            "usageThisMonth": usage_month,
        },
        "daily": daily,
        "models": models,
        "clients": clients,
        "byClient": client_groups,
        "accounts": accounts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::config::now_ms;

    fn temp_path(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("wb-shift-ledger-{label}-{unique}.db"))
    }

    fn request(id: &str, client: &str, credit: f64, time: &str, account: &str) -> Value {
        json!({
            "accountId": account,
            "accountName": format!("{account} 昵称"),
            "requestId": id,
            "credit": credit,
            "model": "deepseek-v4-flash",
            "client": client,
            "requestTime": time,
        })
    }

    #[test]
    fn record_is_idempotent_and_snapshot_groups() {
        let path = temp_path("record");
        let now = now_ms();
        let today = Local
            .timestamp_millis_opt(now)
            .single()
            .map(|value| value.format("%Y-%m-%d").to_string())
            .unwrap();
        let rows = vec![
            request(
                "r1",
                "WorkBuddy",
                1.5,
                &format!("{today} 10:00:00"),
                "acc-1",
            ),
            request("r2", "VSCode", 2.5, &format!("{today} 11:00:00"), "acc-1"),
            request("r3", "—", 4.0, &format!("{today} 12:00:00"), "acc-2"),
        ];
        assert_eq!(record_requests_at(&path, &rows, now).unwrap(), 3);
        // 重复抓取同一批：去重生效，不新增。
        assert_eq!(record_requests_at(&path, &rows, now).unwrap(), 0);

        let snapshot = snapshot_at(&path, now);
        assert_eq!(snapshot["status"], "ok");
        assert_eq!(snapshot["rows"], 3);
        assert_eq!(snapshot["startAt"], json!(today));
        assert_eq!(snapshot["summary"]["usageToday"], json!(8.0));
        assert_eq!(snapshot["models"][0]["requestCount"], 3);
        let clients = snapshot["clients"].as_array().unwrap();
        assert_eq!(clients.len(), 3);
        let accounts = snapshot["accounts"].as_array().unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(snapshot["daily"][0]["usage"], json!(8.0));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_database_is_empty_not_error() {
        let path = temp_path("missing");
        let snapshot = snapshot_at(&path, now_ms());
        assert_eq!(snapshot["status"], "empty");
        assert_eq!(snapshot["rows"], 0);
    }

    #[test]
    fn revised_credit_updates_the_row_without_adding_rows() {
        let path = temp_path("revised");
        let now = now_ms();
        let today = Local
            .timestamp_millis_opt(now)
            .single()
            .map(|value| value.format("%Y-%m-%d").to_string())
            .unwrap();
        assert_eq!(
            record_requests_at(
                &path,
                &[request(
                    "r1",
                    "WorkBuddy",
                    1.0,
                    &format!("{today} 10:00:00"),
                    "acc-1"
                )],
                now
            )
            .unwrap(),
            1
        );
        // 同一条请求被官方修正为 5.0：不能新增行，但金额必须跟上。
        assert_eq!(
            record_requests_at(
                &path,
                &[request(
                    "r1",
                    "WorkBuddy",
                    5.0,
                    &format!("{today} 10:00:00"),
                    "acc-1"
                )],
                now
            )
            .unwrap(),
            0
        );
        let snapshot = snapshot_at(&path, now);
        assert_eq!(snapshot["rows"], 1);
        assert_eq!(snapshot["summary"]["usageToday"], json!(5.0));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn invalid_rows_are_skipped() {
        let path = temp_path("invalid");
        let rows = vec![
            json!({ "requestId": "", "accountId": "a", "credit": 1.0, "requestTime": "2026-11-19 10:00:00" }),
            json!({ "requestId": "x", "accountId": "a", "credit": 1.0, "requestTime": "not a time" }),
            json!({ "requestId": "y", "accountId": "", "credit": 1.0, "requestTime": "2026-11-19 10:00:00" }),
        ];
        assert_eq!(record_requests_at(&path, &rows, now_ms()).unwrap(), 0);
        std::fs::remove_file(&path).ok();
    }
}
