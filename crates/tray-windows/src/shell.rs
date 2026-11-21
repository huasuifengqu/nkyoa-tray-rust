//! 开始菜单快捷方式、AUMID 与受控协议注册。
//!
//! 安装命令只写当前用户注册表（HKCU），不要求管理员权限。快捷方式携带
//! `System.AppUserModel.ID` 与协议激活 CLSID，Toast 才能稳定归属本应用。

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::{
    InitPropVariantFromCLSID, PROPVARIANT, PROPVARIANT_0_0,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, IPersistFile,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, SHStrDupW,
    SetCurrentProcessExplicitAppUserModelID, ShellLink,
};
use windows::core::{GUID, HSTRING, Interface as _, PCWSTR};

use crate::brand;

const APP_USER_MODEL_FORMAT: GUID = GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3);
const PROPERTY_AUMID: u32 = 5;
const PROPERTY_TOAST_ACTIVATOR: u32 = 26;
/// Shell 通过该固定 CLSID 处理协议激活的桌面通知。
const PROTOCOL_TOAST_ACTIVATOR: GUID = GUID::from_u128(0xE82D235E_46BC_4018_A883_65AEBDE7E1BA);

#[derive(Debug, Clone)]
pub struct ShellError(pub String);

impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ShellError {}

fn shell_error(context: &str, error: impl fmt::Display) -> ShellError {
    ShellError(format!("{context}：{error}"))
}

