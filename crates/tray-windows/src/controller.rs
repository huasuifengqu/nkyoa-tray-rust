//! Slint UI 与核心客户端的粘合层。
//!
//! 所有异步操作在 tokio 运行时执行；UI 线程通过脏标记与定时器刷新，
//! 跨线程只传递 `slint::Weak`（Slint 保证跨线程 upgrade 返回 None）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use slint::{CloseRequestResponse, ComponentHandle, Timer, TimerMode, Weak};
use time::OffsetDateTime;
use time::UtcOffset;
use time::format_description::FormatItem;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use tray_core::reception::store::JsonClientStateStore;
use tray_core::{
    ApiIssue, ClientError, ClientPhase, ClientStatus, CredentialStore, DeliveryResult, DesktopApi,
    DeviceRegistration, NotificationClient, PairingCode, Popup, RecoveryReason, ServerOrigin,
    SystemNotifier,
};
use zeroize::Zeroize;

use crate::activation::Activation;
use crate::brand;
use crate::credential::WindowsCredentialStore;
use crate::notifier::WindowsNotifier;
use crate::platform::WindowsPlatform;
use crate::single_instance::{self, SingleInstance};
use crate::{AppTray, AppWindow, log};

const LOCAL_TIME_FORMAT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");

pub struct SharedState {
    pub runtime: Handle,
    pub client: Arc<NotificationClient>,
    pub platform: Arc<WindowsPlatform>,
    pub notifier: Arc<WindowsNotifier>,
    pub credentials: Arc<WindowsCredentialStore>,
    pub registration: DeviceRegistration,
    pub lifetime: CancellationToken,
    pub tray_available: bool,
    pub busy: AtomicBool,
    pub exiting: AtomicBool,
    pub dirty: Arc<AtomicBool>,
    pub window: Weak<AppWindow>,
    pub tray: Weak<AppTray>,
    pub start_in_tray: bool,
    pub initial: Option<Activation>,
}

pub fn run(
    executable: std::path::PathBuf,
    initial: Option<Activation>,
    background: bool,
    _instance: SingleInstance,
) -> Result<u8, Box<dyn std::error::Error>> {
    // 设置窗口体量很小，软件渲染足够，且不加载 GPU 驱动栈，常驻内存更低。
    let _ = slint::BackendSelector::new()
        .backend_name("winit".to_owned())
        .renderer_name("software".to_owned())
        .select();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    let window = AppWindow::new()?;
    let tray = AppTray::new()?;

    let platform = Arc::new(WindowsPlatform::new(executable));
    let notifier = Arc::new(WindowsNotifier::new());
    let credentials = Arc::new(WindowsCredentialStore::new());
    let state_store = Arc::new(JsonClientStateStore::new(brand::state_root()));
    let tray_available = platform.tray_available();
    let start_in_tray = background || initial.is_some();

    let dirty = Arc::new(AtomicBool::new(false));
    let progress = {
        let dirty = dirty.clone();
        Arc::new(move |_status: ClientStatus| {
            dirty.store(true, Ordering::SeqCst);
        })
    };

    let client = Arc::new(NotificationClient::new(
        Arc::new(DesktopApi::new()),
        credentials.clone(),
        state_store,
        notifier.clone(),
        Some(progress),
        None,
    ));

    let registration = DeviceRegistration {
        name: machine_name(),
        platform: "windows".to_owned(),
        architecture: if std::env::consts::ARCH == "aarch64" {
            "arm64".to_owned()
        } else {
            "x64".to_owned()
        },
        client_version: env!("CARGO_PKG_VERSION").to_owned(),
    };

    let state = Arc::new(SharedState {
        runtime: runtime.handle().clone(),
        client,
        platform,
        notifier,
        credentials,
        registration,
        lifetime: CancellationToken::new(),
        tray_available,
        busy: AtomicBool::new(false),
        exiting: AtomicBool::new(false),
        dirty,
        window: window.as_weak(),
        tray: tray.as_weak(),
        start_in_tray,
        initial,
    });

    wire_callbacks(&state, &window, &tray);
    window.set_device_text(state.registration.name.clone().into());
    render(&state);

    let timer = Timer::default();
    {
        let timer_state = state.clone();
        timer.start(TimerMode::Repeated, Duration::from_millis(150), move || {
            if timer_state.dirty.swap(false, Ordering::SeqCst) {
                render(&timer_state);
            }
        });
    }

    let listener = {
        let dispatch_state = state.clone();
        single_instance::spawn_listener(
            &state.runtime,
            single_instance::pipe_name(),
            move |activation| {
                let state = dispatch_state.clone();
                spawn(state.clone(), async move {
                    handle_activation(&state, activation).await;
                });
            },
            state.lifetime.clone(),
        )
    };

    initialize(state.clone());

    if !state.start_in_tray || !state.tray_available {
        window.show()?;
    }

    let exit_code = match slint::run_event_loop() {
        Ok(()) => 0,
        Err(error) => {
            log::write(&format!("event_loop_error={error}"));
            1
        }
    };

    state.lifetime.cancel();
    runtime.block_on(state.client.dispose());
    drop(timer);
    drop(listener);
    runtime.shutdown_timeout(Duration::from_secs(2));
    Ok(exit_code)
}

