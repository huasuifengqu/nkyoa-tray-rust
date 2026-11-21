//! 桌面通知 v1 HTTPS 客户端。
//!
//! 每个操作都校验状态码、媒体类型、信封与字段级契约；服务器诊断文本不会
//! 进入错误对象，只有可判定的问题分类。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

use super::credential::{Binding, DeviceToken, PairingCode};
use super::dto::{
    AckReceipt, DeviceView, Feed, RegisterReceipt, RegisterRequest, ResyncReceipt, RevokeReceipt,
    Seq,
};
use super::error::{ApiFailure, ApiIssue};
use super::origin::ServerOrigin;
use super::transport::{
    HttpRequest, HttpResponse, HttpTransport, ReqwestTransport, TransportError,
};
use super::validation;
use super::{
    AUTHORIZATION_SCHEME, CSRF_HEADER_NAME, CSRF_HEADER_VALUE, MAX_ACK_BODY_BYTES,
    MAX_FEED_RESPONSE_BYTES, MAX_PAGE_SIZE, MAX_REGISTER_BODY_BYTES, PATH_ACKS, PATH_DEVICE,
    PATH_NOTIFICATIONS, PATH_REGISTER, PATH_RESYNC, PATH_REVOKE_SELF,
};

const DEFAULT_RESPONSE_BYTES: usize = 16 * 1024;
const PROBLEM_RESPONSE_BYTES: usize = 16 * 1024;

/// 平台入口依赖的协议接口；测试可替换为假实现。
#[async_trait]
pub trait NotificationApi: Send + Sync {
    async fn register(
        &self,
        origin: &ServerOrigin,
        code: &PairingCode,
        request: &RegisterRequest,
        cancel: &CancellationToken,
    ) -> Result<RegisterReceipt, ApiFailure>;

    async fn get_device(
        &self,
        binding: &Binding,
        cancel: &CancellationToken,
    ) -> Result<DeviceView, ApiFailure>;

    async fn get_feed(
        &self,
        binding: &Binding,
        after: i64,
        limit: i32,
        cancel: &CancellationToken,
    ) -> Result<Feed, ApiFailure>;

    async fn ack(
        &self,
        binding: &Binding,
        through: i64,
        cancel: &CancellationToken,
    ) -> Result<i64, ApiFailure>;

    async fn resync(
        &self,
        binding: &Binding,
        cancel: &CancellationToken,
    ) -> Result<ResyncReceipt, ApiFailure>;

    async fn revoke(&self, binding: &Binding, cancel: &CancellationToken)
    -> Result<(), ApiFailure>;
}

pub struct DesktopApi {
    transport: Arc<dyn HttpTransport>,
}

impl DesktopApi {
    pub fn new() -> Self {
        Self::with_transport(Arc::new(ReqwestTransport::new()))
    }

    pub fn with_transport(transport: Arc<dyn HttpTransport>) -> Self {
        Self { transport }
    }

    #[allow(clippy::too_many_arguments)]
    async fn send<T>(
        &self,
        origin: &ServerOrigin,
        method: &str,
        path: &str,
        credential: &str,
        body: Option<Vec<u8>>,
        max_response_bytes: usize,
        cancel: &CancellationToken,
    ) -> Result<T, ApiFailure>
    where
        T: DeserializeOwned,
    {
        let mut url = origin
            .url()
            .join(path)
            .map_err(|_| ApiFailure::new(ApiIssue::Validation))?;
        if let Some(query) = path.split_once('?').map(|(_, query)| query) {
            url.set_query(Some(query));
        }

        let mut headers = vec![
            (
                "Authorization".to_owned(),
                format!("{AUTHORIZATION_SCHEME} {credential}"),
            ),
            (
                "Accept".to_owned(),
                "application/json, application/problem+json".to_owned(),
            ),
        ];
        if method != "GET" {
            headers.push((CSRF_HEADER_NAME.to_owned(), CSRF_HEADER_VALUE.to_owned()));
        }

        let request = HttpRequest {
            method: method.to_owned(),
            url,
            headers,
            body,
            max_response_bytes,
        };
        let response = self
            .transport
            .send(request, cancel)
            .await
            .map_err(transport_failure)?;

        if response.status != 200 {
            return Err(classify_failure(&response));
        }
        if !media_type(response.content_type.as_deref())
            .is_some_and(|value| value.eq_ignore_ascii_case("application/json"))
        {
            return Err(ApiFailure::new(ApiIssue::InvalidResponse));
        }

        let envelope: Envelope<T> = serde_json::from_slice(&response.body)
            .map_err(|_| ApiFailure::new(ApiIssue::InvalidResponse))?;
        if !envelope.success
            || envelope.data.is_none()
            || envelope.error.is_some()
            || envelope.code.is_some()
        {
            return Err(ApiFailure::new(ApiIssue::InvalidResponse));
        }
        Ok(envelope.data.expect("已检查 data 非空"))
    }
}

