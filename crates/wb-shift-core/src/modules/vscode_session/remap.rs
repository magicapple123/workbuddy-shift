//! ID 重映射：RemapPlan、会话/消息/extra 内 id 改写与索引重写。

#[allow(unused_imports)]
use super::copy::*;
#[allow(unused_imports)]
use super::index_merge::*;
#[allow(unused_imports)]
use super::list::*;
#[allow(unused_imports)]
use super::paths::*;
use super::*;

/// 会话内 id 重映射计划。
pub(crate) struct RemapPlan {
    /// 旧消息 id → 新消息 id。
    pub(super) message_ids: BTreeMap<String, String>,
    /// 旧请求 id → 新请求 id。
    pub(super) request_ids: BTreeMap<String, String>,
    /// 递归精确匹配用的合并表：`message_ids` 优先于 `request_ids`
    /// （与历史 `lookup` 的两表顺序一致）。
    pub(super) combined_ids: BTreeMap<String, String>,
    /// 参与复制的消息数量（用于报告）。
    pub(super) message_total: usize,
}

impl RemapPlan {
    /// 依据会话索引与磁盘 `messages/` 目录建立 id 重映射表。
    pub(super) fn build(source_index: &Value, source_dir: &Path) -> Self {
        let mut message_ids: BTreeMap<String, String> = BTreeMap::new();
        let mut request_ids: BTreeMap<String, String> = BTreeMap::new();
        let mut used: BTreeSet<String> = BTreeSet::new();

        // 消息 id：来自索引 messages[]（索引是权威引用，取值按契约即 hex，不做过滤）
        // 与磁盘 messages/*.json 文件名（只认 32 位小写 hex：目录里的其它 json 如
        // 调试文件既非消息 id，也不该被改名，原样复制）。
        if let Some(messages) = source_index.get("messages").and_then(Value::as_array) {
            for message in messages {
                if let Some(id) = message.get("id").and_then(Value::as_str) {
                    insert_new_id(&mut message_ids, &mut used, id.trim());
                }
            }
        }
        if let Ok(entries) = std::fs::read_dir(source_dir.join("messages")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(stem) = name.strip_suffix(".json") {
                    if is_hex32(stem) {
                        insert_new_id(&mut message_ids, &mut used, stem);
                    }
                }
            }
        }
        // 请求 id：来自索引 requests[]。
        if let Some(requests) = source_index.get("requests").and_then(Value::as_array) {
            for request in requests {
                if let Some(id) = request.get("id").and_then(Value::as_str) {
                    let id = id.trim();
                    if !id.is_empty() && !request_ids.contains_key(id) {
                        request_ids.insert(id.to_string(), unique_hex32(&mut used));
                    }
                }
            }
        }

        Self {
            message_total: message_ids.len(),
            combined_ids: merge_message_first(&message_ids, &request_ids),
            message_ids,
            request_ids,
        }
    }
}

/// 用给定的 id 映射表构造计划：复制路径用随机新 hex，覆盖同步用确定性派生 id。
pub(crate) fn remap_plan_from_maps(
    message_ids: BTreeMap<String, String>,
    request_ids: BTreeMap<String, String>,
) -> RemapPlan {
    RemapPlan {
        message_total: message_ids.len(),
        combined_ids: merge_message_first(&message_ids, &request_ids),
        message_ids,
        request_ids,
    }
}

/// 改写副本内「恰好等于旧会话 id」的引用：只重写确有引用的 JSON 文件，
/// 未命中的文件保持字节不变（实测会话索引与消息文件都不含自身会话 id 时即零改写）。
pub(super) fn rewrite_conversation_id_references(dest_dir: &Path, old_id: &str, new_id: &str) {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    map.insert(old_id.to_string(), new_id.to_string());
    for path in json_files(dest_dir) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        // 先判定「确有引用」再改写：改写会把文件重新序列化为紧凑格式，不该波及无关文件。
        if !contains_exact_id(&value, old_id) {
            continue;
        }
        let _ = std::fs::write(&path, remap_json_references(&value, &map).to_string());
    }
}