fn spawn(state: Arc<SharedState>, future: impl std::future::Future<Output = ()> + Send + 'static) {
    state.runtime.spawn(future);
}

fn wire_callbacks(state: &Arc<SharedState>, window: &AppWindow, tray: &AppTray) {
    {
        let state = state.clone();
        window.on_bind_clicked(move || spawn_bind(&state));
    }
    {
        let state = state.clone();
        window.on_retry_clicked(move || spawn_retry(&state));
    }
    {
        let state = state.clone();
        window.on_unbind_requested(move || {
            if let Some(window) = state.window.upgrade() {
                window.set_confirming_unbind(true);
            }
        });
    }
    {
        let state = state.clone();
        window.on_unbind_cancelled(move || {
            if let Some(window) = state.window.upgrade() {
                window.set_confirming_unbind(false);
            }
        });
    }
    {
        let state = state.clone();
        window.on_unbind_confirmed(move || spawn_unbind(&state));
    }
    {
        let state = state.clone();
        window.on_autostart_toggled(move |value| set_autostart(&state, value));
    }
    {
        let state = state.clone();
        window.on_pause_toggled(move |value| apply_pause(&state, value));
    }
    {
        let state = state.clone();
        window.on_open_home(move || open_home(&state));
    }
    {
        let state = state.clone();
        window.on_test_notification(move || spawn_test_notification(&state));
    }
    {
        let state = state.clone();
        window.on_export_diagnostics(move || export_diagnostics(&state));
    }
    {
        let state = state.clone();
        window.on_exit_clicked(move || start_exit(&state));
    }

    {
        let state = state.clone();
        tray.on_show_settings(move || show_window(&state));
    }
    {
        let state = state.clone();
        tray.on_open_home(move || open_home(&state));
    }
    {
        let state = state.clone();
        tray.on_toggle_pause(move || {
            let value = !state.client.is_paused();
            apply_pause(&state, value);
        });
    }
    {
        let state = state.clone();
        tray.on_test_notification(move || spawn_test_notification(&state));
    }
    {
        let state = state.clone();
        tray.on_exit_requested(move || start_exit(&state));
    }

    {
        let state = state.clone();
        window.window().on_close_requested(move || {
            if !state.tray_available || state.exiting.load(Ordering::SeqCst) {
                start_exit(&state);
            }
            CloseRequestResponse::HideWindow
        });
    }
}

fn initialize(state: Arc<SharedState>) {
    let state = state.clone();
    spawn(state.clone(), async move {
        let paused = state.platform.are_popups_paused();
        let _ = state.client.set_paused(paused, &state.lifetime).await;
        let _ = state.client.start(&state.lifetime).await;
        state.dirty.store(true, Ordering::SeqCst);

        if state.client.home().is_none() {
            show_window(&state);
        }
        if let Some(activation) = state.initial.clone() {
            handle_activation(&state, activation).await;
        }
    });
}

async fn handle_activation(state: &Arc<SharedState>, activation: Activation) {
    let Some(profile_id) = activation.profile_id else {
        show_window(state);
        return;
    };

    let binding = state.credentials.read(&state.lifetime).await.ok().flatten();
    if let Some(binding) = binding
        && binding.profile_id() == profile_id
        && let Err(error) = state.platform.open_home(binding.origin())
    {
        log::write(&format!("open_home_error={error}"));
        show_window(state);
        set_feedback_async(state, "无法打开已保存的 OA 首页。".to_owned());
    }
}

