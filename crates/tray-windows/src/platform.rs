//! Windows 当前用户设置：自启动、暂停偏好与打开 OA。

use std::fmt;
use std::path::{Path, PathBuf};

use tray_core::ServerOrigin;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SW_SHOWNORMAL};
use windows::core::{HSTRING, PCWSTR, w};
use windows_registry::CURRENT_USER;

use crate::brand;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const PREFERENCES_KEY: &str = r"Software\NKYOA\Tray.Rust";
const PAUSE_VALUE: &str = "PausePopups";

#[derive(Debug, Clone)]
pub struct PlatformError(pub String);

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PlatformError {}

pub struct WindowsPlatform {
    executable: PathBuf,
}

impl WindowsPlatform {
    pub fn new(executable: PathBuf) -> Self {
        Self { executable }
    }

    /// 与 .NET 版一致：存在 `Shell_TrayWnd` 才认为系统托盘可用。
    pub fn tray_available(&self) -> bool {
        unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).is_ok() }
    }

    pub fn is_autostart_enabled(&self) -> bool {
        let Ok(key) = CURRENT_USER.open(RUN_KEY) else {
            return false;
        };
        matches!(key.get_string(brand::AUTOSTART_VALUE), Ok(value) if value == self.launch_command())
    }

    pub fn set_autostart(&self, enabled: bool) -> Result<(), PlatformError> {
        let key = CURRENT_USER
            .create(RUN_KEY)
            .map_err(|error| PlatformError(format!("无法打开自启动注册表：{error}")))?;
        if enabled {
            key.set_string(brand::AUTOSTART_VALUE, self.launch_command())
                .map_err(|error| PlatformError(format!("无法写入自启动注册表：{error}")))
        } else {
            if key.get_string(brand::AUTOSTART_VALUE).is_ok() {
                key.remove_value(brand::AUTOSTART_VALUE)
                    .map_err(|error| PlatformError(format!("无法删除自启动注册表：{error}")))?;
            }
            Ok(())
        }
    }

    pub fn are_popups_paused(&self) -> bool {
        let Ok(key) = CURRENT_USER.open(PREFERENCES_KEY) else {
            return false;
        };
        matches!(key.get_u32(PAUSE_VALUE), Ok(1))
    }

    pub fn save_popup_pause(&self, paused: bool) -> Result<(), PlatformError> {
        CURRENT_USER
            .create(PREFERENCES_KEY)
            .and_then(|key| key.set_u32(PAUSE_VALUE, u32::from(paused)))
            .map_err(|error| PlatformError(format!("无法保存弹窗设置：{error}")))
    }

    pub fn clear_popup_pause(&self) {
        if let Ok(key) = CURRENT_USER.open(PREFERENCES_KEY)
            && key.get_u32(PAUSE_VALUE).is_ok()
        {
            let _ = key.remove_value(PAUSE_VALUE);
        }
    }

    pub fn open_home(&self, origin: &ServerOrigin) -> Result<(), PlatformError> {
        let url = HSTRING::from(origin.as_str());
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                &url,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            Err(PlatformError("无法启动默认浏览器。".to_owned()))
        } else {
            Ok(())
        }
    }

    pub fn launch_command(&self) -> String {
        Self::launch_command_for(&self.executable)
    }

    pub fn launch_command_for(executable: &Path) -> String {
        format!("\"{}\" --background", executable.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_argument_quotes_the_executable_and_only_adds_background() {
        let executable = r"C:\Program Files\nkyoa\nkyoa-tray.exe";
        let expected = format!("\"{}\" --background", executable);
        assert_eq!(
            WindowsPlatform::launch_command_for(Path::new(executable)),
            expected
        );
    }
}
