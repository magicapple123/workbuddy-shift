//! WorkBuddy / CodeBuddy IDE hook 的「即发即走」落盘队列。
//!
//! 宿主（WorkBuddy / CodeBuddy IDE）给 hook 只有 3000ms，且这段预算包含
//! `bash.exe` 启动、原生 exe 冷启动、HTTP IPC 排队等全部开销。任何同步 RPC
//! 都会在忙时撞上这个上限，于是宿主把 hook 报成超时并拦截操作。
//!
//! 这两类宿主的 hook 只做「观察」，不需要宿主等待采集结果，注册的命令也不再是 exe：
//! v0.4.3 起注册的是 `bash '<脚本>'`（见 [`crate::hook_script`]），脚本把
//! 「来源指令行 + 原始 payload」写进**自己的一份事件文件**后立即输出 `{}` 退出，不解析
//! JSON、不冷启动 5MB 的 exe（issue #20 的回归：冷启动在慢盘 / 杀软环境下要 1.88–2.19s）。
//! 一事件一文件是为了让并发 hook 物理上无法交错 —— shell 的 `printf … >> file` 不是一次
//! `write()`，16 并发时实测会丢事件（见 [`crate::hook_inbox`]）。
//!
//! 消费端因此有两条链路，[`HookSpool::drain`] 一次都跑：
//!
//! 1. [`crate::hook_inbox`]：脚本发布的事件文件（`.evt`），读一条删一条；
//! 2. 本模块的单文件队列 `companion-hook-spool.jsonl`：v0.4.2 的 exe 命令与遗留事件
//!    继续写在这里，按字节偏移 + 内容指纹增量消费（旧队列里未消费的事件不会被升级丢弃）。
//!
//! 队列里可能同时存在两种格式：脚本写的「指令行 + payload」（来源在指令行里）与
//! v0.4.2 及更早 exe 写的「单行 JSON」（自带 `agent_source`）。解码规则统一在
//! [`crate::hook_frame`]。
//!
//! 与 `wb-shift-core` 的积分签到系统完全隔离：签到写
//! `~/.workbuddy-shift/hook-events.jsonl`，本模块写
//! `~/.workbuddy-shift-companion/companion-hook-spool.jsonl` 与 `hook-inbox.d/`，两者互不读写。

use crate::hook_frame::{Frame, HookFrameDecoder};
use crate::hook_inbox::EventInbox;
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
    // 与脚本链路共用同一份盖章语义：非 object 的 payload 包装成 `payload` 字段，
    // 不丢弃、也不会在 `value[key] = …` 上 panic（hook 绝不能反过来打断宿主）。
    let payload = crate::hook_frame::stamp_source(raw, source, edition);
    if let Err(e) = append(home, &payload) {
        eprintln!("hook 事件暂存失败（不拦截宿主）：{e}");
    }
    Ok(true)
}

/// 以独占文件锁串行化本文件队列的所有写入者（走 exe 的 codex 与 v0.4.2 遗留命令；
/// 脚本已改为写收件箱，不再碰这里），保证一条事件恰好占一行。
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
///
/// 同时负责脚本收件箱（[`crate::hook_inbox`]）：`drain` 会先清空单文件队列里的积压，
/// 再消费收件箱，保证「旧事件先于新事件」的投递顺序。
pub struct HookSpool {
    path: PathBuf,
    checkpoint: PathBuf,
    offset: u64,
    tail: Vec<u8>,
    /// 跨行 payload 与「来源指令行」的解析状态，必须跨 `drain` 调用保留。
    frames: HookFrameDecoder,
    /// 脚本发布的事件文件（一事件一文件）。
    inbox: EventInbox,
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
            frames: HookFrameDecoder::new(),
            inbox: EventInbox::new(home),
        };
        spool.save_checkpoint();
        spool
    }

    fn reset(&mut self) {
        self.offset = 0;
        self.tail.clear();
        self.frames.reset();
    }

    fn save_checkpoint(&self) {
        // 不用 atomic_json 的 fsync：这里丢的只是「已消费」进度，最坏重复一条观察
        // 事件，绝不会丢 hook 本身；每次事件都 fsync 会拖慢 daemon worker。
        let _ = std::fs::write(
            &self.checkpoint,
            json!({"offset":self.offset,"tail":self.tail}).to_string(),
        );
    }

    /// 消费全部待处理事件：先清空单文件队列（v0.4.2 遗留 / exe 写入者），再消费脚本
    /// 收件箱里的 `.evt` 文件。两段都按「至少一次」语义投递。
    pub fn drain(&mut self, mut consume: impl FnMut(Value)) -> Result<usize, String> {
        let mut count = self.drain_file(&mut consume)?;
        count += self.inbox.drain(&mut consume)?;
        Ok(count)
    }

    /// 消费单文件队列里自上次调用以来新增的完整行；不完整的尾行留给下一次。
    fn drain_file(&mut self, consume: &mut impl FnMut(Value)) -> Result<usize, String> {
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
                match std::str::from_utf8(&buf[start..i]) {
                    Ok(line) => match self.frames.push(line) {
                        Frame::Event(value) => {
                            // 投递回调 panic 时不推进 offset ⇒ 下次重投（至少一次）。
                            consume(value);
                            count += 1;
                        }
                        Frame::Incomplete | Frame::Skipped => {}
                        Frame::Invalid => eprintln!("忽略损坏的 hook 暂存记录"),
                    },
                    Err(_) => eprintln!("忽略损坏的 hook 暂存记录"),
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
