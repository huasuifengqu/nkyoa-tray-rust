//! 客户端生命周期：绑定、解绑、暂停与单接收任务。
//!
//! 生命周期命令串行执行；绑定切换会取消并等待旧任务退出，迟到响应不会写入
//! 新状态。UI 只读取状态快照，不参与网络与持久化。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::protocol::error::{ApiFailure, ApiIssue};
use crate::protocol::origin::ServerOrigin;
use crate::protocol::{
    BACKOFF_SECONDS, Binding, DeviceToken, NotificationApi, POLL_JITTER_FRACTION, PairingCode,
    RegisterRequest,
};

use super::contracts::{
    ClientError, ClientPhase, ClientStateStore, ClientStatus, CredentialStore, DeviceRegistration,
    SystemNotifier,
};
use super::session::{Clock, PauseFn, PublishFn, ReceptionSession, SystemClock};

type Progress = Arc<dyn Fn(ClientStatus) + Send + Sync>;
type Jitter = Arc<dyn Fn() -> f64 + Send + Sync>;

struct WorkerState {
    cancel: Option<CancellationToken>,
    handle: Option<JoinHandle<()>>,
}

pub struct NotificationClient {
    api: Arc<dyn NotificationApi>,
    credentials: Arc<dyn CredentialStore>,
    state: Arc<dyn ClientStateStore>,
    notifier: Arc<dyn SystemNotifier>,
    clock: Arc<dyn Clock>,
    jitter: Jitter,
    progress: Option<Progress>,
    lifecycle: tokio::sync::Mutex<()>,
    worker: Mutex<WorkerState>,
    generation: AtomicU64,
    status: Mutex<ClientStatus>,
    paused: AtomicBool,
    home: Mutex<Option<ServerOrigin>>,
    disposed: AtomicBool,
}

impl NotificationClient {
    pub fn new(
        api: Arc<dyn NotificationApi>,
        credentials: Arc<dyn CredentialStore>,
        state: Arc<dyn ClientStateStore>,
        notifier: Arc<dyn SystemNotifier>,
        progress: Option<Progress>,
        clock: Option<Arc<dyn Clock>>,
    ) -> Self {
        Self {
            api,
            credentials,
            state,
            notifier,
            clock: clock.unwrap_or_else(|| Arc::new(SystemClock)),
            jitter: Arc::new(default_jitter),
            progress,
            lifecycle: tokio::sync::Mutex::new(()),
            worker: Mutex::new(WorkerState {
                cancel: None,
                handle: None,
            }),
            generation: AtomicU64::new(0),
            status: Mutex::new(ClientStatus::default()),
            paused: AtomicBool::new(false),
            home: Mutex::new(None),
            disposed: AtomicBool::new(false),
        }
    }

    /// 测试用：固定抖动函数（返回 0.5 表示无抖动）。
    pub fn with_jitter(mut self, jitter: Jitter) -> Self {
        self.jitter = jitter;
        self
    }

    pub fn status(&self) -> ClientStatus {
        lock(&self.status).clone()
    }