impl Default for DesktopApi {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl NotificationApi for DesktopApi {
    async fn register(
        &self,
        origin: &ServerOrigin,
        code: &PairingCode,
        request: &RegisterRequest,
        cancel: &CancellationToken,
    ) -> Result<RegisterReceipt, ApiFailure> {
        let request = request.normalize()?;
        let body =
            serde_json::to_vec(&request).map_err(|_| ApiFailure::new(ApiIssue::Validation))?;
        if body.len() > MAX_REGISTER_BODY_BYTES {
            return Err(ApiFailure::new(ApiIssue::Validation));
        }

        let wire: RegisterReceipt = self
            .send(
                origin,
                "POST",
                PATH_REGISTER,
                code.value(),
                Some(body),
                DEFAULT_RESPONSE_BYTES,
                cancel,
            )
            .await?;

        validation::version(wire.protocol_version)?;
        validation::device_parameters(
            wire.user_id,
            wire.device_id,
            wire.initial_sequence,
            wire.poll_interval_seconds,
            wire.max_page_size,
        )?;
        validation::summary(&wire.summary)?;
        let token = DeviceToken::parse(&wire.device_token)
            .map_err(|_| ApiFailure::new(ApiIssue::InvalidResponse))?;
        if token.device_id() != wire.device_id {
            return Err(ApiFailure::new(ApiIssue::InvalidResponse));
        }
        Ok(wire)
    }

    async fn get_device(
        &self,
        binding: &Binding,
        cancel: &CancellationToken,
    ) -> Result<DeviceView, ApiFailure> {
        let view: DeviceView = self
            .send(
                binding.origin(),
                "GET",
                PATH_DEVICE,
                binding.token().reveal_for_secure_storage(),
                None,
                DEFAULT_RESPONSE_BYTES,
                cancel,
            )
            .await?;

        validation::version(view.protocol_version)?;
        validation::device_parameters(
            view.user_id,
            view.device_id,
            view.initial_sequence,
            view.poll_interval_seconds,
            view.max_page_size,
        )?;
        validation::require(
            view.user_id == binding.user_id()
                && view.device_id == binding.device_id()
                && validation::valid_text(&view.device_name, 64),
        )?;
        Ok(view)
    }

    async fn get_feed(
        &self,
        binding: &Binding,
        after: i64,
        limit: i32,
        cancel: &CancellationToken,
    ) -> Result<Feed, ApiFailure> {
        if after < 0 || !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(ApiFailure::new(ApiIssue::Validation));
        }
        let path = format!("{PATH_NOTIFICATIONS}?afterSequence={after}&limit={limit}");
        let feed: Feed = self
            .send(
                binding.origin(),
                "GET",
                &path,
                binding.token().reveal_for_secure_storage(),
                None,
                MAX_FEED_RESPONSE_BYTES,
                cancel,
            )
            .await?;
        validation::page(&feed, after, limit)?;
        Ok(feed)
    }

    async fn ack(
        &self,
        binding: &Binding,
        through: i64,
        cancel: &CancellationToken,
    ) -> Result<i64, ApiFailure> {
        if through < 0 {
            return Err(ApiFailure::new(ApiIssue::Validation));
        }
        let body = serde_json::to_vec(&AckRequest {
            received_through_sequence: Seq(through),
        })
        .map_err(|_| ApiFailure::new(ApiIssue::Validation))?;
        if body.len() > MAX_ACK_BODY_BYTES {
            return Err(ApiFailure::new(ApiIssue::Validation));
        }

        let receipt: AckReceipt = self
            .send(
                binding.origin(),
                "POST",
                PATH_ACKS,
                binding.token().reveal_for_secure_storage(),
                Some(body),
                DEFAULT_RESPONSE_BYTES,
                cancel,
            )
            .await?;
        validation::require(receipt.acked_sequence.0 >= through)?;
        Ok(receipt.acked_sequence.0)
    }

