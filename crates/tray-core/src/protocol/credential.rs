//! 通知凭证与设备绑定。
//!
//! 凭证格式：`nky-<用途>-v1.<32 位十六进制公开 ID>.<43 位 base64url 秘密>`。
//! `Debug`/`Display` 一律脱敏，日志与诊断不会带出秘密。

use std::fmt;

use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroize;

use super::error::CredentialError;
use super::origin::ServerOrigin;
use super::{
    AUTHORIZATION_SCHEME, CREDENTIAL_FIELD_COUNT, DEVICE_CREDENTIAL_PREFIX,
    MAX_AUTHORIZATION_LENGTH, PAIRING_CREDENTIAL_PREFIX,
};

const BEARER_PREFIX_LENGTH: usize = AUTHORIZATION_SCHEME.len() + 1;
const SECRET_LENGTH: usize = 43;
const PUBLIC_ID_LENGTH: usize = 32;

/// 一次性配对码；只用于注册请求的 Authorization。
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode(String);

impl PairingCode {
    pub fn parse(input: &str) -> Result<Self, CredentialError> {
        let value = input.trim();
        validate_credential(value, PAIRING_CREDENTIAL_PREFIX)?;
        Ok(Self(value.to_owned()))
    }

    pub fn value(&self) -> &str {
        &self.0
    }
}

impl Drop for PairingCode {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted pairing code]")
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted pairing code]")
    }
}

/// 设备令牌；公开 ID 必须与注册回执的 deviceId 一致。
#[derive(Clone)]
pub struct DeviceToken {
    value: String,
    device_id: Uuid,
}

impl DeviceToken {
    pub fn parse(value: &str) -> Result<Self, CredentialError> {
        let public_id = validate_credential(value, DEVICE_CREDENTIAL_PREFIX)?;
        Ok(Self {
            value: value.to_owned(),
            device_id: public_id,
        })
    }

    pub fn device_id(&self) -> Uuid {
        self.device_id
    }

    /// 仅供操作系统凭据存储适配器写入密钥库。
    pub fn reveal_for_secure_storage(&self) -> &str {
        &self.value
    }
}

impl Drop for DeviceToken {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

impl PartialEq for DeviceToken {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && self.device_id == other.device_id
    }
}

impl Eq for DeviceToken {}

impl fmt::Debug for DeviceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted device token]")
    }
}

impl fmt::Display for DeviceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted device token]")
    }
}

/// 唯一活跃绑定；替换必须原子写入。
#[derive(Clone, PartialEq, Eq)]
pub struct Binding {
    origin: ServerOrigin,
    user_id: Uuid,
    device_id: Uuid,
    token: DeviceToken,
    profile_id: String,
}

impl Binding {
    pub fn new(
        origin: ServerOrigin,
        user_id: Uuid,
        device_id: Uuid,
        token: DeviceToken,
    ) -> Result<Self, CredentialError> {
        if user_id.is_nil() || device_id.is_nil() || token.device_id != device_id {
            return Err(CredentialError);
        }

        let profile_id = profile_id(&origin, user_id, device_id);
        Ok(Self {
            origin,
            user_id,
            device_id,
            token,
            profile_id,
        })
    }

    pub fn origin(&self) -> &ServerOrigin {
        &self.origin
    }

    pub fn user_id(&self) -> Uuid {
        self.user_id
    }

    pub fn device_id(&self) -> Uuid {
        self.device_id
    }

    pub fn token(&self) -> &DeviceToken {
        &self.token
    }

    /// 配置标识：站点、用户与设备 GUID 的 SHA-256（小写十六进制）。
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }
}

impl fmt::Debug for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Binding")
            .field("origin", &self.origin)
            .field("user_id", &self.user_id)
            .field("device_id", &self.device_id)
            .field("profile_id", &self.profile_id)
            .finish_non_exhaustive()
    }
}

fn profile_id(origin: &ServerOrigin, user_id: Uuid, device_id: Uuid) -> String {
    let mut hasher = Sha256::new();
    hasher.update(origin.as_str().as_bytes());
    hasher.update(b"\n");
    hasher.update(user_id.as_simple().to_string().as_bytes());
    hasher.update(b"\n");
    hasher.update(device_id.as_simple().to_string().as_bytes());
    let digest = hasher.finalize();
    let mut text = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use fmt::Write as _;
        // 写入十六进制不会失败。
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn validate_credential(value: &str, prefix: &str) -> Result<Uuid, CredentialError> {
    if value.len() > MAX_AUTHORIZATION_LENGTH - BEARER_PREFIX_LENGTH {
        return Err(CredentialError);
    }
    if !value.is_ascii() {
        return Err(CredentialError);
    }

    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() != CREDENTIAL_FIELD_COUNT || parts[0] != prefix {
        return Err(CredentialError);
    }

    let public_id = parts[1];
    if public_id.len() != PUBLIC_ID_LENGTH
        || !public_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CredentialError);
    }
    let id = Uuid::parse_str(public_id).map_err(|_| CredentialError)?;
    if id.is_nil() {
        return Err(CredentialError);
    }

    let secret = parts[2];
    if secret.len() != SECRET_LENGTH
        || !secret
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CredentialError);
    }

    Ok(id)
}