    pub fn home(&self) -> Option<ServerOrigin> {
        lock(&self.home).clone()
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub async fn start(self: &Arc<Self>, cancel: &CancellationToken) -> Result<(), ClientError> {
        let _guard = self.lock_lifecycle(cancel).await?;
        if self.disposed.load(Ordering::SeqCst) {
            return Err(ClientError::Cancelled);
        }

        let status = self.status();
        let unavailable_retry =
            status.phase == ClientPhase::NotificationUnavailable && status.retry_in.is_none();
        if self.worker_running() && !unavailable_retry {
            return Ok(());
        }

        self.stop_worker().await;
        match self.credentials.read(cancel).await {
            Ok(Some(binding)) => self.start_worker(binding, None),
            Ok(None) => self.publish(ClientStatus::new(ClientPhase::Unconfigured)),
            Err(ClientError::Cancelled) => return Err(ClientError::Cancelled),
            Err(_) => self.publish(ClientStatus::new(ClientPhase::StorageError)),
        }
        Ok(())
    }

    pub async fn bind(
        self: &Arc<Self>,
        origin: ServerOrigin,
        code: PairingCode,
        registration: DeviceRegistration,
        cancel: &CancellationToken,
    ) -> Result<bool, ClientError> {
        let _guard = self.lock_lifecycle(cancel).await?;
        if self.disposed.load(Ordering::SeqCst) {
            return Err(ClientError::Cancelled);
        }
        self.stop_worker().await;
        self.publish(ClientStatus::new(ClientPhase::Binding));

        let installation = match self.state.installation_id(cancel).await {
            Ok(id) => id,
            Err(ClientError::Cancelled) => {
                self.publish(ClientStatus::new(ClientPhase::Stopped));
                return Err(ClientError::Cancelled);
            }
            Err(_) => {
                self.publish(ClientStatus::new(ClientPhase::StorageError));
                return Ok(false);
            }
        };

        let request = RegisterRequest::new(
            installation,
            registration.name,
            registration.platform,
            registration.architecture,
            registration.client_version,
        );
        let receipt = match self.api.register(&origin, &code, &request, cancel).await {
            Ok(receipt) => receipt,
            Err(failure) => {
                self.publish(failure_status(&failure));
                return Ok(false);
            }
        };
        if cancel.is_cancelled() {
            self.publish(ClientStatus::new(ClientPhase::Stopped));
            return Err(ClientError::Cancelled);
        }

        let token = match DeviceToken::parse(&receipt.device_token) {
            Ok(token) => token,
            Err(_) => {
                self.publish(failure_status(&ApiFailure::new(ApiIssue::InvalidResponse)));
                return Ok(false);
            }
        };
        let binding = match Binding::new(origin, receipt.user_id, receipt.device_id, token) {
            Ok(binding) => binding,
            Err(_) => {
                self.publish(failure_status(&ApiFailure::new(ApiIssue::InvalidResponse)));
                return Ok(false);
            }
        };

        let view = match self.api.get_device(&binding, cancel).await {
            Ok(view) => view,
            Err(failure) => {
                self.publish(failure_status(&failure));
                return Ok(false);
            }
        };
        if view.user_id != binding.user_id() || view.device_id != binding.device_id() {
            self.publish(failure_status(&ApiFailure::new(ApiIssue::InvalidResponse)));
            return Ok(false);
        }
        if cancel.is_cancelled() {
            self.publish(ClientStatus::new(ClientPhase::Stopped));
            return Err(ClientError::Cancelled);
        }

        match self.credentials.save(&binding, cancel).await {
            Ok(()) => {}
            Err(ClientError::Cancelled) => {
                self.publish(ClientStatus::new(ClientPhase::Stopped));
                return Err(ClientError::Cancelled);
            }
            Err(_) => {
                self.publish(ClientStatus::new(ClientPhase::StorageError));
                return Ok(false);
            }
        }

        self.start_worker(binding, Some(receipt.summary.pending_task_count));
        Ok(true)
    }

    pub async fn unbind(&self, cancel: &CancellationToken) -> Result<bool, ClientError> {
        let _guard = self.lock_lifecycle(cancel).await?;
        if self.disposed.load(Ordering::SeqCst) {
            return Err(ClientError::Cancelled);
        }
        self.stop_worker().await;

        let binding = match self.credentials.read(cancel).await {
            Ok(binding) => binding,
            Err(ClientError::Cancelled) => return Err(ClientError::Cancelled),
            Err(_) => {
                self.publish(ClientStatus::new(ClientPhase::StorageError));
                return Ok(false);
            }
        };

        if let Some(binding) = binding {
            if let Err(failure) = self.api.revoke(&binding, cancel).await
                && failure.issue != ApiIssue::CredentialInvalid
            {
                self.publish(failure_status(&failure));
                return Ok(false);
            }
            match self.credentials.remove(cancel).await {
                Ok(()) => {}
                Err(ClientError::Cancelled) => return Err(ClientError::Cancelled),
                Err(_) => {
                    self.publish(ClientStatus::new(ClientPhase::StorageError));
                    return Ok(false);
                }
            }
        }

        *lock(&self.home) = None;
        self.publish(ClientStatus::new(ClientPhase::Unconfigured));
        Ok(true)
    }

    pub async fn set_paused(
        self: &Arc<Self>,
        value: bool,
        cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        self.paused.store(value, Ordering::SeqCst);
        if value && self.status().phase == ClientPhase::NotificationUnavailable {
            return self.start(cancel).await;
        }

        let next = {
            let mut status = lock(&self.status);
            if matches!(
                status.phase,
                ClientPhase::Connected | ClientPhase::LocallyPaused
            ) {
                status.phase = if value {
                    ClientPhase::LocallyPaused
                } else {
                    ClientPhase::Connected
                };
                Some(status.clone())
            } else {
                None
            }
        };
        if let Some(next) = next
            && let Some(progress) = &self.progress
        {
            progress(next);
        }
        Ok(())
    }

    pub async fn stop(&self, cancel: &CancellationToken) -> Result<(), ClientError> {
        let _guard = self.lock_lifecycle(cancel).await?;
        if !self.disposed.load(Ordering::SeqCst) {
            self.publish(ClientStatus::new(ClientPhase::Stopping));
            self.stop_worker().await;
            self.publish(ClientStatus::new(ClientPhase::Stopped));
        }
        Ok(())
    }

    pub async fn dispose(&self) {
        if self.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _guard = self.lifecycle.lock().await;
        self.publish(ClientStatus::new(ClientPhase::Stopping));
        self.stop_worker().await;
        self.publish(ClientStatus::new(ClientPhase::Stopped));
    }

    async fn lock_lifecycle<'a>(
        &'a self,
        cancel: &CancellationToken,
    ) -> Result<tokio::sync::MutexGuard<'a, ()>, ClientError> {
        tokio::select! {
            guard = self.lifecycle.lock() => Ok(guard),
            _ = cancel.cancelled() => Err(ClientError::Cancelled),
        }
    }

    fn worker_running(&self) -> bool {
        lock(&self.worker)
            .handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }

    fn start_worker(self: &Arc<Self>, binding: Binding, pending_task_count: Option<i32>) {
        *lock(&self.home) = Some(binding.origin().clone());
        self.publish(
            ClientStatus::new(ClientPhase::Binding).with_pending_optional(pending_task_count),
        );

        let cancel = CancellationToken::new();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let this = Arc::clone(self);
        let worker_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            this.run_worker(binding, generation, worker_cancel).await;
        });

        let mut worker = lock(&self.worker);
        worker.cancel = Some(cancel);
        worker.handle = Some(handle);
    }

    async fn stop_worker(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let (cancel, handle) = {
            let mut worker = lock(&self.worker);
            (worker.cancel.take(), worker.handle.take())
        };
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }

    async fn run_worker(
        self: Arc<Self>,
        binding: Binding,
        generation: u64,
        cancel: CancellationToken,
    ) {
        let weak: Weak<Self> = Arc::downgrade(&self);
        let pause_weak = weak.clone();
        let publish_weak = weak;
        let publish_cancel = cancel.clone();
        let is_paused: PauseFn = Arc::new(move || {
            pause_weak
                .upgrade()
                .is_some_and(|client| client.is_paused())
        });
        let publish: PublishFn = Arc::new(move |status| {
            if let Some(client) = publish_weak.upgrade() {
                client.publish_update(generation, &publish_cancel, status);
            }
        });

        let mut session = ReceptionSession::new(
            Arc::new(binding),
            self.api.clone(),
            self.state.clone(),
            self.notifier.clone(),
            self.clock.clone(),
            is_paused,
            publish,
        );
        let mut failures = 0usize;

        loop {
            if cancel.is_cancelled() {
                return;
            }
            let delay = match session.step(&cancel).await {
                Ok(()) => {
                    failures = 0;
                    if session.has_more() {
                        Duration::from_secs(1)
                    } else {
                        self.jittered(session.poll_seconds())
                    }
                }
                Err(ClientError::Api(failure)) => {
                    let status = failure_status(&failure);
                    if !failure.issue.is_retryable() {
                        self.publish_update(generation, &cancel, status);
                        return;
                    }

                    let mut delay = self.next_backoff(&mut failures);
                    if failure.issue == ApiIssue::ServerDisabled {
                        delay = Duration::from_secs(300);
                    }
                    if let Some(retry_after) = failure.retry_after
                        && retry_after > delay
                    {
                        delay = retry_after;
                    }
                    self.publish_update(generation, &cancel, status.with_retry_in(delay));
                    session.require_device_check();
                    delay
                }
                Err(ClientError::Notification(result)) => {
                    if result != super::contracts::DeliveryResult::RetryableFailure {
                        self.publish_update(
                            generation,
                            &cancel,
                            ClientStatus::new(ClientPhase::NotificationUnavailable),
                        );
                        return;
                    }
                    let delay = self.next_backoff(&mut failures);
                    self.publish_update(
                        generation,
                        &cancel,
                        ClientStatus::new(ClientPhase::NotificationUnavailable)
                            .with_retry_in(delay),
                    );
                    session.require_device_check();
                    delay
                }
                Err(ClientError::Storage(_)) => {
                    self.publish_update(
                        generation,
                        &cancel,
                        ClientStatus::new(ClientPhase::StorageError),
                    );
                    return;
                }
                Err(ClientError::Cancelled) => return,
            };

            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = cancel.cancelled() => return,
            }
        }
    }

    fn publish(&self, next: ClientStatus) {
        *lock(&self.status) = next.clone();
        if let Some(progress) = &self.progress {
            progress(next);
        }
    }

    fn publish_update(&self, generation: u64, cancel: &CancellationToken, next: ClientStatus) {
        if self.generation.load(Ordering::SeqCst) != generation || cancel.is_cancelled() {
            return;
        }

        let merged = {
            let mut status = lock(&self.status);
            let merged = ClientStatus {
                phase: next.phase,
                issue: next.issue,
                retry_in: next.retry_in,
                pending_task_count: next.pending_task_count.or(status.pending_task_count),
                recovery: next.recovery.or(status.recovery),
                delivery: next.delivery,
                last_sync_at: next.last_sync_at.or(status.last_sync_at),
                account_id: next.account_id.or(status.account_id),
                device_name: next.device_name.or_else(|| status.device_name.clone()),
            };
            *status = merged.clone();
            merged
        };
        if let Some(progress) = &self.progress {
            progress(merged);
        }
    }

    fn next_backoff(&self, failures: &mut usize) -> Duration {
        let index = (*failures).min(BACKOFF_SECONDS.len() - 1);
        let delay = self.jittered(BACKOFF_SECONDS[index] as i32);
        *failures = (*failures + 1).min(BACKOFF_SECONDS.len() - 1);
        delay
    }

    fn jittered(&self, seconds: i32) -> Duration {
        let factor = 1.0 + (2.0 * (self.jitter)() - 1.0) * POLL_JITTER_FRACTION;
        Duration::from_secs_f64((f64::from(seconds) * factor).max(0.0))
    }
}

fn failure_status(failure: &ApiFailure) -> ClientStatus {
    let phase = match failure.issue {
        ApiIssue::CredentialInvalid => ClientPhase::CredentialInvalid,
        ApiIssue::Offline => ClientPhase::Offline,
        ApiIssue::RateLimited | ApiIssue::ServerUnavailable => ClientPhase::Backoff,
        ApiIssue::ServerDisabled => ClientPhase::ServerDisabled,
        ApiIssue::Certificate => ClientPhase::CertificateError,
        ApiIssue::UnsupportedVersion => ClientPhase::UpgradeRequired,
        _ => ClientPhase::RequestError,
    };
    ClientStatus::new(phase).with_issue(failure.issue)
}

fn default_jitter() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| f64::from(duration.subsec_nanos() % 1_000_000) / 1_000_000.0)
        .unwrap_or(0.5)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
