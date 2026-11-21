//! 服务器站点地址。
//!
//! 只接受 HTTPS 根路径（可省略 `https://` 前缀），规范化后始终带尾部 `/`。
//! 这是配置标识 `profileId` 的输入之一，必须与 .NET 版 `Uri.AbsoluteUri`
//! 的输出逐字一致。

use std::fmt;

use url::Url;

use super::error::OriginError;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ServerOrigin {
    url: Url,
}

impl ServerOrigin {
    pub fn parse(input: &str) -> Result<Self, OriginError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(OriginError);
        }

        let candidate = if trimmed.contains("://") {
            trimmed.to_owned()
        } else {
            format!("https://{trimmed}")
        };

        // WHATWG URL 会剥离制表/换行等控制字符，必须先显式拒绝。
        if candidate
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(OriginError);
        }
        if candidate.contains('\\') || candidate.contains('%') {
            return Err(OriginError);
        }

        let scheme_end = candidate.find("://").ok_or(OriginError)?;
        if !candidate[..scheme_end].eq_ignore_ascii_case("https") {
            return Err(OriginError);
        }
        let after_scheme = &candidate[scheme_end + 3..];
        let authority = match after_scheme.find('/') {
            // 只允许 root 的单个尾部斜杠。
            Some(index) if index == after_scheme.len() - 1 => &after_scheme[..index],
            Some(_) => return Err(OriginError),
            None => after_scheme,
        };
        if authority.is_empty()
            || authority
                .chars()
                .any(|c| matches!(c, '@' | '?' | '#' | '\\' | '%'))
        {
            return Err(OriginError);
        }

        let url = Url::parse(&candidate).map_err(|_| OriginError)?;
        if url.scheme() != "https"
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port() == Some(0)
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(OriginError);
        }

        Ok(Self { url })
    }

    /// 规范化后的绝对地址（含尾部 `/`）；用于 `profileId` 与凭据存储。
    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    pub fn url(&self) -> &Url {
        &self.url
    }
}

impl fmt::Debug for ServerOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ServerOrigin").field(&self.as_str()).finish()
    }
}

impl fmt::Display for ServerOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
