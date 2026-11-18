//! 关联组与配对基线的实现；经 `session_link` 重新导出为 crate 公开 API。

mod baseline;
mod store;

pub use baseline::{
    inheritable_baseline, load_baseline, load_pair_baseline, save_baseline, BaselineRecord,
    BaselineState, BASELINE_VERSION,
};
pub use store::{
    active_member_for, add_active_member, find_group_for_identity, find_member, find_pair_base,
    load_store, member_variant, pair_key, representative_member, set_member_state, set_pair_base,
    validate_store, with_link_store_write, LinkGroup, LinkMember, LinkStore, MemberState, PairBase,
    StoreState, LINK_STORE_VERSION,
};
