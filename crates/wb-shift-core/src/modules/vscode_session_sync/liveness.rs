//! 会话存活判定（预览 / 管理页 / 批量清理**共用同一份结论**）。
//!
//! 插件的会话「行」由两份证据共同决定，优先级从强到弱：
//!
//! 1. **索引命中**：`index.json` 里有该会话条目（`Partial` 下命中可读分片即可）→
//!    [`SessionLiveness::Alive`]；
//! 2. **目录存在**：`history/<workspaceHash>/<conversationId>/` 还在 → `Alive`。
//!    这是本轮新增的更强证据：条目可能因为索引损坏、半截写入或改归属而缺失，目录却在；
//! 3. **两者都没有**时，才有资格谈「消失」，且必须看索引四态（[`ConversationIndexState`]）：
//!    - `Available`（每一份索引都解析成功）→ [`SessionLiveness::Gone`]（可信消失）；
//!    - `Partial` / `Unavailable` / `Absent` → [`SessionLiveness::Unknown`]（没有完整依据）。
//!
//! 三处消费方对本结论的取舍不同，但**判定只有这一份**：
//!
//! | 消费方 | `Alive` | `Unknown` |
//! | --- | --- | --- |
//! | 预览（`links_preview*`） | 保留 | fail-open：保留 |
//! | 管理页（`visible_groups`） | 展示 | fail-open：展示 |
//! | 批量清理（`dead_groups_by_index`） | 不删 | fail-closed：拒绝整次清理 |
//!
//! 判定按 `(spec, root, uid)` 缓存（[`LivenessJudge`]）：管理页 / 清理一次可能判几十上百个
//! 会话，同一账号只该扫一遍盘——`conversation_index_scan` 一次遍历就带回索引四态、定位表
//! 与磁盘目录集合。

use super::*;

/// 单条会话的存活判定结果（三态，语义见模块注释）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionLiveness {
    /// 会话确实存在：索引条目命中，或磁盘目录还在。
    Alive,
    /// 会话确实消失：每一份工作区索引都解析成功，且索引与目录里都没有它。
    Gone,
    /// 无法判定：索引未命中、目录也不存在，但索引本身不完整（附四态供错误文案使用）。
    Unknown(ConversationIndexState),
}

/// 单条会话的存活判定（唯一的组合判定点）。
///
/// 调用方不要自己判「目录在不在」或「索引是否可用」：任何一处另写一份判定，都会出现
/// 「管理页隐藏了但清理不删」或反过来的不一致。
pub(super) fn session_liveness(scan: &ConversationIndexScan, session_id: &str) -> SessionLiveness {
    if scan.holds(session_id) {
        return SessionLiveness::Alive;
    }
    match scan.state() {
        // 索引读得全 + 目录也没有：这才是「可信消失」。
        ConversationIndexState::Available(_) => SessionLiveness::Gone,
        state => SessionLiveness::Unknown(state),
    }
}

/// 按账号缓存工作区扫描的判定器（同一请求内一个账号只扫一遍盘）。
pub(crate) struct LivenessJudge {
    spec: SessionStoreSpec,
    root: PathBuf,
    scans: BTreeMap<String, ConversationIndexScan>,
}

impl LivenessJudge {
    /// `root` 必须是该客户端（`spec`）的数据根；解析不到时调用方按 fail-open / 拒绝处理，
    /// 不要用 `None` 造一个「什么都判不了」的判定器来猜。
    pub(crate) fn new(spec: SessionStoreSpec, root: &Path) -> Self {
        Self {
            spec,
            root: root.to_path_buf(),
            scans: BTreeMap::new(),
        }
    }

    /// 某账号的扫描结果（索引四态原料 + 定位表 + 磁盘目录集合）。
    pub(super) fn scan(&mut self, uid: &str) -> &ConversationIndexScan {
        let (spec, root) = (self.spec, self.root.as_path());
        self.scans
            .entry(uid.to_string())
            .or_insert_with(|| conversation_index_scan(spec, root, uid))
    }

    /// 某账号某会话的存活判定。
    pub(crate) fn liveness(&mut self, uid: &str, session_id: &str) -> SessionLiveness {
        session_liveness(self.scan(uid), session_id)
    }

    /// 组内该账号**代表成员**的存活判定。
    ///
    /// 代表成员口径与标题/摘要一致：只看当前副本，不看历史 superseded 副本（旧副本还在、
    /// 当前副本已删的组必须判为失效）。组内没有该账号成员时没有会话可谈 → `Gone`。
    pub(crate) fn member_liveness(&mut self, group: &LinkGroup, uid: &str) -> SessionLiveness {
        let Some(session_id) =
            session_link::representative_member(group, uid).map(|member| member.session_id.clone())
        else {
            return SessionLiveness::Gone;
        };
        self.liveness(uid, &session_id)
    }
}
