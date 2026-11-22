//! 桌面通知协议 v1 DTO。
//!
//! 所有 64 位序号一律以十进制字符串传输；内部按非负 `i64` 检查，覆盖超过
//! JavaScript 安全整数与边界值。字段名与 OpenAPI 快照的 camelCase 一致。

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use uuid::Uuid;

use super::error::{ApiFailure, ApiIssue};
use super::utc_time;

/// 十进制字符串序号（非负 i64）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(pub i64);

impl Seq {
    pub const ZERO: Self = Self(0);

    pub fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for Seq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for Seq {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for Seq {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        parse_sequence(&text).map(Self).map_err(D::Error::custom)
    }
}

/// 严格解析十进制序号：1–19 位、无前导零、仅 ASCII 数字、可放入 i64。
pub fn parse_sequence(text: &str) -> Result<i64, String> {
    if text.is_empty() || text.len() > 19 {
        return Err("无效的通知序号".to_owned());
    }
    if text.len() > 1 && text.starts_with('0') {
        return Err("无效的通知序号".to_owned());
    }
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("无效的通知序号".to_owned());
    }
    text.parse::<i64>().map_err(|_| "无效的通知序号".to_owned())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PendingSummary {
    pub pending_task_count: i32,
}

/// 注册请求体（客户端构造）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    pub installation_id: Uuid,
    pub device_name: String,
    pub platform: String,
    pub architecture: String,
    pub client_version: String,
    /// 能力声明：本客户端支持流程标题/流转意见预览（旧客户端不传，服务端只回通用文案）
    pub preview_capable: bool,
}

impl RegisterRequest {
    pub fn new(
        installation_id: Uuid,
        device_name: impl Into<String>,
        platform: impl Into<String>,
        architecture: impl Into<String>,
        client_version: impl Into<String>,
    ) -> Self {
        Self {
            installation_id,
            device_name: device_name.into(),
            platform: platform.into(),
            architecture: architecture.into(),
            client_version: client_version.into(),
            preview_capable: super::PREVIEW_CAPABLE,
        }
    }

    /// 校验并规范化设备名；非法参数在发出网络请求前即失败。
    pub fn normalize(&self) -> Result<Self, ApiFailure> {
        let name = self.device_name.trim();
        let valid_platform = matches!(self.platform.as_str(), "windows" | "linux");
        let valid_architecture = matches!(self.architecture.as_str(), "x64" | "arm64");
        let version_length = self.client_version.chars().count();
        let valid_version = !self.client_version.trim().is_empty()
            && version_length <= 32
            && !self.client_version.chars().any(char::is_control);

        if self.installation_id.is_nil()
            || name.is_empty()
            || name.chars().count() > 64
            || name.chars().any(char::is_control)
            || !valid_platform
            || !valid_architecture
            || !valid_version
        {
            return Err(ApiFailure::new(ApiIssue::Validation));
        }

        Ok(Self {
            installation_id: self.installation_id,
            device_name: name.to_owned(),
            platform: self.platform.clone(),
            architecture: self.architecture.clone(),
            client_version: self.client_version.clone(),
            preview_capable: self.preview_capable,
        })
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RegisterReceipt {
    pub protocol_version: i32,
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub device_token: String,
    pub initial_sequence: Seq,
    pub poll_interval_seconds: i32,
    pub max_page_size: i32,
    #[serde(with = "utc_time")]
    pub server_time: OffsetDateTime,
    pub summary: PendingSummary,
}

// 设备令牌是秘密；手工实现 Debug 以保证诊断与日志不泄漏。
impl fmt::Debug for RegisterReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterReceipt")
            .field("protocol_version", &self.protocol_version)
            .field("user_id", &self.user_id)
            .field("device_id", &self.device_id)
            .field("device_token", &"[redacted device token]")
            .field("initial_sequence", &self.initial_sequence)
            .field("poll_interval_seconds", &self.poll_interval_seconds)
            .field("max_page_size", &self.max_page_size)
            .field("server_time", &self.server_time)
            .field("summary", &self.summary)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub protocol_version: i32,
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub device_name: String,
    pub initial_sequence: Seq,
    pub poll_interval_seconds: i32,
    pub max_page_size: i32,
    #[serde(with = "utc_time")]
    pub server_time: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Feed {
    pub protocol_version: i32,
    #[serde(with = "utc_time")]
    pub server_time: OffsetDateTime,
    pub head_sequence: Seq,
    pub purged_through_sequence: Seq,
    pub next_sequence: Seq,
    pub has_more: bool,
    pub items: Vec<FeedItem>,
    pub summary: PendingSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeedItem {
    pub id: Uuid,
    pub sequence: Seq,
    pub scenario: String,
    #[serde(with = "utc_time")]
    pub occurred_at: OffsetDateTime,
    #[serde(with = "utc_time")]
    pub expires_at: OffsetDateTime,
    pub title: String,
    pub body: String,
    pub target: String,
    pub payload_version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AckReceipt {
    pub acked_sequence: Seq,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResyncReceipt {
    pub initial_sequence: Seq,
    #[serde(with = "utc_time")]
    pub server_time: OffsetDateTime,
    pub summary: PendingSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RevokeReceipt {
    pub revoked: bool,
}