    async fn resync(
        &self,
        binding: &Binding,
        cancel: &CancellationToken,
    ) -> Result<ResyncReceipt, ApiFailure> {
        let receipt: ResyncReceipt = self
            .send(
                binding.origin(),
                "POST",
                PATH_RESYNC,
                binding.token().reveal_for_secure_storage(),
                Some(b"{}".to_vec()),
                DEFAULT_RESPONSE_BYTES,
                cancel,
            )
            .await?;
        validation::require(receipt.initial_sequence.0 >= 0)?;
        validation::summary(&receipt.summary)?;
        Ok(receipt)
    }

    async fn revoke(
        &self,
        binding: &Binding,
        cancel: &CancellationToken,
    ) -> Result<(), ApiFailure> {
        let receipt: RevokeReceipt = self
            .send(
                binding.origin(),
                "DELETE",
                PATH_REVOKE_SELF,
                binding.token().reveal_for_secure_storage(),
                None,
                DEFAULT_RESPONSE_BYTES,
                cancel,
            )
            .await?;
        validation::require(receipt.revoked)
    }
}

#[derive(serde::Deserialize)]
struct Envelope<T> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
    code: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AckRequest {
    received_through_sequence: Seq,
}

fn media_type(raw: Option<&str>) -> Option<&str> {
    raw.map(|value| value.split(';').next().unwrap_or("").trim())
}

fn transport_failure(error: TransportError) -> ApiFailure {
    ApiFailure::new(match error {
        TransportError::Certificate => ApiIssue::Certificate,
        TransportError::Offline => ApiIssue::Offline,
        TransportError::InvalidResponse => ApiIssue::InvalidResponse,
    })
}

fn classify_failure(response: &HttpResponse) -> ApiFailure {
    let retry_after = parse_retry_after(response.retry_after.as_deref());
    match response.status {
        401 => return ApiFailure::new(ApiIssue::CredentialInvalid).with_retry_after(retry_after),
        429 => return ApiFailure::new(ApiIssue::RateLimited).with_retry_after(retry_after),
        _ => {}
    }

    let code = if matches!(response.status, 403 | 409 | 410 | 503)
        && media_type(response.content_type.as_deref())
            .is_some_and(|value| value.eq_ignore_ascii_case("application/problem+json"))
        && response.body.len() <= PROBLEM_RESPONSE_BYTES
    {
        extract_code(&response.body)
    } else {
        None
    };

    let issue = match (response.status, code.as_deref()) {
        (403, Some("CSRF_REQUIRED")) => ApiIssue::Csrf,
        (409, Some("NOTIFY_DEVICE_LIMIT")) => ApiIssue::DeviceLimit,
        (409, Some("VERSION_CONFLICT")) => ApiIssue::VersionConflict,
        (409, Some("NOTIFY_STREAM_RESET_REQUIRED")) => ApiIssue::StreamReset,
        (410, Some("NOTIFY_HISTORY_EXPIRED")) => ApiIssue::HistoryExpired,
        (422, _) => ApiIssue::Validation,
        (503, Some("DESKTOP_NOTIFICATIONS_DISABLED")) => ApiIssue::ServerDisabled,
        (500..=599, _) => ApiIssue::ServerUnavailable,
        _ => ApiIssue::InvalidResponse,
    };
    ApiFailure::new(issue).with_retry_after(retry_after)
}

fn extract_code(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value.as_object()?.get("code")?.as_str().map(str::to_owned)
}

fn parse_retry_after(raw: Option<&str>) -> Option<Duration> {
    let raw = raw?.trim();
    if let Ok(seconds) = raw.parse::<u64>() {
        let duration = Duration::from_secs(seconds);
        return (!duration.is_zero()).then_some(duration);
    }
    let date = httpdate::parse_http_date(raw).ok()?;
    let remaining = date.duration_since(std::time::SystemTime::now()).ok()?;
    (!remaining.is_zero()).then_some(remaining)
}
