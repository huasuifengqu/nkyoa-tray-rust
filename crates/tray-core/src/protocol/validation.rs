//! 协议响应校验。
//!
//! 任何不满足契约的响应都不能推进游标；非法响应统一抛 `InvalidResponse`，
//! 未知协议/载荷版本抛 `UnsupportedVersion`。

use super::dto::{Feed, FeedItem, PendingSummary, Seq};
use super::error::{ApiFailure, ApiIssue};
use super::{MAX_PAGE_SIZE, PROTOCOL_VERSION, SCENARIO_KEYS};

const MIN_POLL_INTERVAL_SECONDS: i32 = 10;
const MAX_POLL_INTERVAL_SECONDS: i32 = 3600;

pub fn version(version: i32) -> Result<(), ApiFailure> {
    if version != PROTOCOL_VERSION {
        return Err(ApiFailure::new(ApiIssue::UnsupportedVersion));
    }
    Ok(())
}

pub fn device_parameters(
    user_id: uuid::Uuid,
    device_id: uuid::Uuid,
    initial: Seq,
    poll_seconds: i32,
    max_page_size: i32,
) -> Result<(), ApiFailure> {
    require(
        !user_id.is_nil()
            && !device_id.is_nil()
            && initial.0 >= 0
            && (MIN_POLL_INTERVAL_SECONDS..=MAX_POLL_INTERVAL_SECONDS).contains(&poll_seconds)
            && (1..=MAX_PAGE_SIZE).contains(&max_page_size),
    )
}

pub fn summary(summary: &PendingSummary) -> Result<(), ApiFailure> {
    require(summary.pending_task_count >= 0)
}

pub fn page(feed: &Feed, after: i64, limit: i32) -> Result<(), ApiFailure> {
    version(feed.protocol_version)?;
    summary(&feed.summary)?;
    require(
        feed.purged_through_sequence.0 >= 0
            && feed.purged_through_sequence.0 <= after
            && feed.next_sequence.0 >= after
            && feed.head_sequence.0 >= feed.next_sequence.0
            && feed.items.len() as i64 <= i64::from(limit)
            && feed.has_more == (feed.next_sequence.0 < feed.head_sequence.0)
            && (!feed.has_more || feed.items.len() as i64 == i64::from(limit)),
    )?;
    items(&feed.items, after, feed.next_sequence.0)
}

pub fn items(items: &[FeedItem], after: i64, through: i64) -> Result<(), ApiFailure> {
    let mut ids = std::collections::HashSet::with_capacity(items.len());
    let mut previous = after;
    for item in items {
        version(item.payload_version)?;
        require(
            !item.id.is_nil()
                && ids.insert(item.id)
                && item.sequence.0 > previous
                && item.sequence.0 - previous == 1
                && item.sequence.0 <= through
                && item.expires_at > item.occurred_at
                && SCENARIO_KEYS.contains(&item.scenario.as_str())
                && item.target == "oa-home"
                && valid_text(&item.title, 256)
                && valid_text(&item.body, 2048),
        )?;
        previous = item.sequence.0;
    }
    require(previous == through)
}

pub fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control)
}

pub fn require(valid: bool) -> Result<(), ApiFailure> {
    if valid {
        Ok(())
    } else {
        Err(ApiFailure::new(ApiIssue::InvalidResponse))
    }
}
