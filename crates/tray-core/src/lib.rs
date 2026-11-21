//! nkyoa 桌面通知共享核心。
//!
//! 本 crate 只包含与平台无关的协议 DTO、HTTPS 传输、接收状态机、游标持久化与
//! 设置命令，不引用任何 UI 或平台 SDK。Windows / Linux 入口实现平台适配 trait。

pub mod protocol;
pub mod reception;

pub use protocol::credential::{Binding, DeviceToken, PairingCode};
pub use protocol::error::{ApiFailure, ApiIssue, CredentialError, OriginError, StoreError};
pub use protocol::origin::ServerOrigin;
pub use protocol::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};
pub use protocol::{DesktopApi, NotificationApi};

pub use reception::client::NotificationClient;
pub use reception::contracts::{
    ClientError, ClientPhase, ClientStateStore, ClientStatus, CredentialStore, DeliveryReport,
    DeliveryResult, DeviceRegistration, Popup, RecoveryReason, SystemNotifier,
};
pub use reception::session::Clock;
pub use reception::state::{ClientState, PendingPage, StateRead};
