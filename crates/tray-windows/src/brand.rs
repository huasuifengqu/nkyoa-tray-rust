//! 试点期品牌与路径常量。
//!
//! 按方案 §6，试点与 .NET 版并行使用独立的安装目录、AUMID、激活协议、
//! 自启动值与凭据目标；正式切换时只需修改本文件。

use std::path::PathBuf;

/// 开始菜单快捷方式与 Toast 使用的 AppUserModelID。
pub const AUMID: &str = "NKYOA.Tray.Rust.DesktopNotifications";
/// 受控协议激活头（与 .NET 的 nkyoa-notify 区分）。
pub const PROTOCOL_SCHEME: &str = "nkyoa-notify-rust";
pub const PROTOCOL_PREFIX: &str = "nkyoa-notify-rust://open/";
/// Windows 凭据管理器目标名。
pub const CREDENTIAL_TARGET: &str = "NKYOA.Tray.Rust/DesktopNotifications.v1";
/// HKCU Run 值名。
pub const AUTOSTART_VALUE: &str = "NKYOA.Tray.Rust";
/// 显示名称（开始菜单与通知中心）。
pub const DISPLAY_NAME: &str = "OA通知";
pub const SHORTCUT_NAME: &str = "OA通知.lnk";

pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Nkyoa")
        .join("Tray.Rust")
}

pub fn state_root() -> PathBuf {
    data_dir().join("state")
}

pub fn diagnostics_dir() -> PathBuf {
    data_dir().join("diagnostics")
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}
