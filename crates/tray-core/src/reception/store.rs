//! 按配置隔离的 JSON 接收状态存储。
//!
//! 写入使用同目录临时文件 + 落盘 + 原子替换；损坏文件只保留固定诊断标记，
//! 不复制原始内容。安装标识并发首次创建时以硬链接保证只有一个赢家。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::protocol::Binding;
use crate::protocol::error::{ApiFailure, ApiIssue, StoreError};

use super::contracts::{ClientError, ClientStateStore};
use super::state::{ClientState, StateRead, persisted_item, validate};

const MAX_STATE_BYTES: u64 = 1024 * 1024;
const INSTALLATION_ID_BYTES: usize = 36;
const CORRUPT_MARKER: &[u8] = b"{\"reason\":\"invalid-state\"}\n";

pub struct JsonClientStateStore {
    root: PathBuf,
}

impl JsonClientStateStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn state_path(&self, binding: &Binding) -> PathBuf {
        self.root.join(binding.profile_id()).join("state.json")
    }

    fn mark_corrupt(
        &self,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<StateRead, ClientError> {
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        let marker = PathBuf::from(format!("{}.corrupt.json", path.display()));
        write_atomic(&marker, CORRUPT_MARKER).map_err(ClientError::Storage)?;
        Ok(StateRead {
            state: None,
            corrupt: true,
        })
    }
}

#[async_trait]
impl ClientStateStore for JsonClientStateStore {
    async fn installation_id(&self, cancel: &CancellationToken) -> Result<Uuid, ClientError> {
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        let path = self.root.join("installation-id");
        if let Some(id) = read_installation_id(&path).map_err(ClientError::Storage)? {
            return Ok(id);
        }

        let id = Uuid::new_v4();
        let bytes = id.hyphenated().to_string().into_bytes();
        write_new_atomic(&path, &bytes).map_err(ClientError::Storage)?;
        read_installation_id(&path)
            .map_err(ClientError::Storage)?
            .ok_or_else(|| ClientError::Storage(StoreError::new("安装标识写入失败")))
    }

    async fn read(
        &self,
        binding: &Binding,
        cancel: &CancellationToken,
    ) -> Result<StateRead, ClientError> {
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        let path = self.state_path(binding);
        let bytes = match open_state(&path) {
            StateFile::Missing => {
                return Ok(StateRead {
                    state: None,
                    corrupt: false,
                });
            }
            StateFile::TooLarge => return self.mark_corrupt(&path, cancel),
            StateFile::Io(error) => return Err(ClientError::Storage(error)),
            StateFile::Bytes(bytes) => bytes,
        };

        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }

        match serde_json::from_slice::<ClientState>(&bytes) {
            Ok(state) => {
                if state.version > ClientState::CURRENT_VERSION {
                    return Err(ClientError::Api(ApiFailure::new(
                        ApiIssue::UnsupportedVersion,
                    )));
                }
                match validate(&state) {
                    Ok(()) => Ok(StateRead {
                        state: Some(state),
                        corrupt: false,
                    }),
                    Err(_) => self.mark_corrupt(&path, cancel),
                }
            }
            Err(_) => {
                // 结构可能与未来版本不同；宽松解析只用于判断版本，不用于读取状态。
                let future = serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .and_then(|value| value.get("version").and_then(|v| v.as_i64()))
                    .is_some_and(|version| version > i64::from(ClientState::CURRENT_VERSION));
                if future {
                    return Err(ClientError::Api(ApiFailure::new(
                        ApiIssue::UnsupportedVersion,
                    )));
                }
                self.mark_corrupt(&path, cancel)
            }
        }
    }

    async fn save(
        &self,
        binding: &Binding,
        state: &ClientState,
        cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        validate(state).map_err(ClientError::Api)?;
        // 防御性收口：即使调用方把预览文案放进了内存状态，磁盘上也只出现通用提示。
        // 标题与流转意见只允许存在于内存及系统通知显示路径（方案 §4）。
        let persisted = persisted_copy(state);
        let bytes = serde_json::to_vec(&persisted).map_err(|error| {
            ClientError::Storage(StoreError::new(format!("状态序列化失败：{error}")))
        })?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err(ClientError::Storage(StoreError::new(
                "本机通知状态超过大小限制。",
            )));
        }
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        write_atomic(&self.state_path(binding), &bytes).map_err(ClientError::Storage)
    }
}

/// 落盘副本：待处理页条目的标题/正文统一替换为通用提示。
fn persisted_copy(state: &ClientState) -> ClientState {
    let mut copy = state.clone();
    if let Some(page) = &mut copy.pending {
        for item in &mut page.items {
            *item = persisted_item(item);
        }
    }
    copy
}

enum StateFile {
    Missing,
    TooLarge,
    Io(StoreError),
    Bytes(Vec<u8>),
}

fn open_state(path: &Path) -> StateFile {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return StateFile::Missing,
        Err(error) => {
            return StateFile::Io(StoreError::new(format!("无法读取状态文件：{error}")));
        }
    };
    let length = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            return StateFile::Io(StoreError::new(format!("无法读取状态文件：{error}")));
        }
    };
    if length > MAX_STATE_BYTES {
        return StateFile::TooLarge;
    }

    let mut bytes = Vec::with_capacity(length as usize + 1);
    match file.take(MAX_STATE_BYTES + 1).read_to_end(&mut bytes) {
        Ok(_) if bytes.len() as u64 <= MAX_STATE_BYTES => StateFile::Bytes(bytes),
        Ok(_) => StateFile::TooLarge,
        Err(error) => StateFile::Io(StoreError::new(format!("无法读取状态文件：{error}"))),
    }
}

fn read_installation_id(path: &Path) -> Result<Option<Uuid>, StoreError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(StoreError::new(format!("无法读取安装标识：{error}")));
        }
    };
    let mut bytes = Vec::with_capacity(INSTALLATION_ID_BYTES + 1);
    file.take(INSTALLATION_ID_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| StoreError::new(format!("无法读取安装标识：{error}")))?;
    if bytes.len() != INSTALLATION_ID_BYTES {
        return Err(StoreError::new("安装标识已损坏。"));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| StoreError::new("安装标识已损坏。"))?;
    let id = Uuid::parse_str(text).map_err(|_| StoreError::new("安装标识已损坏。"))?;
    if id.is_nil() {
        return Err(StoreError::new("安装标识已损坏。"));
    }
    Ok(Some(id))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let directory = path
        .parent()
        .ok_or_else(|| StoreError::new("状态目录无效。"))?;
    fs::create_dir_all(directory)
        .map_err(|error| StoreError::new(format!("无法创建状态目录：{error}")))?;
    let temporary = directory.join(format!(".{}.tmp", Uuid::new_v4().simple()));

    let result = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();

    let _ = fs::remove_file(&temporary);
    result.map_err(|error| StoreError::new(format!("无法写入状态文件：{error}")))
}

fn write_new_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let directory = path
        .parent()
        .ok_or_else(|| StoreError::new("状态目录无效。"))?;
    fs::create_dir_all(directory)
        .map_err(|error| StoreError::new(format!("无法创建状态目录：{error}")))?;
    let temporary = directory.join(format!(".{}.tmp", Uuid::new_v4().simple()));

    let result = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        match fs::hard_link(&temporary, path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(_) => match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    file.write_all(bytes)?;
                    file.sync_all()
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(error),
            },
        }
    })();

    let _ = fs::remove_file(&temporary);
    result.map_err(|error| StoreError::new(format!("无法写入安装标识：{error}")))
}