fn render(state: &SharedState) {
    let status = state.client.status();
    let home = state.client.home();
    let Some(window) = state.window.upgrade() else {
        return;
    };

    window.set_status_text(status_label(&status).into());
    window.set_details_text(details_text(&status, home.as_ref()).into());
    window.set_recovery_text(recovery_text(status.recovery).unwrap_or_default().into());
    window.set_show_tray_warning(!state.tray_available);
    window.set_paused(state.client.is_paused());
    window.set_has_binding(home.is_some());
    window.set_can_retry(matches!(
        status.phase,
        ClientPhase::Offline
            | ClientPhase::Backoff
            | ClientPhase::ServerDisabled
            | ClientPhase::StorageError
            | ClientPhase::RequestError
            | ClientPhase::CertificateError
            | ClientPhase::NotificationUnavailable
            | ClientPhase::Stopped
    ));
    window.set_busy(state.busy.load(Ordering::SeqCst));
    window.set_autostart(state.platform.is_autostart_enabled());
    window.set_version_text(format!("nkyoa {}", state.registration.client_version).into());
    if window.get_origin_text().is_empty()
        && let Some(home) = &home
    {
        window.set_origin_text(home.as_str().into());
    }

    if let Some(tray) = state.tray.upgrade() {
        tray.set_status_text(status_label(&status).into());
        tray.set_has_binding(home.is_some());
        tray.set_paused(state.client.is_paused());
    }
}

fn spawn_bind(state: &Arc<SharedState>) {
    if state.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(window) = state.window.upgrade() else {
        state.busy.store(false, Ordering::SeqCst);
        return;
    };

    let origin_text = window.get_origin_text().to_string();
    let mut pairing_text = window.get_pairing_text().to_string();
    let device_name = window.get_device_text().to_string();
    window.set_pairing_text("".into());
    window.set_feedback_text("".into());
    state.dirty.store(true, Ordering::SeqCst);

    let origin = match ServerOrigin::parse(&origin_text) {
        Ok(origin) => origin,
        Err(error) => {
            finish_feedback(state, error.to_string());
            return;
        }
    };
    let code = match PairingCode::parse(&pairing_text) {
        Ok(code) => code,
        Err(error) => {
            pairing_text.zeroize();
            finish_feedback(state, error.to_string());
            return;
        }
    };
    pairing_text.zeroize();

    let state = state.clone();
    spawn(state.clone(), async move {
        let registration = DeviceRegistration {
            name: device_name,
            ..state.registration.clone()
        };
        let result = state
            .client
            .bind(origin, code, registration, &state.lifetime)
            .await;
        state.busy.store(false, Ordering::SeqCst);
        state.dirty.store(true, Ordering::SeqCst);
        match result {
            Ok(true) => set_feedback_async(&state, "设备已绑定。".to_owned()),
            Ok(false) | Err(ClientError::Cancelled) => {}
            Err(_) => set_feedback_async(&state, "本机操作失败，请重试。".to_owned()),
        }
    });
}

fn spawn_unbind(state: &Arc<SharedState>) {
    if state.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(window) = state.window.upgrade() {
        window.set_confirming_unbind(false);
    }
    state.dirty.store(true, Ordering::SeqCst);

    let state = state.clone();
    spawn(state.clone(), async move {
        let result = state.client.unbind(&state.lifetime).await;
        state.busy.store(false, Ordering::SeqCst);
        state.dirty.store(true, Ordering::SeqCst);
        match result {
            Ok(true) => set_feedback_async(&state, "设备绑定已解除。".to_owned()),
            Ok(false) | Err(ClientError::Cancelled) => {}
            Err(_) => set_feedback_async(&state, "本机操作失败，请重试。".to_owned()),
        }
    });
}

fn spawn_retry(state: &Arc<SharedState>) {
    if state.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let state = state.clone();
    spawn(state.clone(), async move {
        let _ = state.client.start(&state.lifetime).await;
        state.busy.store(false, Ordering::SeqCst);
        state.dirty.store(true, Ordering::SeqCst);
    });
}