/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs\OA通知.lnk`
pub fn start_menu_link() -> Result<PathBuf, ShellError> {
    let pointer = unsafe { SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None) }
        .map_err(|error| shell_error("无法定位开始菜单", error))?;
    let folder = unsafe { pointer.to_string() }
        .map_err(|_| ShellError("开始菜单路径包含无效字符。".to_owned()))?;
    unsafe {
        CoTaskMemFree(Some(pointer.0 as *const core::ffi::c_void));
    }
    Ok(PathBuf::from(folder).join(brand::SHORTCUT_NAME))
}

pub fn set_process_app_id() -> Result<(), ShellError> {
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(brand::AUMID)) }
        .map_err(|error| shell_error("无法设置进程 AUMID", error))
}

pub fn install(executable: &Path) -> Result<(), ShellError> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let link = start_menu_link()?;
    create_shortcut(executable, &link)?;

    let app_key = format!(r"Software\Classes\AppUserModelId\{}", brand::AUMID);
    windows_registry::CURRENT_USER
        .create(&app_key)
        .and_then(|key| key.set_string("DisplayName", brand::DISPLAY_NAME))
        .map_err(|error| shell_error("无法注册通知应用标识", error))?;

    let protocol_key = format!(r"Software\Classes\{}", brand::PROTOCOL_SCHEME);
    let protocol = windows_registry::CURRENT_USER
        .create(&protocol_key)
        .map_err(|error| shell_error("无法注册通知协议", error))?;
    protocol
        .set_string("", format!("URL:{}", brand::DISPLAY_NAME))
        .and_then(|()| protocol.set_string("URL Protocol", ""))
        .map_err(|error| shell_error("无法写入通知协议", error))?;

    let command_key = format!(r"{protocol_key}\shell\open\command");
    let command = windows_registry::CURRENT_USER
        .create(&command_key)
        .map_err(|error| shell_error("无法注册通知协议命令", error))?;
    command
        .set_string("", format!("\"{}\" \"%1\"", executable.display()))
        .map_err(|error| shell_error("无法写入通知协议命令", error))
}

pub fn remove() -> Result<(), ShellError> {
    if let Ok(link) = start_menu_link() {
        match fs::remove_file(&link) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(shell_error("无法删除开始菜单快捷方式", error)),
        }
    }

    for key in [
        format!(r"Software\Classes\AppUserModelId\{}", brand::AUMID),
        format!(r"Software\Classes\{}", brand::PROTOCOL_SCHEME),
    ] {
        if windows_registry::CURRENT_USER.open(&key).is_ok() {
            windows_registry::CURRENT_USER
                .remove_tree(&key)
                .map_err(|error| shell_error("无法删除通知注册表项", error))?;
        }
    }
    Ok(())
}

fn create_shortcut(executable: &Path, link: &Path) -> Result<(), ShellError> {
    let executable_text = executable.to_string_lossy().into_owned();
    let working_directory = executable
        .parent()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();

    unsafe {
        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| shell_error("无法创建快捷方式对象", error))?;
        shell_link
            .SetPath(&HSTRING::from(executable_text))
            .and_then(|()| shell_link.SetWorkingDirectory(&HSTRING::from(working_directory)))
            .and_then(|()| shell_link.SetDescription(&HSTRING::from(brand::DISPLAY_NAME)))
            .map_err(|error| shell_error("无法设置快捷方式目标", error))?;

        let store: IPropertyStore = shell_link
            .cast()
            .map_err(|error| shell_error("无法访问快捷方式属性", error))?;
        // Shell 的属性存储接管 PROPVARIANT 内的 CoTaskMem 内存；此处不能回收，
        // 否则提交时会二次释放并触发 STATUS_HEAP_CORRUPTION。
        set_string_property(&store, PROPERTY_AUMID, brand::AUMID)?;
        set_clsid_property(&store, PROPERTY_TOAST_ACTIVATOR, PROTOCOL_TOAST_ACTIVATOR)?;
        store
            .Commit()
            .map_err(|error| shell_error("无法提交快捷方式属性", error))?;

        if let Some(parent) = link.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| shell_error("无法创建开始菜单目录", error))?;
        }
        let persist: IPersistFile = shell_link
            .cast()
            .map_err(|error| shell_error("无法保存快捷方式", error))?;
        persist
            .Save(&HSTRING::from(link.to_string_lossy().into_owned()), true)
            .map_err(|error| shell_error("无法写入快捷方式", error))?;
    }
    Ok(())
}

fn set_string_property(store: &IPropertyStore, id: u32, value: &str) -> Result<(), ShellError> {
    let key = PROPERTYKEY {
        fmtid: APP_USER_MODEL_FORMAT,
        pid: id,
    };
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let pointer = unsafe { SHStrDupW(PCWSTR(wide.as_ptr())) }
        .map_err(|error| shell_error("无法分配快捷方式属性内存", error))?;

    let mut variant = PROPVARIANT::default();
    unsafe {
        let inner = (&mut variant as *mut PROPVARIANT).cast::<PROPVARIANT_0_0>();
        (*inner).vt = VT_LPWSTR;
        (*inner).Anonymous.pwszVal = pointer;
    }
    unsafe { store.SetValue(&key, &variant) }
        .map_err(|error| shell_error("无法写入快捷方式属性", error))
}

fn set_clsid_property(store: &IPropertyStore, id: u32, value: GUID) -> Result<(), ShellError> {
    let key = PROPERTYKEY {
        fmtid: APP_USER_MODEL_FORMAT,
        pid: id,
    };
    let variant = unsafe { InitPropVariantFromCLSID(&value) }
        .map_err(|error| shell_error("无法构造激活标识", error))?;
    // 同字符串属性：内存随属性存储释放，不在此清空。
    unsafe { store.SetValue(&key, &variant) }
        .map_err(|error| shell_error("无法写入快捷方式激活标识", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_shortcut_with_aumid_and_activator_properties() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let directory = tempfile::tempdir().unwrap();
        let link = directory.path().join("test.lnk");
        let executable = std::env::current_exe().unwrap();
        create_shortcut(&executable, &link).unwrap();
        assert!(link.is_file());
    }

    #[test]
    fn start_menu_link_has_the_expected_name() {
        assert!(start_menu_link().unwrap().ends_with(brand::SHORTCUT_NAME));
    }
}
