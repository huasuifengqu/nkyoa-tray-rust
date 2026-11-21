//! nkyoa 桌面通知与托盘客户端（Windows / Slint）。
//!
//! 命令行与 .NET 版保持一致：`--install-integration [--autostart]`、
//! `--remove-integration [--clear-binding]`、`--check-instance`、
//! `--background`，以及 `nkyoa-notify-rust://open/<配置标识>` 激活参数。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod activation;
mod brand;
mod controller;
mod credential;
mod log;
mod notifier;
mod platform;
mod shell;
mod single_instance;

use std::path::Path;
use std::process::ExitCode;

use tokio_util::sync::CancellationToken;
use tray_core::{
    ApiIssue, CredentialStore, DeliveryResult, NotificationApi, Popup, SystemNotifier,
};

use activation::Activation;
use credential::WindowsCredentialStore;
use platform::WindowsPlatform;

fn main() -> ExitCode {
    ExitCode::from(run())
}

fn run() -> u8 {
    let executable = std::env::current_exe().unwrap_or_default();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args == ["--selftest-notification"] {
        return selftest_notification();
    }

    let install = args == ["--install-integration"];
    let install_autostart = args == ["--install-integration", "--autostart"];
    if install || install_autostart {
        return install_integration(&executable, install_autostart);
    }

    let remove =
        args == ["--remove-integration"] || args == ["--remove-integration", "--clear-binding"];
    let clear_binding = args.len() == 2;
    let check = args == ["--check-instance"];
    let background = args == ["--background"];

    let mut initial = None;
    if !remove && !check && !background && !args.is_empty() {
        if args.len() != 1 {
            return 2;
        }
        match Activation::from_uri(&args[0]) {
            Some(activation) => initial = Some(activation),
            None => return 2,
        }
    }

    let instance = match single_instance::SingleInstance::acquire(&brand::data_dir()) {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            if remove || check {
                return 3;
            }
            if background {
                return 0;
            }
            let runtime = new_runtime();
            let activation = initial.unwrap_or(Activation::SHOW_SETTINGS);
            let forwarded = runtime.block_on(single_instance::forward(
                &single_instance::pipe_name(),
                &activation,
            ));
            runtime.shutdown_timeout(std::time::Duration::from_secs(1));
            return if forwarded { 0 } else { 2 };
        }
        Err(error) => {
            log::write(&format!("instance_lock_error={error}"));
            return 4;
        }
    };

    if check {
        return 0;
    }
    if remove {
        return remove_integration(&executable, clear_binding);
    }

    if let Err(error) = shell::set_process_app_id() {
        log::write(&format!("set_process_app_id_error={error}"));
    }
    match controller::run(executable, initial, background, instance) {
        Ok(code) => code,
        Err(error) => {
            log::write(&format!("run_error={error}"));
            4
        }
    }
}

fn selftest_notification() -> u8 {
    let runtime = new_runtime();
    let notifier = notifier::WindowsNotifier::new();
    let popup = Popup {
        title: "OA通知".to_owned(),
        body: "这是一条本机测试通知。".to_owned(),
        home: None,
        event_count: 1,
        profile_id: None,
    };
    let result = runtime.block_on(notifier.show(&popup, &CancellationToken::new()));
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    if matches!(result, Ok(DeliveryResult::Accepted)) {
        0
    } else {
        1
    }
}

fn install_integration(executable: &Path, autostart: bool) -> u8 {
    if let Err(error) = shell::install(executable) {
        log::write(&format!("install_integration_error={error}"));
        return 4;
    }
    if autostart {
        let platform = WindowsPlatform::new(executable.to_path_buf());
        if let Err(error) = platform.set_autostart(true) {
            log::write(&format!("install_autostart_error={error}"));
            return 4;
        }
    }
    0
}

fn remove_integration(executable: &Path, clear_binding: bool) -> u8 {
    let platform = WindowsPlatform::new(executable.to_path_buf());
    let mut reached_server = true;

    if clear_binding {
        let runtime = new_runtime();
        let store = WindowsCredentialStore::new();
        let cancel = CancellationToken::new();

        match runtime.block_on(store.read(&cancel)) {
            Ok(Some(binding)) => {
                let api = tray_core::DesktopApi::new();
                match runtime.block_on(api.revoke(&binding, &cancel)) {
                    Ok(()) => {}
                    Err(failure) if failure.issue == ApiIssue::CredentialInvalid => {}
                    Err(_) => reached_server = false,
                }
            }
            Ok(None) => {}
            Err(_) => reached_server = false,
        }

        if runtime.block_on(store.remove(&cancel)).is_err() {
            runtime.shutdown_timeout(std::time::Duration::from_secs(1));
            return 4;
        }
        platform.clear_popup_pause();
        runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    }

    if platform.set_autostart(false).is_err() {
        return 4;
    }
    if shell::remove().is_err() {
        return 4;
    }

    if reached_server { 0 } else { 5 }
}

fn new_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio 运行时初始化失败")
}
