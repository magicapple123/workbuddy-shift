//! 工作区索引合并：WorkspaceState、索引条目合并与消息首行合并。

#[allow(unused_imports)]
use super::copy::*;
#[allow(unused_imports)]
use super::list::*;
#[allow(unused_imports)]
use super::paths::*;
#[allow(unused_imports)]
use super::remap::*;
use super::*;

/// 目标工作区在本次复制期间的状态：磁盘备份 + 内存工作副本。
pub(super) struct WorkspaceState {
    /// 目标工作区目录（`...\history\<workspaceHash>`）。
    pub(super) dir: PathBuf,
    /// 进入本次操作前的 `index.json` 原始字节（不存在为 `None`），用于回退。
    pub(super) original: Option<Vec<u8>>,
    /// 当前工作副本（已合并成功条目）。
    pub(super) current: Value,
    /// 索引备份落盘目录（`<backup_root>\<workspaceHash>`）。
    pub(super) backup_dir: PathBuf,
}

impl WorkspaceState {
    /// 加载目标工作区：读取既有索引、备份将被修改的索引、建立内存工作副本。
    ///
    /// **延迟创建**：此处**不**创建目标工作区目录，只有确有会话写入（[`Self::persist`] 或
    /// [`write_conversation`] 落临时目录）时才创建，避免「该工作区所有条目最终都失败」
    /// 时留下空目录。备份目录也仅在确有既有索引需要备份时才落盘。
    pub(super) fn load(
        dir: &Path,
        backup_root: &Path,
        workspace_hash: &str,
    ) -> Result<Self, String> {
        let index_path = dir.join("index.json");
        let original = std::fs::read(&index_path).ok();
        let original_bak = std::fs::read(dir.join(".index_bak.json")).ok();

        let workspace_backup = backup_root.join(workspace_hash);
        if original.is_some() || original_bak.is_some() {
            std::fs::create_dir_all(&workspace_backup).map_err(|error| error.to_string())?;
            if let Some(bytes) = &original {
                std::fs::write(workspace_backup.join("index.json"), bytes)
                    .map_err(|error| error.to_string())?;
            }
            if let Some(bytes) = &original_bak {
                let _ = std::fs::write(workspace_backup.join(".index_bak.json"), bytes);
            }
        }

        let current = match &original {
            Some(bytes) => serde_json::from_slice(bytes).unwrap_or_else(|_| json!({})),
            None => json!({}),
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            original,
            current,
            backup_dir: workspace_backup,
        })
    }

    /// 把当前工作副本写回磁盘（`index.json`；插件侧同时写 `.index_bak.json`，均用 `atomic_write`）。
    ///
    /// 目标工作区目录在此处按需创建（延迟创建），保证只在确有会话写入时才建目录。
    pub(super) fn persist(&self, write_backup: bool) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let text = self.current.to_string();
        atomic_write(&self.dir.join("index.json"), &text).map_err(|error| error.to_string())?;
        if write_backup {
            atomic_write(&self.dir.join(".index_bak.json"), &text)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// 把工作区索引回退到进入本次操作前的状态（尽力而为）。
    pub(super) fn restore(&self) {
        match &self.original {
            Some(bytes) => {
                let _ = std::fs::write(self.dir.join("index.json"), bytes);
                if let Ok(backup) = std::fs::read(self.backup_dir.join(".index_bak.json")) {
                    let _ = std::fs::write(self.dir.join(".index_bak.json"), backup);
                }
            }
            None => {
                let _ = std::fs::remove_file(self.dir.join("index.json"));
                let _ = std::fs::remove_file(self.dir.join(".index_bak.json"));
            }
        }
    }
}

/// 合并两张 id 映射表：`messages` 优先（同一个旧 id 同时出现在两张表时以消息表为准，
/// 与历史 `lookup` 的先后顺序一致）。仅用于「递归精确匹配」这一类不做区分的遍历。
pub(super) fn merge_message_first(
    message_ids: &BTreeMap<String, String>,
    request_ids: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut combined = request_ids.clone();
    for (old_id, new_id) in message_ids {
        combined.insert(old_id.clone(), new_id.clone());
    }
    combined
}

/// 为一个旧 id 生成并登记新 id（空串或已登记则跳过）。
pub(super) fn insert_new_id(
    map: &mut BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
    old_id: &str,
) {
    if old_id.is_empty() || map.contains_key(old_id) {
        return;
    }
    let new_id = unique_hex32(used);
    map.insert(old_id.to_string(), new_id);
}

/// 把新会话条目并入工作区索引（保留 `current` 等字段不变，追加到 `conversations[]`）。
pub(super) fn merge_workspace_index(index: &mut Value, entry: Value, new_id: &str) {
    if !index.is_object() {
        *index = json!({});
    }
    let Some(object) = index.as_object_mut() else {
        return;
    };
    let conversations = object
        .entry("conversations".to_string())
        .or_insert_with(|| json!([]));
    if !conversations.is_array() {
        *conversations = json!([]);
    }
    if let Some(array) = conversations.as_array_mut() {
        array.push(entry);
    }

    // 保证 `current` 存在且指向索引内真实存在的会话：实测（macOS，扩展 4.12）缺 `current`
    // 的工作区索引会被扩展判定为损坏——改名为 `index.json.corrupted.<ms>` 并重建，同时
    // 多出一条垃圾空会话（见 acceptance.md §3 / F9）。目标原本的 `current` 仍有效则保留，
    // 缺失或悬空时指向本次并入的会话。
    let current_ok = match object.get("current").and_then(Value::as_str) {
        Some(current) => object
            .get("conversations")
            .and_then(Value::as_array)
            .map(|array| {
                array
                    .iter()
                    .any(|item| item.get("id").and_then(Value::as_str) == Some(current))
            })
            .unwrap_or(false),
        None => false,
    };
    if !current_ok {
        object.insert("current".to_string(), json!(new_id));
    }
}

/// 从工作区索引中查找指定会话条目。
pub(super) fn find_conversation(index: &Value, conversation_id: &str) -> Option<Value> {
    index
        .get("conversations")
        .and_then(Value::as_array)?
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(conversation_id))
        .cloned()
}

/// 构造并入目标索引的会话条目：优先复用源条目（保留 name/type/时间等），仅换 id。
pub(super) fn conversation_entry(source_entry: Option<&Value>, new_id: &str) -> Value {
    match source_entry.and_then(Value::as_object) {
        Some(object) => {
            let mut entry = object.clone();
            entry.insert("id".to_string(), json!(new_id));
            Value::Object(entry)
        }
        None => json!({
            "id": new_id,
            "type": "craft",
            "name": "",
            "createdAt": Value::Null,
            "lastMessageAt": Value::Null,
        }),
    }
}
