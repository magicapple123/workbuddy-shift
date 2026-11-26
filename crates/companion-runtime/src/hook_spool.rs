//! WorkBuddy / CodeBuddy IDE hook 的「即发即走」落盘队列。
//!
//! 宿主（WorkBuddy / CodeBuddy IDE）给 hook 只有 3000ms，且这段预算包含
//! `bash.exe` 启动、原生 exe 冷启动、HTTP IPC 排队等全部开销。任何同步 RPC
//! 都会在忙时撞上这个上限，于是宿主把 hook 报成超时并拦截操作。
//!
//! 这两类宿主的 hook 只做「观察」，不需要宿主等待采集结果：CLI 先把 `{}`
//! 写到 stdout 并 flush，再把带来源标注的 payload 以单行 JSON 追加到用户目录下的
//! spool 文件，随即退出。常驻 daemon 的 worker 线程按字节偏移增量消费这个文件，
//! 因此即使 daemon 尚未启动 / 正在忙 / 暂时不存在，事件也会留在磁盘上、稍后补采。
//!
//! 与 `wb-shift-core` 的积分签到系统完全隔离：签到写
//! `~/.workbuddy-shift/hook-events.jsonl`，本模块写
//! `~/.workbuddy-shift-companion/companion-hook-spool.jsonl`，两者互不读写。

use fs2::FileExt;
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const SPOOL_FILE_NAME: &str = "companion-hook-spool.jsonl";
const LOCK_FILE_NAME: &str = "companion-hook-spool.lock";
const CHECKPOINT_FILE_NAME: &str = "companion-hook-spool.offset";
const ROTATED_FILE_NAME: &str = "companion-hook-spool.done";
/// 超过该大小且 daemon 重启时把已消费的旧文件改名归档，避免无限增长。
const MAX_SPOOL_BYTES: u64 = 16 * 1024 * 1024;
/// checkpoint 同时记录消费位置前若干字节的指纹，用于识别「截断后又长回同样长度」。
const TAIL_BYTES: usize = 32;
/// 这些来源的 hook 走落盘队列；codex 仍走需要确认的同步 RPC。
pub const HOST_SOURCES: [&str; 2] = ["workbuddy", "codebuddy-ide"];

pub fn is_host_source(source: &str) -> bool {
    HOST_SOURCES.contains(&source)
}

fn state_dir(home: &Path) -> PathBuf {
    home.join(".workbuddy-shift-companion")
}

pub fn spool_path(home: &Path) -> PathBuf {
    state_dir(home).join(SPOOL_FILE_NAME)
}

fn lock_path(home: &Path) -> PathBuf {
    state_dir(home).join(LOCK_FILE_NAME)
}

fn checkpoint_path(home: &Path) -> PathBuf {
    state_dir(home).join(CHECKPOINT_FILE_NAME)
}

/// stdout 输出 `{}` 并把 payload 暂存到 spool；仅处理 WorkBuddy / CodeBuddy IDE。
///
/// 返回 `Ok(true)` 表示事件已由本函数接管，调用方必须立即退出，不能再发 RPC。
/// 返回 `Ok(false)` 表示是 codex 等需要同步确认的来源，调用方沿用原路径。
///
/// 落盘失败只写 stderr、不返回错误：hook 的职责是让宿主继续执行，绝不能因为
/// 本地磁盘问题反过来拦截用户操作（issue #20 的「软件关掉了还在拦」）。
pub fn handle_host_hook<W: Write>(
    home: &Path,
    bytes: &[u8],
    source: &str,
    edition: Option<&str>,
    out: &mut W,
) -> Result<bool, String> {
    if !is_host_source(source) {
        return Ok(false);
    }
    // 宿主在等 stdout 解析 JSON，必须先输出再碰磁盘。
    let _ = writeln!(out, "{{}}");
    let _ = out.flush();
    let Ok(raw) = serde_json::from_slice::<Value>(bytes) else {
        return Ok(true);
    };
    let mut payload = raw;
    payload["agent_source"] = json!(source);
    if let Some(edition) = edition.filter(|s| !s.is_empty()) {
        payload["agent_edition"] = json!(edition);
    }
    if let Err(e) = append(home, &payload) {
        eprintln!("hook 事件暂存失败（不拦截宿主）：{e}");
    }
    Ok(true)
}

