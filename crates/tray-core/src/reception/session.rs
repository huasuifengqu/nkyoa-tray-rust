//! 单绑定接收状态机。
//!
//! 所有网络、弹窗与游标推进都在同一个任务里串行发生；任何一步失败都保留
//! 可恢复的中间状态，重启后从已落盘的页继续。

use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use crate::protocol::error::{ApiFailure, ApiIssue};
use crate::protocol::{Binding, NotificationApi, validation};
use crate::protocol::{DEFAULT_PAGE_SIZE, MAX_POPUPS_PER_PAGE, POLL_INTERVAL_SECONDS};

use super::contracts::{
    ClientError, ClientPhase, ClientStateStore, ClientStatus, DeliveryReport, DeliveryResult,
    Popup, RecoveryReason, SystemNotifier,
};
use super::state::{ClientState, PendingPage, ResyncReason, StateRead};

/// 可注入时钟；测试使用固定时间。
pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

pub type PauseFn = Arc<dyn Fn() -> bool + Send + Sync>;
pub type PublishFn = Arc<dyn Fn(ClientStatus) + Send + Sync>;

pub struct ReceptionSession {
    binding: Arc<Binding>,
    api: Arc<dyn NotificationApi>,
    store: Arc<dyn ClientStateStore>,
    notifier: Arc<dyn SystemNotifier>,
    clock: Arc<dyn Clock>,
    is_paused: PauseFn,
    publish: PublishFn,
    verified: Option<crate::protocol::DeviceView>,
    server_time: Option<OffsetDateTime>,
    calibrated_at: Option<OffsetDateTime>,
    recovery: Option<RecoveryReason>,
    summary: Option<i32>,
    delivery: Option<DeliveryReport>,
    has_more: bool,
}

