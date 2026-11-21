//! 当前用户单实例与本地激活转发。
//!
//! 实例锁使用与 .NET 版一致的语义：以共享模式 0 打开 `instance.lock`，
//! 两个版本互斥。命名管道按用户配置目录派生，只接受长度受限的激活帧。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE,
    GetLastError, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE, OPEN_ALWAYS,
};
use windows::core::PCWSTR;

use crate::activation::Activation;

const MAX_MESSAGE_BYTES: usize = 69;

pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    /// 获取当前用户实例锁；已有实例持有时返回 `Ok(None)`。
    pub fn acquire(user_directory: &Path) -> std::io::Result<Option<Self>> {
        std::fs::create_dir_all(user_directory)?;
        let lease_path = user_directory.join("instance.lock");
        let wide: Vec<u16> = OsStr::new(&lease_path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let result = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };

        match result {
            Ok(handle) => Ok(Some(Self { handle })),
            Err(_) => {
                let error = unsafe { GetLastError() };
                if error == ERROR_SHARING_VIOLATION || error == ERROR_LOCK_VIOLATION {
                    Ok(None)
                } else {
                    Err(std::io::Error::from_raw_os_error(error.0 as i32))
                }
            }
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// 按当前用户配置目录派生管道名，隔离同机其他用户。
pub fn pipe_name() -> String {
    pipe_name_for(
        &std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default(),
    )
}

pub fn pipe_name_for(seed: &str) -> String {
    let digest = Sha256::digest(seed.to_lowercase().as_bytes());
    let mut suffix = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(suffix, "{byte:02x}");
    }
    format!(r"\\.\pipe\Nkyoa.Tray.Rust.{suffix}")
}

/// 主实例监听本地激活请求；每帧最多 69 字节，2 秒内必须完成。
pub fn spawn_listener(
    runtime: &tokio::runtime::Handle,
    pipe_name: String,
    dispatch: impl Fn(Activation) + Send + Sync + 'static,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    runtime.spawn(async move {
        loop {
            if cancel.is_cancelled() {
                return;
            }
            let mut server = match ServerOptions::new()
                .reject_remote_clients(true)
                .create(&pipe_name)
            {
                Ok(server) => server,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            };

            let connected = tokio::select! {
                result = server.connect() => result.is_ok(),
                _ = cancel.cancelled() => return,
            };
            if !connected {
                continue;
            }

            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                handle_connection(&mut server, &dispatch),
            )
            .await;
        }
    })
}

async fn handle_connection(
    server: &mut NamedPipeServer,
    dispatch: &(impl Fn(Activation) + Send + Sync),
) {
    let mut length = [0u8; 1];
    if server.read_exact(&mut length).await.is_err() {
        return;
    }
    if length[0] == 0 || length[0] as usize > MAX_MESSAGE_BYTES {
        return;
    }
    let mut payload = vec![0u8; length[0] as usize];
    if server.read_exact(&mut payload).await.is_err() {
        return;
    }
    let Some(activation) = std::str::from_utf8(&payload)
        .ok()
        .and_then(Activation::from_message)
    else {
        return;
    };
    dispatch(activation);
    let _ = server.write_all(&[1]).await;
}

/// 次实例把激活请求转发给主实例；超时 3 秒。
pub async fn forward(pipe_name: &str, activation: &Activation) -> bool {
    let message = activation.to_message();
    let bytes = message.as_bytes();
    if bytes.len() > MAX_MESSAGE_BYTES || Activation::from_message(&message).is_none() {
        return false;
    }

    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let mut client = ClientOptions::new().open(pipe_name)?;
        let mut frame = Vec::with_capacity(bytes.len() + 1);
        frame.push(bytes.len() as u8);
        frame.extend_from_slice(bytes);
        client.write_all(&frame).await?;
        let mut response = [0u8; 1];
        client.read_exact(&mut response).await?;
        Ok::<bool, std::io::Error>(response[0] == 1)
    })
    .await;

    matches!(result, Ok(Ok(true)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_is_exclusive_and_released_on_drop() {
        let directory = tempfile::tempdir().unwrap();
        let first = SingleInstance::acquire(directory.path()).unwrap();
        assert!(first.is_some());
        assert!(SingleInstance::acquire(directory.path()).unwrap().is_none());
        drop(first);
        assert!(SingleInstance::acquire(directory.path()).unwrap().is_some());
    }

    #[tokio::test]
    async fn pipe_forwards_valid_activations_and_rejects_oversize_frames() {
        let pipe = pipe_name_for(&format!("test-{}", uuid::Uuid::new_v4()));
        let cancel = CancellationToken::new();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let listener = spawn_listener(
            &tokio::runtime::Handle::current(),
            pipe.clone(),
            move |activation| {
                let _ = sender.send(activation);
            },
            cancel.clone(),
        );

        tokio::time::sleep(Duration::from_millis(100)).await;
        let activation = Activation {
            profile_id: Some("b".repeat(64)),
        };
        assert!(forward(&pipe, &activation).await);
        assert_eq!(receiver.recv().await.unwrap(), activation);

        assert!(forward(&pipe, &Activation::SHOW_SETTINGS).await);
        assert_eq!(receiver.recv().await.unwrap(), Activation::SHOW_SETTINGS);

        if let Ok(mut bad) = ClientOptions::new().open(&pipe) {
            let _ = bad.write_all(&[70u8]).await;
            let mut response = [0u8; 1];
            let _ = bad.read_exact(&mut response).await;
        }
        assert!(receiver.try_recv().is_err(), "超长帧不应触发激活分发");

        cancel.cancel();
        let _ = listener.await;
    }
}
