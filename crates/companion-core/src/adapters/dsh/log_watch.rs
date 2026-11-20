//! DSH 会话日志的帧切分、增量读取与扫描状态。
//!
//! 关键实测结论（决定了这里的实现）：
//!
//! - 一个 `session*.jsonl.zstd` 是**多帧拼接**（886KB 的文件里 416 帧），每帧以
//!   换行结束、从完整行开始。Node 的 `zlib.zstdDecompressSync` 只出第一帧，
//!   所以解码只能做在 Rust 侧。
//! - 因此这里自己解析 zstd 帧头/块头来算帧边界：只有整帧都在缓冲区内才解压并
//!   推进 offset，尾部半帧留到下一轮（DSH 正在追加写入）。
//! - 单轮单文件读 1MB 上限，避免首次遇到大文件把一轮轮询卡住。
use super::events::{Header, HeaderError};
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

/// 扫描节流：清单要求 5 秒。
pub(crate) const SCAN_INTERVAL_MS: i64 = 5_000;
/// 首扫例外窗口：`mtime` 在 2 分钟内的文件才完整解析一次。
pub(crate) const ACTIVE_WINDOW_MS: i64 = 2 * 60_000;
/// 单轮单文件读取上限。
pub(crate) const MAX_READ_PER_ROUND: usize = 1 << 20;
/// 单帧解压输出上限（防解压炸弹）。
pub(crate) const MAX_FRAME_OUTPUT: usize = 8 << 20;
/// 首帧探测长度；实测 header 帧只有 253 字节。
const HEADER_PROBE_BYTES: usize = 8 * 1024;
/// 首帧超过探测长度时的兜底探测长度。
const HEADER_PROBE_FALLBACK: usize = 64 * 1024;
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// 单个日志文件的追踪状态。
#[derive(Debug, Clone)]
pub struct FileState {
    /// 已消费到的字节偏移（只落在帧边界上）。
    pub offset: u64,
    /// 上次看到的 mtime（毫秒）。与 `offset` 都没变就完全跳过。
    pub mtime: i64,
    /// 首帧解析出的会话头；`None` 表示该文件不参与建档。
    pub header: Option<Header>,
    /// 未知格式版本；非空表示停止解析该文件。
    pub unsupported: Option<u32>,
    /// 子代理会话（header 带 `parentSession`）：不建档、不读内容。
    pub hidden: bool,
}

impl FileState {
    fn new(offset: u64, mtime: i64) -> Self {
        Self {
            offset,
            mtime,
            header: None,
            unsupported: None,
            hidden: false,
        }
    }
}

/// DSH 监听的增量状态。
#[derive(Default)]
pub struct LogWatch {
    pub files: HashMap<PathBuf, FileState>,
    /// 每个文件尚未成行的尾巴（理论上不会出现，帧按整行切，留作兜底）。
    pub rest: HashMap<PathBuf, String>,
    /// 用户点关闭的会话：会话 ID → 被关掉的 `roundId`。
    ///
    /// 只记 `closed_monitor_sessions` 不够：那里只有会话 ID，无法区分「同一个 turn
    /// 的后续事件」和「下一个 turn」。DSH 的 `roundId` 就是 `turn:{n}`，所以这里
    /// 记下被关掉的轮次，同轮事件一律丢弃，见到新轮次的 `turn/start` 才解开。
    pub closed: HashMap<String, String>,
    pub primed: bool,
    pub last_scan: i64,
}

impl LogWatch {
    /// 记下被用户关闭的轮次。
    pub fn close(&mut self, session_id: &str, round: &str) {
        if self.closed.len() >= 512 {
            self.closed.clear();
        }
        self.closed.insert(session_id.to_owned(), round.to_owned());
    }
}

/// 返回 `buf` 起始处一个**完整** zstd 帧的字节长度；不足一帧时返回 `None`。
///
/// 只按帧头与块头跳读，不解压，所以可以廉价地对 1MB 缓冲反复调用。
/// 遇到 skippable frame / 保留块类型时返回 `None`（DSH 不会写这两种）。
pub(crate) fn frame_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 5 || buf[..4] != ZSTD_MAGIC {
        return None;
    }
    let fhd = buf[4];
    let fcs_flag = fhd >> 6;
    let single_segment = (fhd >> 5) & 1 == 1;
    let checksum = (fhd >> 2) & 1 == 1;
    let dict_flag = fhd & 3;
    let mut p: usize = 5;
    if !single_segment {
        p = p.checked_add(1)?; // Window_Descriptor
    }
    p = p.checked_add([0usize, 1, 2, 4][dict_flag as usize])?;
    p = p.checked_add(match fcs_flag {
        0 => usize::from(single_segment),
        1 => 2,
        2 => 4,
        _ => 8,
    })?;
    loop {
        let header = buf.get(p..p.checked_add(3)?)?;
        let block = u32::from_le_bytes([header[0], header[1], header[2], 0]);
        p += 3;
        let last = block & 1 == 1;
        let kind = (block >> 1) & 3;
        let size = (block >> 3) as usize;
        if kind == 3 {
            return None; // 保留块类型：不是合法帧
        }
        // RLE 块只带 1 字节负载，其余块带 `size` 字节。
        p = p.checked_add(if kind == 1 { 1 } else { size })?;
        if p > buf.len() {
            return None;
        }
        if last {
            break;
        }
    }
    if checksum {
        p = p.checked_add(4)?;
        if p > buf.len() {
            return None;
        }
    }
    Some(p)
}

