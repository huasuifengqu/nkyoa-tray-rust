//! HTTP 传输抽象。
//!
//! 生产实现使用系统 TLS（Windows Schannel）且禁止自动重定向；测试可注入
//! 假传输，重放黄金样本并检查请求方法与头字段。

use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::REQUEST_TIMEOUT;

#[derive(Clone)]
pub struct HttpRequest {
    pub method: String,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub max_response_bytes: usize,
}

#[derive(Clone)]
pub struct HttpResponse {
    pub status: u16,
    /// 原始 `Content-Type`（含参数）；由 API 层取 media type。
    pub content_type: Option<String>,
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    /// TLS 证书/安全连接失败。
    Certificate,
    /// 断网、DNS、连接拒绝、超时或读取中断。
    Offline,
    /// 响应超过上限或请求构造失败。
    InvalidResponse,
}

#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn send(
        &self,
        request: HttpRequest,
        cancel: &CancellationToken,
    ) -> Result<HttpResponse, TransportError>;
}

/// 基于 reqwest 的生产传输：系统 TLS 信任、不跟随重定向、15 秒总超时。
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .pool_idle_timeout(Duration::from_secs(300))
            .build()
            .expect("TLS 后端初始化失败");
        Self { client }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn send(
        &self,
        request: HttpRequest,
        cancel: &CancellationToken,
    ) -> Result<HttpResponse, TransportError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| TransportError::InvalidResponse)?;
        let mut builder = self.client.request(method, request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(TransportError::Offline),
            result = builder.send() => result.map_err(|error| classify_send_error(&error))?,
        };

        if response
            .content_length()
            .is_some_and(|length| length > request.max_response_bytes as u64)
        {
            return Err(TransportError::InvalidResponse);
        }

        let status = response.status().as_u16();
        let content_type = header_text(&response, reqwest::header::CONTENT_TYPE);
        let retry_after = header_text(&response, reqwest::header::RETRY_AFTER);

        let mut body = Vec::new();
        let mut response = response;
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => return Err(TransportError::Offline),
                chunk = response.chunk() => chunk.map_err(classify_body_error)?,
            };
            let Some(chunk) = chunk else { break };
            if body.len() + chunk.len() > request.max_response_bytes {
                return Err(TransportError::InvalidResponse);
            }
            body.extend_from_slice(&chunk);
        }

        Ok(HttpResponse {
            status,
            content_type,
            retry_after,
            body,
        })
    }
}

fn header_text(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn classify_send_error(error: &reqwest::Error) -> TransportError {
    if error.is_builder() {
        return TransportError::InvalidResponse;
    }
    // 只有能确认是 TLS 证书失败才不可重试；其余连接/超时/解析失败都按离线退避。
    if error.is_connect() && tls_failure_in_sources(error) {
        return TransportError::Certificate;
    }
    TransportError::Offline
}

fn classify_body_error(_error: reqwest::Error) -> TransportError {
    // 已收到响应头后的读取中断（连接重置、代理断开、超时）属于传输层抖动，
    // 与 .NET 的 IOException 一致按可重试离线处理；大小上限由本模块自行判定。
    TransportError::Offline
}

/// 只检查顶层错误之下的 source 链。
///
/// reqwest 顶层 `Display` 会包含完整 URL，若在其中搜 "tls"/"ssl" 会把普通
/// 连接失败误判为证书错误（例如站点域名含 ssl）；证书判定因此只认
/// `native_tls::Error` 类型与 source 文本中的 "certificate"。
fn tls_failure_in_sources(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut source = error.source();
    while let Some(current) = source {
        if current.downcast_ref::<native_tls::Error>().is_some() {
            return true;
        }
        if current
            .to_string()
            .to_ascii_lowercase()
            .contains("certificate")
        {
            return true;
        }
        source = current.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    #[derive(Debug)]
    struct Fake {
        message: &'static str,
        source: Option<Box<Fake>>,
    }

    impl fmt::Display for Fake {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.message)
        }
    }

    impl std::error::Error for Fake {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.source
                .as_deref()
                .map(|error| error as &(dyn std::error::Error + 'static))
        }
    }

    #[test]
    fn hostname_containing_ssl_does_not_mean_certificate_failure() {
        let outer = Fake {
            message: "error sending request for url (https://ssl.oa.example.cn/api)",
            source: Some(Box::new(Fake {
                message: "tcp connect error: connection refused",
                source: None,
            })),
        };
        assert!(!tls_failure_in_sources(&outer));
    }

    #[test]
    fn certificate_text_in_a_source_is_detected() {
        let outer = Fake {
            message: "error sending request for url (https://oa.example.cn/)",
            source: Some(Box::new(Fake {
                message: "certificate verify failed",
                source: None,
            })),
        };
        assert!(tls_failure_in_sources(&outer));
    }
}
