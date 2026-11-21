//! WinRT Toast 通知。
//!
//! 使用明确的 AUMID 与开始菜单快捷方式；点击通过受控协议激活，不传任意
//! URL。仅 AUMID 与 64 位配置标识进入 XML，正文按 XML 规则转义。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tray_core::{ClientError, DeliveryResult, Popup, SystemNotifier};
use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{
    NotificationSetting, ToastNotification, ToastNotificationManager,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::{Error, HSTRING};

use crate::activation::Activation;
use crate::brand;

const RETRYABLE_HRESULT: [u32; 3] = [0x8007_06BA, 0x8001_0108, 0x8007_06BE];
const SETTINGS_NOT_FOUND_HRESULT: u32 = 0x8007_0490;

pub struct WindowsNotifier {
    shortcut: PathBuf,
}

impl WindowsNotifier {
    pub fn new() -> Self {
        Self {
            shortcut: crate::shell::start_menu_link().unwrap_or_default(),
        }
    }

    fn deliver(&self, popup: &Popup, cancel: &CancellationToken) -> DeliveryResult {
        if cancel.is_cancelled() {
            return DeliveryResult::Unavailable;
        }
        if self.shortcut.as_os_str().is_empty() || !Path::new(&self.shortcut).is_file() {
            return DeliveryResult::Unavailable;
        }

        // WinRT 调用可能发生在接收任务的线程；MTA 初始化可重复调用。
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }

        let notifier =
            match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(brand::AUMID))
            {
                Ok(notifier) => notifier,
                Err(_) => return DeliveryResult::Unavailable,
            };
        match notifier.Setting() {
            Ok(setting) if setting != NotificationSetting::Enabled => {
                return DeliveryResult::Unavailable;
            }
            Ok(_) => {}
            Err(error) if hresult(&error) == SETTINGS_NOT_FOUND_HRESULT => {}
            Err(_) => return DeliveryResult::Unavailable,
        }

        let document = match XmlDocument::new() {
            Ok(document) => document,
            Err(_) => return DeliveryResult::Unavailable,
        };
        if document
            .LoadXml(&HSTRING::from(build_toast_xml(popup)))
            .is_err()
        {
            return DeliveryResult::Unavailable;
        }
        let toast = match ToastNotification::CreateToastNotification(&document) {
            Ok(toast) => toast,
            Err(_) => return DeliveryResult::Unavailable,
        };

        match notifier.Show(&toast) {
            Ok(()) => DeliveryResult::Accepted,
            Err(error) if RETRYABLE_HRESULT.contains(&hresult(&error)) => {
                DeliveryResult::RetryableFailure
            }
            Err(_) => DeliveryResult::Unavailable,
        }
    }
}

impl Default for WindowsNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemNotifier for WindowsNotifier {
    async fn show(
        &self,
        popup: &Popup,
        cancel: &CancellationToken,
    ) -> Result<DeliveryResult, ClientError> {
        Ok(self.deliver(popup, cancel))
    }
}

/// 生成 Toast XML；只有合法配置标识才写入协议激活属性。
pub fn build_toast_xml(popup: &Popup) -> String {
    let mut attributes = String::new();
    if let Some(profile_id) = &popup.profile_id
        && Activation::is_profile(profile_id)
        && let Some(uri) = Activation::to_uri(profile_id)
    {
        attributes = format!(
            r#" activationType="protocol" launch="{}""#,
            escape_xml(&uri)
        );
    }
    format!(
        r#"<toast{attributes}><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual></toast>"#,
        escape_xml(&popup.title),
        escape_xml(&popup.body)
    )
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn hresult(error: &Error) -> u32 {
    error.code().0 as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use tray_core::{Binding, DeviceToken, ServerOrigin};
    use uuid::Uuid;

    fn test_binding() -> Binding {
        let device = Uuid::new_v4();
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

    #[test]
    fn toast_escapes_text_and_uses_only_the_fixed_protocol_and_local_profile() {
        let binding = test_binding();
        let popup = Popup {
            title: "<hello>".to_owned(),
            body: "Use & read".to_owned(),
            home: Some(binding.origin().clone()),
            event_count: 1,
            profile_id: Some(binding.profile_id().to_owned()),
        };
        let xml = build_toast_xml(&popup);
        assert!(xml.contains(r#"activationType="protocol""#));
        let quote = '"';
        let expected = format!(
            "launch={quote}nkyoa-notify-rust://open/{}{quote}",
            binding.profile_id()
        );
        assert!(xml.contains(&expected));
        assert!(xml.contains("&lt;hello&gt;"));
        assert!(xml.contains("Use &amp; read"));
        assert!(!xml.contains("https:"));

        let tampered = Popup {
            profile_id: Some("https://attacker.test/".to_owned()),
            ..popup
        };
        assert!(!build_toast_xml(&tampered).contains("launch"));
    }
}