fn set_autostart(state: &Arc<SharedState>, value: bool) {
    match state.platform.set_autostart(value) {
        Ok(()) => {
            set_feedback(
                state,
                if value {
                    "已开启当前用户自启动。"
                } else {
                    "已关闭自启动。"
                },
            );
        }
        Err(_) => {
            if let Some(window) = state.window.upgrade() {
                window.set_autostart(!value);
            }
            set_feedback(state, "无法保存系统自启动设置。");
        }
    }
    state.dirty.store(true, Ordering::SeqCst);
}

fn apply_pause(state: &Arc<SharedState>, value: bool) {
    if state.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let state = state.clone();
    spawn(state.clone(), async move {
        let saved = state.platform.save_popup_pause(value);
        let result = state.client.set_paused(value, &state.lifetime).await;
        state.busy.store(false, Ordering::SeqCst);
        state.dirty.store(true, Ordering::SeqCst);
        if saved.is_err() || result.is_err() {
            set_feedback_async(&state, "无法保存弹窗设置。".to_owned());
        }
    });
}

fn open_home(state: &Arc<SharedState>) {
    let Some(home) = state.client.home() else {
        return;
    };
    if state.platform.open_home(&home).is_err() {
        set_feedback(state, "无法启动默认浏览器。");
    }
}

fn spawn_test_notification(state: &Arc<SharedState>) {
    if state.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let state = state.clone();
    spawn(state.clone(), async move {
        let popup = Popup {
            title: "nkyoa 桌面通知".to_owned(),
            body: "这是一条本机测试通知。".to_owned(),
            home: None,
            event_count: 1,
            profile_id: None,
        };
        let result = state.notifier.show(&popup, &state.lifetime).await;
        state.busy.store(false, Ordering::SeqCst);
        state.dirty.store(true, Ordering::SeqCst);
        let feedback = if matches!(result, Ok(DeliveryResult::Accepted)) {
            "系统已接受测试通知。"
        } else {
            "系统未接受测试通知，请检查通知设置。"
        };
        set_feedback_async(&state, feedback.to_owned());
    });
}

fn export_diagnostics(state: &Arc<SharedState>) {
    let status = state.client.status();
    let text = format!(
        "version={}\nphase={:?}\nissue={}\nrecovery={:?}\nlastSyncUtc={}\ntrayAvailable={}\n",
        state.registration.client_version,
        status.phase,
        status.issue.map(ApiIssue::as_str).unwrap_or("-"),
        status.recovery,
        status
            .last_sync_at
            .and_then(|time| time.format(&Rfc3339).ok())
            .unwrap_or_else(|| "-".to_owned()),
        state.tray_available
    );

    let result = (|| -> std::io::Result<std::path::PathBuf> {
        let directory = brand::diagnostics_dir();
        std::fs::create_dir_all(&directory)?;
        let stamp = OffsetDateTime::now_utc()
            .to_offset(UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC))
            .format(format_description!(
                "[year][month][day]-[hour][minute][second]"
            ))
            .unwrap_or_else(|_| "unknown".to_owned());
        let path = directory.join(format!("nkyoa-tray-diagnostics-{stamp}.txt"));
        std::fs::write(&path, text.as_bytes())?;
        Ok(path)
    })();

    match result {
        Ok(path) => set_feedback(state, format!("诊断已导出到 {}", path.display())),
        Err(_) => set_feedback(state, "无法保存诊断文件。"),
    }
    state.dirty.store(true, Ordering::SeqCst);
}

fn start_exit(state: &Arc<SharedState>) {
    if state.exiting.swap(true, Ordering::SeqCst) {
        return;
    }
    state.lifetime.cancel();
    let state = state.clone();
    spawn(state.clone(), async move {
        let _ = state.client.stop(&CancellationToken::new()).await;
        let window = state.window.clone();
        let tray = state.tray.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(tray) = tray.upgrade() {
                let _ = tray.hide();
            }
            if let Some(window) = window.upgrade() {
                let _ = window.hide();
            }
            let _ = slint::quit_event_loop();
        });
    });
}

/// UI 线程：直接设置反馈并刷新。
fn set_feedback(state: &SharedState, text: impl Into<String>) {
    if let Some(window) = state.window.upgrade() {
        window.set_feedback_text(text.into().into());
    }
    state.dirty.store(true, Ordering::SeqCst);
}

