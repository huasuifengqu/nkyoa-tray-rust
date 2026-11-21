//! 平台入口需要实现的 trait，以及 UI 订阅的状态快照。

use std::time::Duration;

use async_trait::async_trait;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::protocol::Binding;
use crate::protocol::error::{ApiFailure, StoreError};
use crate::protocol::origin::ServerOrigin;

use super::state::{ClientState, StateRead};

/// 注册时提交的设备信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRegistration {
    pub name: String,
    pub platform: String,
    pub architecture: String,
    pub client_version: String,
}

/// 唯一活跃绑定；替换必须原子；实现应使用平台安全存储。
#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn read(&self, cancel: &CancellationToken) -> Result<Option<Binding>, ClientError>;
    async fn save(&self, binding: &Binding, cancel: &CancellationToken) -> Result<(), ClientError>;
    async fn remove(&self, cancel: &CancellationToken) -> Result<(), ClientError>;
}

/// 安装标识与按配置隔离的接收状态。
#[async_trait]
pub trait ClientStateStore: Send + Sync {
    async fn installation_id(&self, cancel: &CancellationToken) -> Result<Uuid, ClientError>;

    /// 未来版本状态以 `Api(UnsupportedVersion)` 返回，且不得覆盖原文件。
    async fn read(
        &self,
        binding: &Binding,
        cancel: &CancellationToken,
    ) -> Result<StateRead, ClientError>;

    async fn save(
        &self,
        binding: &Binding,
        state: &ClientState,
        cancel: &CancellationToken,
    ) -> Result<(), ClientError>;
}

/// 系统通知交付结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryResult {
    Accepted,
    RetryableFailure,
    Unavailable,
}

#[async_trait]
pub trait SystemNotifier: Send + Sync {
    async fn show(
        &self,
        popup: &Popup,
        cancel: &CancellationToken,
    ) -> Result<DeliveryResult, ClientError>;
}

/// 待展示的通知页摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Popup {
    pub title: String,
    pub body: String,
    pub home: Option<ServerOrigin>,
    pub event_count: i32,
    pub profile_id: Option<String>,
}

/// 恢复原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryReason {
    HistoryExpired,
    StreamChanged,
    DamagedState,
    DamagedStateWithHistoryExpired,
}

/// 客户端阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientPhase {
    Unconfigured,
    Binding,
    Connected,
    LocallyPaused,
    Offline,
    Backoff,
    Resynchronizing,
    CredentialInvalid,
    ServerDisabled,
    CertificateError,
    UpgradeRequired,
    RequestError,
    StorageError,
    NotificationUnavailable,
    Stopping,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryReport {
    pub accepted_popups: i32,
    pub aggregated_events: i32,
    pub suppressed_events: i32,
}

/// 一次接收结果的 UI 快照；可选字段在连续更新中沿用上一值。
#[derive(Debug, Clone, PartialEq)]
pub struct ClientStatus {
    pub phase: ClientPhase,
    pub issue: Option<crate::protocol::ApiIssue>,
    pub retry_in: Option<Duration>,
    pub pending_task_count: Option<i32>,
    pub recovery: Option<RecoveryReason>,
    pub delivery: Option<DeliveryReport>,
    pub last_sync_at: Option<OffsetDateTime>,
    pub account_id: Option<Uuid>,
    pub device_name: Option<String>,
}

impl ClientStatus {
    pub fn new(phase: ClientPhase) -> Self {
        Self {
            phase,
            issue: None,
            retry_in: None,
            pending_task_count: None,
            recovery: None,
            delivery: None,
            last_sync_at: None,
            account_id: None,
            device_name: None,
        }
    }

    pub fn with_issue(mut self, issue: crate::protocol::ApiIssue) -> Self {
        self.issue = Some(issue);
        self
    }

    pub fn with_retry_in(mut self, retry_in: Duration) -> Self {
        self.retry_in = Some(retry_in);
        self
    }

    pub fn with_pending(mut self, pending_task_count: i32) -> Self {
        self.pending_task_count = Some(pending_task_count);
        self
    }

    pub fn with_pending_optional(mut self, pending_task_count: Option<i32>) -> Self {
        self.pending_task_count = pending_task_count;
        self
    }

    pub fn with_recovery(mut self, recovery: RecoveryReason) -> Self {
        self.recovery = Some(recovery);
        self
    }

    pub fn with_delivery(mut self, delivery: DeliveryReport) -> Self {
        self.delivery = Some(delivery);
        self
    }

    pub fn with_last_sync_at(mut self, last_sync_at: OffsetDateTime) -> Self {
        self.last_sync_at = Some(last_sync_at);
        self
    }

    pub fn with_account(mut self, account_id: Uuid) -> Self {
        self.account_id = Some(account_id);
        self
    }

    pub fn with_device_name(mut self, device_name: String) -> Self {
        self.device_name = Some(device_name);
        self
    }
}

impl Default for ClientStatus {
    fn default() -> Self {
        Self::new(ClientPhase::Unconfigured)
    }
}

/// 核心层错误；取消不是失败，单独传播以便入口安静退出。
#[derive(Debug, Clone)]
pub enum ClientError {
    Api(ApiFailure),
    Storage(StoreError),
    Notification(DeliveryResult),
    Cancelled,
}

impl From<ApiFailure> for ClientError {
    fn from(value: ApiFailure) -> Self {
        Self::Api(value)
    }
}

impl From<StoreError> for ClientError {
    fn from(value: StoreError) -> Self {
        Self::Storage(value)
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api(failure) => write!(f, "{failure}"),
            Self::Storage(error) => write!(f, "{error}"),
            Self::Notification(result) => write!(f, "notification-{result:?}"),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for ClientError {}