/// 递归判断某个 JSON 值里是否存在「恰好等于目标 id」的字符串（含字符串化 JSON 内部）。
pub(super) fn contains_exact_id(value: &Value, target: &str) -> bool {
    match value {
        Value::String(text) => {
            if text == target {
                return true;
            }
            // 字符串化 JSON（消息 `extra` 的实测形态）：解析后继续往下找。
            matches!(
                serde_json::from_str::<Value>(text),
                Ok(inner) if inner.is_object() && contains_exact_id(&inner, target)
            )
        }
        Value::Array(items) => items.iter().any(|item| contains_exact_id(item, target)),
        Value::Object(object) => object.values().any(|item| contains_exact_id(item, target)),
        _ => false,
    }
}

/// 递归收集目录下的 `.json` 文件（会话索引、消息文件与其备份）。
pub(super) fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(json_files(&path));
            } else if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            {
                out.push(path);
            }
        }
    }
    out
}

/// 与 [`remap_message_file`] 同范围的引用改写：对象各层递归精确匹配 + `extra`
/// （兼容字符串化 JSON），不改动正文与其它无关字符串。
pub(super) fn remap_json_references(value: &Value, map: &BTreeMap<String, String>) -> Value {
    let mut out = value.clone();
    match out.as_object_mut() {
        Some(object) => {
            for item in object.values_mut() {
                replace_exact_ids(item, map);
            }
            if let Some(extra) = object.get("extra").cloned() {
                object.insert("extra".to_string(), replace_ids_in_extra(&extra, map));
            }
        }
        None => replace_exact_ids(&mut out, map),
    }
    out
}

/// 在目标目录写全一个会话：重映射后的 `index.json` + `messages/*` + 其余文件/目录原样复制。
///
/// 覆盖同步（`vscode_session_sync`）复用同一个写入器，只是 id 来源换成确定性派生。
pub(crate) fn write_conversation(
    source_dir: &Path,
    dest_dir: &Path,
    source_index: &Value,
    plan: &RemapPlan,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dest_dir)?;

    // 1) 重映射后的会话索引。
    let remapped_index = remap_session_index(source_index, plan);
    std::fs::write(dest_dir.join("index.json"), remapped_index.to_string())?;

    // 2) messages/<oldId>.json → messages/<newId>.json，并重写 id / extra 内的 id 引用。
    let source_messages = source_dir.join("messages");
    if source_messages.is_dir() {
        let target_messages = dest_dir.join("messages");
        std::fs::create_dir_all(&target_messages)?;
        for entry in std::fs::read_dir(&source_messages)?.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let stem = name.strip_suffix(".json").unwrap_or(&name).to_string();
            // 只重写 32 位小写 hex 的消息文件；`messages/` 下的其它 json（调试文件、
            // 扩展自己的锁文件等）一律按字节原样复制——既不改名也不改内容，避免
            // `remap_message_file` 的重新序列化改变其格式。
            if !is_hex32(&stem) {
                std::fs::copy(&path, target_messages.join(&name))?;
                continue;
            }
            let new_name = match plan.message_ids.get(&stem) {
                Some(new_id) => format!("{new_id}.json"),
                None => name.clone(),
            };
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let remapped = remap_message_file(&text, &stem, plan);
                    std::fs::write(target_messages.join(&new_name), remapped)?;
                }
                // 非 UTF-8 或读取失败：按二进制原样复制。
                Err(_) => {
                    std::fs::copy(&path, target_messages.join(&new_name))?;
                }
            }
        }
    }

    // 2b) 会话级 `.index_bak.json`（若存在，结构与 index.json 同）：同样重映射，
    //     避免旧 messageId / requestId 随原样复制残留在目标目录。
    let source_bak = source_dir.join(".index_bak.json");
    if source_bak.is_file() {
        match read_json(&source_bak) {
            Some(bak) => std::fs::write(
                dest_dir.join(".index_bak.json"),
                remap_session_index(&bak, plan).to_string(),
            )?,
            None => {
                std::fs::copy(&source_bak, dest_dir.join(".index_bak.json"))?;
            }
        }
    }

    // 3) 其余顶层文件 / 目录原样复制（跳过已单独处理的索引与 messages；附件保持原文件名）。
    for entry in std::fs::read_dir(source_dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "index.json" || name == ".index_bak.json" || name == "messages" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            copy_dir_recursive(&path, &dest_dir.join(&name))?;
        } else if path.is_file() {
            std::fs::copy(&path, dest_dir.join(&name))?;
        }
    }
    Ok(())
}

