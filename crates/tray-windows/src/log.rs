//! 最小滚动日志。
//!
//! 只记录生命周期与错误分类，不写凭证、完整 URL 或通知正文。

use std::fs::OpenOptions;
use std::io::Write;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub fn write(event: &str) {
    let directory = crate::brand::log_dir();
    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }
    let path = directory.join("tray.log");
    if let Ok(metadata) = std::fs::metadata(&path)
        && metadata.len() > MAX_LOG_BYTES
    {
        let _ = std::fs::rename(&path, directory.join("tray.1.log"));
    }
    let timestamp = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{timestamp} {event}");
    }
}
