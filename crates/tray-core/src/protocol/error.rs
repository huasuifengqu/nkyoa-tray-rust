//! 协议层错误类型。
//!
//! 错误文案只用于本机诊断，绝不把服务器返回的 `detail` 等文本写入错误对象，
//! 避免敏感信息经托盘/诊断外传。

use std::fmt;

/// 与服务端 v1 错误码一一对应的客户端问题分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApiIssue {
    CredentialInvalid,
    Csrf,
    DeviceLimit,
    VersionConflict,
    StreamReset,
    HistoryExpired,
    Validation,
    RateLimited,
    ServerDisabled,
    ServerUnavailable,
    Offline,
    Certificate,
    UnsupportedVersion,
    InvalidResponse,
}

impl ApiIssue {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CredentialInvalid => "credential-invalid",
            Self::Csrf => "csrf",
            Self::DeviceLimit => "device-limit",
            Self::VersionConflict => "version-conflict",
            Self::StreamReset => "stream-reset",
            Self::HistoryExpired => "history-expired",
            Self::Validation => "validation",
            Self::RateLimited => "rate-limited",
            Self::ServerDisabled => "server-disabled",
            Self::ServerUnavailable => "server-unavailable",
            Self::Offline => "offline",
            Self::Certificate => "certificate",
            Self::UnsupportedVersion => "unsupported-version",
            Self::InvalidResponse => "invalid-response",
        }
    }

    /// 可退避重试的问题；其余问题在接收循环中直接停止。
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::Offline | Self::RateLimited | Self::ServerUnavailable | Self::ServerDisabled
        )
    }
}

/// 一次 API 调用的失败；`retry_after` 来自远端 `Retry-After` 头。
#[derive(Debug, Clone)]
pub struct ApiFailure {
    pub issue: ApiIssue,
    pub retry_after: Option<std::time::Duration>,
}

impl ApiFailure {
    pub const fn new(issue: ApiIssue) -> Self {
        Self {
            issue,
            retry_after: None,
        }
    }

    pub const fn with_retry_after(mut self, retry_after: Option<std::time::Duration>) -> Self {
        self.retry_after = retry_after;
        self
    }
}

impl fmt::Display for ApiFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.issue.as_str())
    }
}

impl std::error::Error for ApiFailure {}

/// 本机状态或系统凭据库访问失败（不含任何凭证内容）。
#[derive(Debug, Clone)]
pub struct StoreError(pub String);

impl StoreError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// 服务器地址必须是不带路径的 HTTPS 根路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OriginError;

impl fmt::Display for OriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("请输入 HTTPS 站点域名或首页地址。")
    }
}

impl std::error::Error for OriginError {}

/// 通知凭证格式不正确。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialError;

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("通知凭证格式不正确。")
    }
}

impl std::error::Error for CredentialError {}