impl ReceptionSession {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        binding: Arc<Binding>,
        api: Arc<dyn NotificationApi>,
        store: Arc<dyn ClientStateStore>,
        notifier: Arc<dyn SystemNotifier>,
        clock: Arc<dyn Clock>,
        is_paused: PauseFn,
        publish: PublishFn,
    ) -> Self {
        Self {
            binding,
            api,
            store,
            notifier,
            clock,
            is_paused,
            publish,
            verified: None,
            server_time: None,
            calibrated_at: None,
            recovery: None,
            summary: None,
            delivery: None,
            has_more: false,
        }
    }

    pub fn poll_seconds(&self) -> i32 {
        self.verified
            .as_ref()
            .map_or(POLL_INTERVAL_SECONDS, |view| view.poll_interval_seconds)
    }

    pub fn has_more(&self) -> bool {
        self.has_more
    }

    pub fn require_device_check(&mut self) {
        self.verified = None;
    }

    pub async fn step(&mut self, cancel: &CancellationToken) -> Result<(), ClientError> {
        let read = self.store.read(&self.binding, cancel).await?;
        let result = self.receive(read, cancel).await;
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }

        match result {
            Ok(()) => Ok(()),
            Err(ClientError::Api(failure)) if failure.issue == ApiIssue::CredentialInvalid => {
                let latest = self.store.read(&self.binding, cancel).await?;
                if let Some(state) = latest.state.filter(|state| state.pending.is_some()) {
                    let cleared = state.with_pending(None).with_ack_pending(false);
                    self.save(&cleared, cancel).await?;
                }
                Err(ClientError::Api(failure))
            }
            Err(error) => Err(error),
        }
    }

    async fn receive(
        &mut self,
        read: StateRead,
        cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        let mut state = read.state;
        let corrupt = read.corrupt;
        self.has_more = false;
        let result = self.receive_inner(&mut state, corrupt, cancel).await;

        if let Err(ClientError::Api(failure)) = &result
            && let Some(current) = state.as_ref()
            && current.resync_pending.is_none()
            && matches!(
                failure.issue,
                ApiIssue::HistoryExpired | ApiIssue::StreamReset
            )
        {
            let reason = if failure.issue == ApiIssue::HistoryExpired {
                RecoveryReason::HistoryExpired
            } else {
                RecoveryReason::StreamChanged
            };
            let current = state.expect("已检查非空");
            return self.resync(current, reason, cancel).await;
        }

        result
    }

    async fn receive_inner(
        &mut self,
        state: &mut Option<ClientState>,
        corrupt: bool,
        cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        if self.verified.is_none() {
            let view = self
                .api
                .get_device(&self.binding, cancel)
                .await
                .map_err(ClientError::Api)?;
            if cancel.is_cancelled() {
                return Err(ClientError::Cancelled);
            }
            let server_time = view.server_time;
            self.verified = Some(view);
            self.calibrate(server_time);
        }
        let device = self.verified.as_ref().expect("已核验设备").clone();

        let mut current = state
            .take()
            .unwrap_or_else(|| ClientState::at(device.initial_sequence.0));

        if device.initial_sequence != current.initial {
            let reason = current
                .resync_pending
                .map(ResyncReason::to_recovery)
                .unwrap_or(RecoveryReason::StreamChanged);
            let replaced = if device.initial_sequence > current.initial {
                ClientState::at(device.initial_sequence.0)
            } else {
                current
                    .clone()
                    .with_resync_pending(ResyncReason::from_recovery(reason))
            };
            self.save(&replaced, cancel).await?;
            current = replaced;
            self.report_recovery(reason);
        }

        if let Some(reason) = current.resync_pending {
            *state = Some(current.clone());
            return self.resync(current, reason.to_recovery(), cancel).await;
        }

        *state = Some(current.clone());

        if current.pending.is_some() {
            self.drain(&mut current, cancel).await?;
            *state = Some(current);
            self.connected();
            self.has_more = true;
            return Ok(());
        }

        let limit = DEFAULT_PAGE_SIZE.min(device.max_page_size);
        let through = current.through.0;
        if corrupt {
            self.report_recovery(RecoveryReason::DamagedState);
        }

        let feed = match self
            .api
            .get_feed(&self.binding, through, limit, cancel)
            .await
        {
            Ok(feed) => feed,
            Err(failure) if failure.issue == ApiIssue::HistoryExpired && corrupt => {
                if cancel.is_cancelled() {
                    return Err(ClientError::Cancelled);
                }
                self.report_recovery(RecoveryReason::DamagedStateWithHistoryExpired);
                let (after, page) = self.find_retained_page(through, limit, cancel).await?;
                current.through = crate::protocol::Seq(after);
                *state = Some(current.clone());
                page
            }
            Err(failure) => return Err(ClientError::Api(failure)),
        };

        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        let through_now = current.through.0;
        validation::page(&feed, through_now, limit).map_err(ClientError::Api)?;
        self.calibrate(feed.server_time);
        self.summary = Some(feed.summary.pending_task_count);
        self.has_more = feed.has_more;

        if !feed.items.is_empty() {
            current.through = feed.next_sequence;
            current.ack_pending = true;
            current.pending = Some(PendingPage {
                items: feed.items.clone(),
                processed: 0,
                accepted: 0,
                suppress: (self.is_paused)(),
            });
            self.save(&current, cancel).await?;
            *state = Some(current.clone());
            self.drain(&mut current, cancel).await?;
            *state = Some(current);
        } else if corrupt {
            self.save(&current, cancel).await?;
        }

        self.connected();
        Ok(())
    }

    async fn resync(
        &mut self,
        mut state: ClientState,
        reason: RecoveryReason,
        cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        self.report_recovery(reason);
        if state.resync_pending.is_none()
            && let Some(pending) = ResyncReason::from_recovery(reason)
        {
            state.resync_pending = Some(pending);
            self.save(&state, cancel).await?;
        }

        let receipt = self
            .api
            .resync(&self.binding, cancel)
            .await
            .map_err(ClientError::Api)?;
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }

        let next = ClientState::at(receipt.initial_sequence.0);
        self.save(&next, cancel).await?;
        if let Some(verified) = &mut self.verified {
            verified.initial_sequence = receipt.initial_sequence;
        }
        self.calibrate(receipt.server_time);
        self.summary = Some(receipt.summary.pending_task_count);
        self.connected();
        self.has_more = true;
        Ok(())
    }

    async fn find_retained_page(
        &mut self,
        expired: i64,
        limit: i32,
        cancel: &CancellationToken,
    ) -> Result<(i64, crate::protocol::Feed), ClientError> {
        // v1 的 410 不带水位：先寻找服务器接受的探测位置，再从清理水位重拉。
        let mut lower = expired;
        let mut upper = i64::MAX;
        let mut stride: i64 = 1;
        while lower < upper {
            let distance = upper - lower;
            let probe = lower + stride.min(distance);
            match self.api.get_feed(&self.binding, probe, limit, cancel).await {
                Ok(candidate) => {
                    if cancel.is_cancelled() {
                        return Err(ClientError::Cancelled);
                    }
                    let after = candidate.purged_through_sequence.0;
                    let page = if after == probe {
                        candidate
                    } else {
                        self.api
                            .get_feed(&self.binding, after, limit, cancel)
                            .await
                            .map_err(ClientError::Api)?
                    };
                    return Ok((after, page));
                }
                Err(failure) if failure.issue == ApiIssue::HistoryExpired => {
                    lower = probe;
                    stride = stride.min(i64::MAX / 2).saturating_mul(2);
                }
                Err(failure) if failure.issue == ApiIssue::StreamReset => {
                    upper = probe - 1;
                    stride = 1.max((upper - lower) / 2);
                }
                Err(failure) => return Err(ClientError::Api(failure)),
            }

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                _ = cancel.cancelled() => return Err(ClientError::Cancelled),
            }
        }

        // 水位可能在探测期间继续推进，从新快照重试，避免丢弃仍保留的历史。
        Err(ClientError::Api(ApiFailure::new(
            ApiIssue::ServerUnavailable,
        )))
    }

    async fn drain(
        &mut self,
        state: &mut ClientState,
        cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        let mut page = state.pending.clone().expect("待处理页存在");
        if !page.suppress && (self.is_paused)() {
            page.suppress = true;
            state.pending = Some(page.clone());
            self.save(state, cancel).await?;
        }

        if state.ack_pending {
            let acked = self
                .api
                .ack(&self.binding, state.through.0, cancel)
                .await
                .map_err(ClientError::Api)?;
            if cancel.is_cancelled() {
                return Err(ClientError::Cancelled);
            }
            if acked < state.through.0 {
                return Err(ClientError::Api(ApiFailure::new(ApiIssue::InvalidResponse)));
            }
            state.ack_pending = false;
            self.save(state, cancel).await?;
        }

        let mut accepted = 0;
        let mut aggregated = 0;
        let mut suppressed = 0;
        let mut index = page.processed.max(0) as usize;

        while index < page.items.len() {
            if cancel.is_cancelled() {
                return Err(ClientError::Cancelled);
            }
            if page.suppress || (self.is_paused)() {
                suppressed += (page.items.len() - index) as i32;
                break;
            }

            let remaining = &page.items[index..];
            let eligible = remaining.iter().filter(|item| self.is_fresh(item)).count() as i32;
            if eligible == 0 {
                suppressed += remaining.len() as i32;
                break;
            }
            if !self.is_fresh(&page.items[index]) {
                index += 1;
                suppressed += 1;
                continue;
            }

            let combine = page.accepted + accepted == MAX_POPUPS_PER_PAGE - 1 && eligible > 1;
            let popup = if combine {
                Popup {
                    title: "OA 通知汇总".to_owned(),
                    body: format!("还有 {eligible} 条 OA 通知，请打开 OA 查看。"),
                    home: Some(self.binding.origin().clone()),
                    event_count: eligible,
                    profile_id: Some(self.binding.profile_id().to_owned()),
                }
            } else {
                let item = &page.items[index];
                Popup {
                    title: item.title.clone(),
                    body: item.body.clone(),
                    home: Some(self.binding.origin().clone()),
                    event_count: 1,
                    profile_id: Some(self.binding.profile_id().to_owned()),
                }
            };

            let result = self.notifier.show(&popup, cancel).await?;
            if cancel.is_cancelled() {
                return Err(ClientError::Cancelled);
            }
            if result != DeliveryResult::Accepted {
                return Err(ClientError::Notification(result));
            }

            accepted += 1;
            if combine {
                aggregated += eligible;
                suppressed += (page.items.len() - index) as i32 - eligible;
                break;
            }

            index += 1;
            if index < page.items.len() && page.accepted + accepted < MAX_POPUPS_PER_PAGE {
                state.pending = Some(PendingPage {
                    items: page.items.clone(),
                    processed: index as i32,
                    accepted: page.accepted + accepted,
                    suppress: page.suppress,
                });
                self.save(state, cancel).await?;
            }
        }

        state.pending = None;
        self.save(state, cancel).await?;
        self.delivery = Some(DeliveryReport {
            accepted_popups: accepted,
            aggregated_events: aggregated,
            suppressed_events: suppressed,
        });
        Ok(())
    }

    fn is_fresh(&self, item: &crate::protocol::FeedItem) -> bool {
        let (Some(server_time), Some(calibrated_at)) = (self.server_time, self.calibrated_at)
        else {
            return false;
        };
        let age = (self.clock.now() - calibrated_at).max(time::Duration::ZERO);
        item.expires_at - server_time > age
    }

    async fn save(
        &self,
        state: &ClientState,
        cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        self.store.save(&self.binding, state, cancel).await
    }

    fn calibrate(&mut self, time: OffsetDateTime) {
        self.server_time = Some(time);
        self.calibrated_at = Some(self.clock.now());
    }

    fn report_recovery(&mut self, reason: RecoveryReason) {
        if self.recovery == Some(reason) {
            return;
        }
        self.recovery = Some(reason);
        (self.publish)(
            ClientStatus::new(ClientPhase::Resynchronizing)
                .with_pending_optional(self.summary)
                .with_recovery(reason),
        );
    }

    fn connected(&mut self) {
        let mut status = ClientStatus::new(if (self.is_paused)() {
            ClientPhase::LocallyPaused
        } else {
            ClientPhase::Connected
        })
        .with_pending_optional(self.summary)
        .with_last_sync_at(self.clock.now())
        .with_account(self.binding.user_id());
        if let Some(recovery) = self.recovery {
            status = status.with_recovery(recovery);
        }
        if let Some(delivery) = self.delivery {
            status = status.with_delivery(delivery);
        }
        if let Some(device) = &self.verified {
            status = status.with_device_name(device.device_name.clone());
        }
        (self.publish)(status);
    }
}