/// 重映射会话索引：`messages[].id`、`requests[].id`、`requests[].messages[]`。
pub(super) fn remap_session_index(source_index: &Value, plan: &RemapPlan) -> Value {
    let mut out = source_index.clone();
    if let Some(messages) = out.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages.iter_mut() {
            if let Some(id) = message
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                if let Some(new_id) = plan.message_ids.get(&id) {
                    if let Some(object) = message.as_object_mut() {
                        object.insert("id".to_string(), json!(new_id));
                    }
                }
            }
        }
    }
    if let Some(requests) = out.get_mut("requests").and_then(Value::as_array_mut) {
        for request in requests.iter_mut() {
            let Some(object) = request.as_object_mut() else {
                continue;
            };
            if let Some(id) = object.get("id").and_then(Value::as_str).map(str::to_string) {
                if let Some(new_id) = plan.request_ids.get(&id) {
                    object.insert("id".to_string(), json!(new_id));
                }
            }
            if let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) {
                for message in messages.iter_mut() {
                    if let Some(id) = message.as_str().map(str::to_string) {
                        if let Some(new_id) = plan.message_ids.get(&id) {
                            *message = json!(new_id);
                        }
                    }
                }
            }
        }
    }
    out
}

/// 重映射单条消息文件：`id` 与 `extra` 内的 id 引用（保留其余字段原样）。
pub(super) fn remap_message_file(text: &str, old_stem: &str, plan: &RemapPlan) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return text.to_string();
    };
    let Some(object) = value.as_object_mut() else {
        return text.to_string();
    };
    // 文件已按映射表重命名，内部 id 同步为新 id。
    if let Some(new_id) = plan.message_ids.get(old_stem) {
        object.insert("id".to_string(), json!(new_id));
    }
    if let Some(extra) = object.get("extra").cloned() {
        object.insert("extra".to_string(), remap_extra(&extra, plan));
    }
    value.to_string()
}

/// 重映射消息 `extra` 内的 id 引用（兼容字符串化 JSON 与对象两种形态）。
pub(super) fn remap_extra(extra: &Value, plan: &RemapPlan) -> Value {
    replace_ids_in_extra(extra, &plan.combined_ids)
}

/// id 容器（消息 `extra`）的通用替换：兼容字符串化 JSON 与对象两种形态。
///
/// 顶层与嵌套一视同仁——对容器内所有层级做「精确等于某旧 id」的替换，
/// 因此 `requestId` / `responseId` 与 `tasks` / `sourceContentBlocks` /
/// `selectionContexts` 等嵌套引用走同一条规则，不会误伤正文、`modelId` 等无关字符串。
///
/// 复制路径传「旧 id → 新 hex」，摘要路径传「旧 id → 占位符」（design §3）。
pub(crate) fn replace_ids_in_extra(extra: &Value, map: &BTreeMap<String, String>) -> Value {
    let (mut object, stringified) = match extra {
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => (map, true),
            _ => return extra.clone(),
        },
        Value::Object(map) => (map.clone(), false),
        _ => return extra.clone(),
    };
    for value in object.values_mut() {
        replace_exact_ids(value, map);
    }
    let value = Value::Object(object);
    if stringified {
        json!(value.to_string())
    } else {
        value
    }
}

/// 递归遍历任意 JSON 值，把「恰好等于映射表中某个旧 id」的字符串替换为映射值。
pub(crate) fn replace_exact_ids(value: &mut Value, map: &BTreeMap<String, String>) {
    match value {
        Value::String(text) => {
            if let Some(replacement) = map.get(text) {
                *value = json!(replacement);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                replace_exact_ids(item, map);
            }
        }
        Value::Object(object) => {
            for item in object.values_mut() {
                replace_exact_ids(item, map);
            }
        }
        _ => {}
    }
}