/// 以独占文件锁串行化所有 hook 进程的追加，保证一条事件恰好占一行。
pub fn append(home: &Path, payload: &Value) -> Result<(), String> {
    let dir = state_dir(home);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(home))
        .map_err(|e| e.to_string())?;
    lock.lock_exclusive().map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(spool_path(home))
            .map_err(|e| e.to_string())?;
        let mut line = serde_json::to_string(payload)
            .map_err(|e| e.to_string())?
            .into_bytes();
        // 上一次写入若在中途被强杀，可能留下半行；补一个换行让本次事件仍可解析。
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        if len > 0 {
            let mut last = [0u8; 1];
            file.seek(SeekFrom::End(-1)).map_err(|e| e.to_string())?;
            file.read_exact(&mut last).map_err(|e| e.to_string())?;
            if last[0] != b'\n' {
                line.insert(0, b'\n');
            }
        }
        // append 模式下 write_all 总是写到文件末尾， seek 只服务于上面的读。
        line.push(b'\n');
        file.write_all(&line).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
        Ok(())
    })();
    let _ = lock.unlock();
    result
}

/// 读取指定偏移处的若干字节；用于校验 checkpoint 是否仍然对应当前文件。
fn bytes_at(path: &Path, start: u64, len: usize) -> Option<Vec<u8>> {
    if len == 0 {
        return Some(Vec::new());
    }
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// daemon 侧的增量读取器。偏移量与内容指纹双重持久化：重启后不会重复消费也不丢事件。
pub struct HookSpool {
    path: PathBuf,
    checkpoint: PathBuf,
    offset: u64,
    tail: Vec<u8>,
}

impl HookSpool {
    pub fn new(home: &Path) -> Self {
        let path = spool_path(home);
        let checkpoint = checkpoint_path(home);
        let saved: Value = std::fs::read_to_string(&checkpoint)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .unwrap_or(Value::Null);
        let mut offset = saved["offset"].as_u64().unwrap_or(0);
        let mut tail = saved["tail"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_u64().map(|n| n as u8))
                    .collect::<Vec<u8>>()
            })
            .unwrap_or_default();
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if offset > len || (offset > 0 && (tail.is_empty() || tail.len() as u64 > offset)) {
            // 旧格式 checkpoint / 损坏的位置 / 文件被缩短：从头开始，绝不跳过新事件。
            offset = 0;
            tail.clear();
        } else if offset > 0
            && bytes_at(&path, offset - tail.len() as u64, tail.len()).as_deref()
                != Some(tail.as_slice())
        {
            // 文件被截断后又长回同样长度：仅靠 offset 判断不出来，靠指纹识别。
            offset = 0;
            tail.clear();
        }
        // 只归档「已消费完」的旧文件：offset == len 表示没有未消费积压。
        // 若 offset < len（daemon 长期未运行导致的积压），此时 rename 会把
        // [offset..] 尚未消费的事件一起丢进 .done，违背"稍后补采"的核心承诺。
        // 这种情况下本轮不归档，交给 worker 用 drain 消费完；待下次重启且
        // offset 追上 len 时再归档。该策略不引入任何新的文件改名/复制操作，
        // 因此不会与 append() 的独占锁产生新竞态，且天然保证 at-least-once。
        if len > MAX_SPOOL_BYTES && offset >= len {
            let rotated = state_dir(home).join(ROTATED_FILE_NAME);
            let _ = std::fs::remove_file(&rotated);
            if std::fs::rename(&path, &rotated).is_ok() {
                offset = 0;
                tail.clear();
            }
        }
        let spool = Self {
            path,
            checkpoint,
            offset,
            tail,
        };
        spool.save_checkpoint();
        spool
    }

    fn reset(&mut self) {
        self.offset = 0;
        self.tail.clear();
    }

    fn save_checkpoint(&self) {
        // 不用 atomic_json 的 fsync：这里丢的只是「已消费」进度，最坏重复一条观察
        // 事件，绝不会丢 hook 本身；每次事件都 fsync 会拖慢 daemon worker。
        let _ = std::fs::write(
            &self.checkpoint,
            json!({"offset":self.offset,"tail":self.tail}).to_string(),
        );
    }

    /// 消费自上次调用以来新增的完整行；不完整的尾行留给下一次。
    pub fn drain(&mut self, mut consume: impl FnMut(Value)) -> Result<usize, String> {
        let len = match std::fs::metadata(&self.path) {
            Ok(meta) => meta.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.to_string()),
        };
        if len < self.offset
            || (self.offset > 0 && !self.tail.is_empty() && self.tail.len() as u64 > self.offset)
        {
            self.reset();
            self.save_checkpoint();
        } else if self.offset > 0
            && !self.tail.is_empty()
            && bytes_at(
                &self.path,
                self.offset - self.tail.len() as u64,
                self.tail.len(),
            )
            .as_deref()
                != Some(self.tail.as_slice())
        {
            // 文件被外部清空或替换：从头开始，绝不跳过新事件。
            self.reset();
            self.save_checkpoint();
        }
        if len == self.offset {
            return Ok(0);
        }
        let mut file = File::open(&self.path).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(self.offset))
            .map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        file.take(len - self.offset)
            .read_to_end(&mut buf)
            .map_err(|e| e.to_string())?;
        let mut count = 0;
        let mut start = 0;
        let mut complete = 0;
        for (i, byte) in buf.iter().enumerate() {
            if *byte == b'\n' {
                let line = &buf[start..i];
                if !line.is_empty() {
                    match serde_json::from_slice::<Value>(line) {
                        Ok(value) => {
                            consume(value);
                            count += 1;
                        }
                        Err(_) => eprintln!("忽略损坏的 hook 暂存记录"),
                    }
                }
                start = i + 1;
                complete = i + 1;
            }
        }
        if complete > 0 {
            let consumed = &buf[..complete];
            let tail_start = consumed.len().saturating_sub(TAIL_BYTES);
            self.tail = consumed[tail_start..].to_vec();
            self.offset += complete as u64;
            self.save_checkpoint();
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn temp_home(tag: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    #[test]
    fn host_hook_prints_empty_object_and_never_waits_for_a_slow_daemon() {
        let home = temp_home("hook-spool-slow-daemon");
        // 端点指向一个收到请求后故意睡 3 秒的服务器；旧实现会在这里超时被拦截。
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        companion_core::atomic_json(
            &spool_path(&home).parent().unwrap().join("runtime-v1.json"),
            &json!({"protocol":1,"port":port,"token":"test"}),
        )
        .unwrap();
        let _ = std::thread::spawn(move || {
            if let Ok(Some(request)) = server.recv_timeout(Duration::from_secs(4)) {
                std::thread::sleep(Duration::from_secs(3));
                let _ = request.respond(tiny_http::Response::empty(204));
            }
        });
        let started = Instant::now();
        let mut out = Vec::new();
        let handled = handle_host_hook(
            &home,
            br#"{"hook_event_name":"UserPromptSubmit","session_id":"s-1"}"#,
            "workbuddy",
            Some("domestic"),
            &mut out,
        )
        .unwrap();
        assert!(handled);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "hook 不得等待 IPC：耗时 {:?}",
            started.elapsed()
        );
        assert_eq!(out, b"{}\n");
        // daemon 此刻才启动：落盘事件必须被补采。
        let mut spool = HookSpool::new(&home);
        let mut events = Vec::new();
        assert_eq!(spool.drain(|v| events.push(v)).unwrap(), 1);
        assert_eq!(events[0]["agent_source"], "workbuddy");
        assert_eq!(events[0]["agent_edition"], "domestic");
        assert_eq!(events[0]["session_id"], "s-1");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn codex_hook_is_not_taken_over_by_the_spool() {
        let home = temp_home("hook-spool-codex");
        let mut out = Vec::new();
        assert!(!handle_host_hook(&home, b"{}", "codex", None, &mut out).unwrap());
        assert!(out.is_empty());
        assert!(!spool_path(&home).exists());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn checkpoint_prevents_duplicate_delivery_and_partial_lines_are_safe() {
        let home = temp_home("hook-spool-checkpoint");
        append(&home, &json!({"id":1})).unwrap();
        append(&home, &json!({"id":2})).unwrap();
        let mut spool = HookSpool::new(&home);
        let mut ids = Vec::new();
        assert_eq!(
            spool
                .drain(|v| ids.push(v["id"].as_u64().unwrap()))
                .unwrap(),
            2
        );
        assert_eq!(ids, vec![1, 2]);
        drop(spool);
        append(&home, &json!({"id":3})).unwrap();
        let mut restarted = HookSpool::new(&home);
        let mut ids = Vec::new();
        assert_eq!(
            restarted
                .drain(|v| ids.push(v["id"].as_u64().unwrap()))
                .unwrap(),
            1
        );
        assert_eq!(ids, vec![3]);
        // 手动制造半行：下一批完整事件仍应被消费。
        let mut file = OpenOptions::new()
            .append(true)
            .open(spool_path(&home))
            .unwrap();
        file.write_all(br#"{"broken":"#).unwrap();
        drop(file);
        append(&home, &json!({"id":4})).unwrap();
        let mut ids = Vec::new();
        assert_eq!(
            restarted
                .drain(|v| ids.push(v["id"].as_u64().unwrap()))
                .unwrap(),
            1
        );
        assert_eq!(ids, vec![4]);
        // 再次消费不得重复投递。
        assert_eq!(restarted.drain(|_| panic!("不得重复投递")).unwrap(), 0);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn truncated_spool_resets_the_offset() {
        let home = temp_home("hook-spool-truncated");
        append(&home, &json!({"id":1})).unwrap();
        let mut spool = HookSpool::new(&home);
        assert_eq!(spool.drain(|_| {}).unwrap(), 1);
        // 截断后重新写入「同样长度」的新事件：旧实现只看 len == offset 会漏掉它。
        std::fs::write(spool_path(&home), b"").unwrap();
        append(&home, &json!({"id":2})).unwrap();
        let mut ids = Vec::new();
        assert_eq!(
            spool
                .drain(|v| ids.push(v["id"].as_u64().unwrap()))
                .unwrap(),
            1
        );
        assert_eq!(ids, vec![2]);
        // 即便守护进程重启，指纹也必须让消费位置回到新文件的起点。
        append(&home, &json!({"id":3})).unwrap();
        let mut restarted = HookSpool::new(&home);
        let mut ids = Vec::new();
        assert_eq!(
            restarted
                .drain(|v| ids.push(v["id"].as_u64().unwrap()))
                .unwrap(),
            1
        );
        assert_eq!(ids, vec![3]);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn concurrent_appends_keep_one_event_per_line() {
        let home = temp_home("hook-spool-concurrent");
        let mut handles = Vec::new();
        for id in 0..8u64 {
            let home = home.clone();
            handles.push(std::thread::spawn(move || {
                append(&home, &json!({"id":id})).unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let mut spool = HookSpool::new(&home);
        let mut ids = Vec::new();
        assert_eq!(
            spool
                .drain(|v| ids.push(v["id"].as_u64().unwrap()))
                .unwrap(),
            8
        );
        ids.sort_unstable();
        assert_eq!(ids, (0..8).collect::<Vec<_>>());
        std::fs::remove_dir_all(&home).unwrap();
    }
}