/// 解压一个完整帧。输出上限 `MAX_FRAME_OUTPUT`。
pub(crate) fn decode_frame(frame: &[u8]) -> Result<String, String> {
    let mut out = Vec::new();
    let mut decoder = zstd::stream::read::Decoder::new(frame)
        .map_err(|e| format!("DSH 会话帧解压失败：{e}"))?;
    decoder
        .by_ref()
        .take(MAX_FRAME_OUTPUT as u64)
        .read_to_end(&mut out)
        .map_err(|e| format!("DSH 会话帧解压失败：{e}"))?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// 读首帧并解析会话头。文件正在写时首帧也可能不完整，返回 `Unreadable` 让调用方
/// 下一轮重试。
pub(crate) fn read_header(path: &Path) -> Result<Header, HeaderError> {
    for probe_len in [HEADER_PROBE_BYTES, HEADER_PROBE_FALLBACK] {
        let Ok(mut file) = std::fs::File::open(path) else {
            return Err(HeaderError::Unreadable);
        };
        let mut probe = vec![0u8; probe_len];
        let Ok(n) = file.read(&mut probe) else {
            return Err(HeaderError::Unreadable);
        };
        probe.truncate(n);
        let Some(len) = frame_len(&probe) else { continue };
        let Ok(text) = decode_frame(&probe[..len]) else {
            return Err(HeaderError::Unreadable);
        };
        let Some(line) = text.lines().find(|line| !line.trim().is_empty()) else {
            return Err(HeaderError::Malformed);
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            return Err(HeaderError::Malformed);
        };
        return super::events::parse_header(&value);
    }
    Err(HeaderError::Unreadable)
}

/// 会话日志文件名：`session.jsonl.zstd`（v0）或 `session.v4.jsonl.zstd`（v4）。
///
/// 不写死版本号，否则 DSH 一升级格式我们就看不到任何会话。
pub fn is_session_log(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("session") && name.ends_with(".jsonl.zstd"))
}

/// 收集 `sessions/<project>/<session-dir>/session*.jsonl.zstd`。
///
/// 目录名不可假设（实测同时存在 `session-<uuid>` 与裸 `<uuid>`），所以只按层级
/// 与文件名匹配，会话 ID 一律取 header 的 `id`。
pub fn collect_session_logs(dir: &Path, out: &mut Vec<PathBuf>) {
    walk(dir, 0, out);
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    children.sort();
    for path in children {
        if path.is_dir() {
            walk(&path, depth + 1, out);
        } else if is_session_log(&path) {
            out.push(path);
        }
    }
}

/// 从 `offset` 起读一段并返回读取到的字节；不修改状态。
pub(crate) fn read_chunk(path: &Path, offset: u64, size: u64) -> Option<Vec<u8>> {
    if size <= offset {
        return None;
    }
    let want = MAX_READ_PER_ROUND.min((size - offset) as usize);
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = vec![0u8; want];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(buf)
}

/// 注册新看到的日志文件：读 header、决定从哪个偏移开始消费。
///
/// 四种情形（`priming` 表示这是启用监听后的首轮扫描）：
///
/// | 情形 | 起始偏移 | 原因 |
/// | --- | --- | --- |
/// | 子代理（有 `parentSession`） | 文件末尾 | 隐藏，永不读内容 |
/// | 首扫 + mtime 在 2 分钟内 | 0 | 例外：正在跑的会话要立刻可见 |
/// | 首扫 + 历史文件 | 文件末尾 | 346 个历史会话一个都不能刷进浮窗 |
/// | 首扫之后新出现的文件 | 0 | 运行期新建的文件本身就是活跃会话 |
///
/// 返回 `None` 表示首帧还不完整（DSH 正在写）：不登记，下一轮再试。
pub(crate) fn register(
    path: &Path,
    size: u64,
    mtime: i64,
    priming: bool,
    active: bool,
) -> Option<FileState> {
    let mut state = FileState::new(size, mtime);
    match read_header(path) {
        Ok(header) => {
            state.hidden = header.parent.is_some();
            let replay = !state.hidden && (!priming || active);
            state.header = Some(header);
            if replay {
                state.offset = 0;
            }
        }
        Err(HeaderError::Unsupported(version)) => {
            state.unsupported = Some(version);
            state.hidden = true;
        }
        Err(HeaderError::Malformed) => {
            state.hidden = true;
        }
        // 首帧还没写完：下一轮重试。
        Err(HeaderError::Unreadable) => return None,
    }
    Some(state)
}
