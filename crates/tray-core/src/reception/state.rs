//! 持久化的接收状态 v1。
//!
//! 字段形态与 `contracts/client-state.schema.json` 一致：序号是十进制字符串，
//! `resyncPending` 是 0/1/null，明细条目复用协议 `FeedItem`。

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::protocol::dto::{FeedItem, Seq};
use crate::protocol::error::{ApiFailure, ApiIssue};
use crate::protocol::validation;
use crate::protocol::{MAX_PAGE_SIZE, MAX_POPUPS_PER_PAGE};

/// 状态文件允许的恢复原因（仅历史过期 / 流变化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResyncReason {
    HistoryExpired,
    StreamChanged,
}

impl ResyncReason {
    pub fn from_recovery(reason: super::contracts::RecoveryReason) -> Option<Self> {
        use super::contracts::RecoveryReason;
        match reason {
            RecoveryReason::HistoryExpired => Some(Self::HistoryExpired),
            RecoveryReason::StreamChanged => Some(Self::StreamChanged),
            RecoveryReason::DamagedState | RecoveryReason::DamagedStateWithHistoryExpired => None,
        }
    }

    pub fn to_recovery(self) -> super::contracts::RecoveryReason {
        use super::contracts::RecoveryReason;
        match self {
            Self::HistoryExpired => RecoveryReason::HistoryExpired,
            Self::StreamChanged => RecoveryReason::StreamChanged,
        }
    }
}

impl Serialize for ResyncReason {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i32(match self {
            Self::HistoryExpired => 0,
            Self::StreamChanged => 1,
        })
    }
}

impl<'de> Deserialize<'de> for ResyncReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match i32::deserialize(deserializer)? {
            0 => Ok(Self::HistoryExpired),
            1 => Ok(Self::StreamChanged),
            _ => Err(D::Error::custom("无效的恢复原因")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientState {
    pub version: i32,
    pub initial: Seq,
    pub through: Seq,
    pub ack_pending: bool,
    pub pending: Option<PendingPage>,
    pub resync_pending: Option<ResyncReason>,
}

impl ClientState {
    pub const CURRENT_VERSION: i32 = 1;

    pub fn at(sequence: i64) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            initial: Seq(sequence),
            through: Seq(sequence),
            ack_pending: false,
            pending: None,
            resync_pending: None,
        }
    }

    pub fn with_pending(mut self, pending: Option<PendingPage>) -> Self {
        self.pending = pending;
        self
    }

    pub fn with_ack_pending(mut self, ack_pending: bool) -> Self {
        self.ack_pending = ack_pending;
        self
    }

    pub fn with_resync_pending(mut self, reason: Option<ResyncReason>) -> Self {
        self.resync_pending = reason;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPage {
    pub items: Vec<FeedItem>,
    pub processed: i32,
    pub accepted: i32,
    pub suppress: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StateRead {
    pub state: Option<ClientState>,
    pub corrupt: bool,
}

/// 校验状态文件；任何不一致都视为损坏（由调用方转成 `corrupt`）。
pub fn validate(state: &ClientState) -> Result<(), ApiFailure> {
    validation::require(
        state.version == ClientState::CURRENT_VERSION
            && state.initial.0 >= 0
            && state.through.0 >= state.initial.0
            && (state.pending.is_some() || !state.ack_pending),
    )?;

    let Some(page) = &state.pending else {
        return Ok(());
    };

    let count = page.items.len() as i64;
    validation::require(
        (1..=i64::from(MAX_PAGE_SIZE)).contains(&count)
            && state.through.0 - state.initial.0 >= count
            && page.processed >= 0
            && i64::from(page.processed) < count
            && page.accepted >= 0
            && page.accepted <= page.processed
            && page.accepted < MAX_POPUPS_PER_PAGE
            && (!state.ack_pending || page.processed == 0),
    )?;

    validation::items(&page.items, state.through.0 - count, state.through.0)
        .map_err(|_| ApiFailure::new(ApiIssue::InvalidResponse))
}