/// 后台线程：回到 UI 线程设置反馈。
fn set_feedback_async(state: &SharedState, text: String) {
    let _ = state.window.upgrade_in_event_loop(move |window| {
        window.set_feedback_text(text.into());
    });
}

fn finish_feedback(state: &Arc<SharedState>, text: String) {
    state.busy.store(false, Ordering::SeqCst);
    set_feedback(state, text);
}

fn show_window(state: &SharedState) {
    let _ = state.window.upgrade_in_event_loop(|window| {
        let _ = window.show();
    });
}

fn details_text(status: &ClientStatus, home: Option<&ServerOrigin>) -> String {
    let mut lines = Vec::new();
    if let Some(account) = status.account_id {
        lines.push(format!("账号标识 {account}"));
    }
    if let Some(name) = &status.device_name {
        lines.push(format!("已绑定设备 {name}"));
    }
    if let Some(last_sync) = status.last_sync_at {
        lines.push(format!("最近同步 {}", format_local(last_sync)));
    }
    if let Some(pending) = status.pending_task_count {
        lines.push(format!("当前待办 {pending} 项"));
    }
    if home.is_some() {
        lines.push("设备令牌存于系统凭据库".to_owned());
    }
    lines.join("\n")
}

fn format_local(time: OffsetDateTime) -> String {
    let local = UtcOffset::current_local_offset()
        .map(|offset| time.to_offset(offset))
        .unwrap_or(time);
    local
        .format(LOCAL_TIME_FORMAT)
        .unwrap_or_else(|_| time.format(&Rfc3339).unwrap_or_default())
}

fn recovery_text(reason: Option<RecoveryReason>) -> Option<String> {
    match reason {
        Some(RecoveryReason::HistoryExpired) => {
            Some("部分通知历史已过期，已请求从服务器当前进度恢复。".to_owned())
        }
        Some(RecoveryReason::StreamChanged) => {
            Some("通知进度发生变化，正在以服务器记录恢复。".to_owned())
        }
        Some(RecoveryReason::DamagedState) => {
            Some("本机接收记录损坏，正在从服务器恢复。".to_owned())
        }
        Some(RecoveryReason::DamagedStateWithHistoryExpired) => {
            Some("本机接收记录损坏，部分历史已过期。仍保留的通知会重新同步。".to_owned())
        }
        None => None,
    }
}

fn status_label(status: &ClientStatus) -> String {
    if let Some(issue) = status.issue {
        let label = match issue {
            ApiIssue::CredentialInvalid => Some("校验码或设备绑定已失效，请重新绑定"),
            ApiIssue::DeviceLimit => Some("已达到设备上限，请到 OA 安全中心撤销闲置设备"),
            ApiIssue::Csrf => Some("客户端请求异常，请检查客户端版本"),
            ApiIssue::Validation => Some("注册参数不符合要求，请检查地址和设备名称"),
            ApiIssue::VersionConflict => Some("设置版本已变化，请重新打开 OA 安全中心"),
            _ => None,
        };
        if let Some(label) = label {
            return label.to_owned();
        }
    }

    match status.phase {
        ClientPhase::Unconfigured => "尚未绑定",
        ClientPhase::Binding => "正在核验连接",
        ClientPhase::Connected => "已连接",
        ClientPhase::LocallyPaused => "已连接，弹窗已暂停",
        ClientPhase::Offline => "网络已断开，等待重试",
        ClientPhase::Backoff => "服务器暂时不可用，等待重试",
        ClientPhase::Resynchronizing => "正在恢复通知进度",
        ClientPhase::ServerDisabled => "服务器已关闭桌面通知",
        ClientPhase::CertificateError => "无法验证服务器证书，请检查证书和站点地址",
        ClientPhase::UpgradeRequired => "协议版本不兼容，请升级客户端",
        ClientPhase::StorageError => "无法访问本机状态或系统凭据库",
        ClientPhase::NotificationUnavailable => "系统通知不可用，请检查系统通知设置",
        ClientPhase::Stopping => "正在结束连接",
        ClientPhase::Stopped => "连接已停止",
        ClientPhase::CredentialInvalid => "校验码或设备绑定已失效，请重新绑定",
        ClientPhase::RequestError => "服务器响应异常，请检查站点地址和客户端版本",
    }
    .to_owned()
}

fn machine_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "办公电脑".to_owned())
}
