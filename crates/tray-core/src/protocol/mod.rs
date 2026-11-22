//! 桌面通知协议 v1 的客户端基线。
//!
//! 常量锚点与 `nkyoa-tray`（.NET）的 `DesktopProtocol` 完全一致；字段形态以
//! `contracts/nkyoa-desktop-v1.openapi.json` 快照为准，两处不一致时以契约快照
//! 与契约测试为准。

pub mod api;
pub mod credential;
pub mod dto;
pub mod error;
pub mod origin;
pub mod transport;
pub mod utc_time;
pub mod validation;

pub use api::{DesktopApi, NotificationApi};
pub use credential::{Binding, DeviceToken, PairingCode};
pub use dto::{
    AckReceipt, DeviceView, Feed, FeedItem, PendingSummary, RegisterReceipt, RegisterRequest,
    ResyncReceipt, RevokeReceipt, Seq,
};
pub use error::{ApiFailure, ApiIssue};
pub use origin::ServerOrigin;

use std::time::Duration;

/// 协议版本：未知重大版本停止并提示升级，不用默认空对象吞错。
pub const PROTOCOL_VERSION: i32 = 1;

// ── 设备路由（相对 base origin；服务器地址只允许 HTTPS 根路径）──
pub const PATH_REGISTER: &str = "/api/desktop/v1/devices/register";
pub const PATH_DEVICE: &str = "/api/desktop/v1/device";
pub const PATH_NOTIFICATIONS: &str = "/api/desktop/v1/notifications";
pub const PATH_ACKS: &str = "/api/desktop/v1/acks";
pub const PATH_RESYNC: &str = "/api/desktop/v1/resync";
pub const PATH_REVOKE_SELF: &str = "/api/desktop/v1/device";

/// Native 客户端与网页同口径的防伪头（服务端全站要求，不豁免）。
pub const CSRF_HEADER_NAME: &str = "X-Requested-With";
pub const CSRF_HEADER_VALUE: &str = "XMLHttpRequest";

/// Authorization 头携带两层凭证；总长度上限 256 字符。
pub const AUTHORIZATION_SCHEME: &str = "Bearer";
pub const MAX_AUTHORIZATION_LENGTH: usize = 256;

/// 所有 64 位序号/版本在 JSON 中一律是十进制字符串。
pub const SEQUENCE_FIELD_TYPE: &str = "decimal-string";

/// 七场景（与服务端通知场景同名单；客户端不发明新键）。
pub const SCENARIO_KEYS: [&str; 7] = [
    "todo",
    "cc",
    "archive-cc",
    "completed",
    "workorder-close",
    "rejected",
    "workorder-deadline",
];

/// 凭证用途前缀（格式 `nky-<用途>-v1.<公开ID>.<随机秘密>`）。
pub const PAIRING_CREDENTIAL_PREFIX: &str = "nky-pair-v1";
pub const DEVICE_CREDENTIAL_PREFIX: &str = "nky-device-v1";

/// 凭证固定三段，以 '.' 分隔。
pub const CREDENTIAL_FIELD_COUNT: usize = 3;

// ── 请求体防御上限 ──
pub const MAX_REGISTER_BODY_BYTES: usize = 8 * 1024;
pub const MAX_ACK_BODY_BYTES: usize = 2 * 1024;
pub const MAX_FEED_RESPONSE_BYTES: usize = 256 * 1024;

/// 单次外部调用上限（方案建议 15 秒）。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 建议参数基线；实现从常量读取默认值，服务端选项可下发不同值。
pub const MAX_DEVICES_PER_USER: i32 = 5;
pub const POLL_INTERVAL_SECONDS: i32 = 60;
pub const POLL_JITTER_FRACTION: f64 = 0.10;
pub const DEFAULT_PAGE_SIZE: i32 = 50;
pub const MAX_PAGE_SIZE: i32 = 100;
pub const RETENTION_DAYS: i32 = 30;
pub const WORKFLOW_DISPLAY_TTL_HOURS: i32 = 24;
pub const DEADLINE_DISPLAY_TTL_MINUTES: i32 = 60;

/// 单页恢复最多弹出 3 条，其余合并提示。
pub const MAX_POPUPS_PER_PAGE: i32 = 3;

/// 客户端能力声明：本版本已支持流程标题与流转意见预览。
/// 在设备注册时声明；服务端只对声明的设备下发预览文案，旧客户端继续收通用文案。
pub const PREVIEW_CAPABLE: bool = true;

/// 失败退避阶梯（秒），实际取值再加抖动；401/撤销不进退避直接停。
pub const BACKOFF_SECONDS: [u64; 6] = [5, 15, 30, 60, 120, 300];

pub const REVOKED_DEVICE_RETENTION_DAYS: i32 = 30;
