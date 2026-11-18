//! Codeg 事件与实时流：webhook 载荷、快照对齐与事件落库。
use super::*;
use crate::{merge, question, questions};

mod ingest;
mod snapshot;
mod stream;

pub(crate) use snapshot::{codeg_ask, codeg_title};
pub(crate) use stream::{request_id, stream_ask};
