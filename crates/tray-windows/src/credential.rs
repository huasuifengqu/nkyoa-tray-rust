//! Windows 凭据管理器适配。
//!
//! 与 .NET 版保持相同的目标名与 JSON blob 形态（`{"Version":1,"Origin":…,
//! "UserId":…,"DeviceId":…,"Token":…}`），便于试点期间双向核验。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tray_core::{Binding, ClientError, CredentialStore, DeviceToken, ServerOrigin, StoreError};
use uuid::Uuid;
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
    CredReadW, CredWriteW,
};
use windows::core::{Error, PCWSTR, PWSTR};
use zeroize::Zeroize;

const MAX_BLOB_BYTES: usize = 2560;
const USER_NAME: &str = "NKYOA Tray";

pub struct WindowsCredentialStore {
    target: String,
}

impl WindowsCredentialStore {
    pub fn new() -> Self {
        Self::with_target(crate::brand::CREDENTIAL_TARGET)
    }

    pub fn with_target(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
        }
    }

    fn target_wide(&self) -> Vec<u16> {
        OsStr::new(&self.target)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn store_error() -> ClientError {
        ClientError::Storage(StoreError::new("无法访问 Windows 凭据管理器。"))
    }
}

impl Default for WindowsCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CredentialStore for WindowsCredentialStore {
    async fn read(&self, cancel: &CancellationToken) -> Result<Option<Binding>, ClientError> {
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }

        let target = self.target_wide();
        let mut pointer: *mut CREDENTIALW = ptr::null_mut();
        if let Err(error) = unsafe {
            CredReadW(
                PCWSTR(target.as_ptr()),
                CRED_TYPE_GENERIC,
                None,
                &mut pointer,
            )
        } {
            return if win32_code(&error) == ERROR_NOT_FOUND.0 {
                Ok(None)
            } else {
                Err(Self::store_error())
            };
        }
        if pointer.is_null() {
            return Err(Self::store_error());
        }

        let (size, blob) = unsafe {
            let credential = &*pointer;
            (
                credential.CredentialBlobSize as usize,
                credential.CredentialBlob,
            )
        };
        if size == 0 || size > MAX_BLOB_BYTES || blob.is_null() {
            unsafe { CredFree(pointer as *const core::ffi::c_void) };
            return Err(Self::store_error());
        }

        let mut bytes = unsafe { std::slice::from_raw_parts(blob, size).to_vec() };
        unsafe { CredFree(pointer as *const core::ffi::c_void) };
        let parsed = parse_binding(&bytes);
        bytes.zeroize();
        parsed.map(Some)
    }

    async fn save(&self, binding: &Binding, cancel: &CancellationToken) -> Result<(), ClientError> {
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }

        let mut stored = StoredBinding {
            version: 1,
            origin: binding.origin().as_str().to_owned(),
            user_id: binding.user_id(),
            device_id: binding.device_id(),
            token: binding.token().reveal_for_secure_storage().to_owned(),
        };
        let mut bytes = serde_json::to_vec(&stored).map_err(|_| Self::store_error())?;
        stored.token.zeroize();
        if bytes.len() > MAX_BLOB_BYTES {
            bytes.zeroize();
            return Err(Self::store_error());
        }

        let mut blob = bytes.clone();
        let mut target = self.target_wide();
        let mut user: Vec<u16> = USER_NAME.encode_utf16().chain(std::iter::once(0)).collect();
        let credential = CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            Comment: PWSTR(ptr::null_mut()),
            LastWritten: Default::default(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: ptr::null_mut(),
            TargetAlias: PWSTR(ptr::null_mut()),
            UserName: PWSTR(user.as_mut_ptr()),
        };
        let result = unsafe { CredWriteW(&credential, 0) };
        blob.zeroize();
        bytes.zeroize();
        result.map_err(|_| Self::store_error())
    }

    async fn remove(&self, cancel: &CancellationToken) -> Result<(), ClientError> {
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        let target = self.target_wide();
        match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(error) if win32_code(&error) == ERROR_NOT_FOUND.0 => Ok(()),
            Err(_) => Err(Self::store_error()),
        }
    }
}

// .NET 版用默认 System.Text.Json 选项写入，属性名为 PascalCase；读取时大小写敏感，
// 因此这里必须保持 `Version/Origin/UserId/DeviceId/Token` 才能双向兼容。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct StoredBinding {
    version: i32,
    origin: String,
    user_id: Uuid,
    device_id: Uuid,
    token: String,
}

fn parse_binding(bytes: &[u8]) -> Result<Binding, ClientError> {
    let invalid = || ClientError::Storage(StoreError::new("无法解析系统凭据记录。"));
    let mut stored: StoredBinding = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if stored.version != 1 {
        stored.token.zeroize();
        return Err(invalid());
    }
    let origin = match ServerOrigin::parse(&stored.origin) {
        Ok(origin) if origin.as_str() == stored.origin => origin,
        _ => {
            stored.token.zeroize();
            return Err(invalid());
        }
    };
    let token = match DeviceToken::parse(&stored.token) {
        Ok(token) => token,
        Err(_) => {
            stored.token.zeroize();
            return Err(invalid());
        }
    };
    stored.token.zeroize();
    Binding::new(origin, stored.user_id, stored.device_id, token).map_err(|_| invalid())
}

fn win32_code(error: &Error) -> u32 {
    error.code().0 as u32 & 0xFFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_binding(device: Uuid) -> Binding {
        Binding::new(
            ServerOrigin::parse("oa.example.test").unwrap(),
            Uuid::new_v4(),
            device,
            DeviceToken::parse(&format!(
                "nky-device-v1.{}.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                device.as_simple()
            ))
            .unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn credential_manager_persists_replaces_and_removes_the_unique_test_entry() {
        let target = format!("NKYOA.Tray.Test/{}", Uuid::new_v4().as_simple());
        let store = WindowsCredentialStore::with_target(target.clone());
        let cancel = CancellationToken::new();
        let first = test_binding(Uuid::new_v4());
        let second = test_binding(Uuid::new_v4());

        assert!(store.read(&cancel).await.unwrap().is_none());
        store.save(&first, &cancel).await.unwrap();
        let reopened = WindowsCredentialStore::with_target(target.clone())
            .read(&cancel)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reopened.profile_id(), first.profile_id());
        assert_eq!(
            reopened.token().reveal_for_secure_storage(),
            first.token().reveal_for_secure_storage()
        );

        store.save(&second, &cancel).await.unwrap();
        assert_eq!(
            store.read(&cancel).await.unwrap().unwrap().profile_id(),
            second.profile_id()
        );

        store.remove(&cancel).await.unwrap();
        assert!(store.read(&cancel).await.unwrap().is_none());
        store.remove(&cancel).await.unwrap();
    }
}
