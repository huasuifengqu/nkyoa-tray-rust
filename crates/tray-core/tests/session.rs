//! 接收状态机故障注入：重启恢复、撤销、暂停、过期与弹窗合并。

use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tray_core::ServerOrigin;
use tray_core::protocol::dto::{
    DeviceView, Feed, FeedItem, PendingSummary, RegisterReceipt, RegisterRequest, ResyncReceipt,
    Seq,
};
use tray_core::protocol::{ApiFailure, ApiIssue, Binding, NotificationApi, PairingCode};
use tray_core::reception::session::{Clock, ReceptionSession};
use tray_core::reception::state::{ClientState, StateRead};
use tray_core::{
    ClientError, ClientStateStore, ClientStatus, CredentialStore, DeliveryReport, DeliveryResult,
    DeviceRegistration, DeviceToken, Popup, RecoveryReason, StoreError, SystemNotifier,
};
use uuid::Uuid;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
}

const USER: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0002);
const DEVICE: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0003);

fn bound() -> Binding {
    Binding::new(
        ServerOrigin::parse("oa.example.test").unwrap(),
        USER,
        DEVICE,
        DeviceToken::parse(&format!(
            "nky-device-v1.{}.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            DEVICE.as_simple()
        ))
        .unwrap(),
    )
    .unwrap()
}

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        now()
    }
}

struct RigData {
    state: Option<ClientState>,
    corrupt: bool,
    paused: bool,
    revoked: bool,
    expired: bool,
    watermark: i64,
    event_count: i64,
    result: DeliveryResult,
    calls: Vec<String>,
    popup_sizes: Vec<i32>,
    notifications: i32,
    status: Option<ClientStatus>,
    initial: i64,
    fault: Option<&'static str>,
    registration_ids: Vec<Uuid>,
    lose_registration: bool,
    credential_writes: i32,
    current_binding: Option<Binding>,
}

impl Default for RigData {
    fn default() -> Self {
        Self {
            state: None,
            corrupt: false,
            paused: false,
            revoked: false,
            expired: false,
            watermark: 0,
            event_count: 1,
            result: DeliveryResult::Accepted,
            calls: Vec::new(),
            popup_sizes: Vec::new(),
            notifications: 0,
            status: None,
            initial: 0,
            fault: None,
            registration_ids: Vec::new(),
            lose_registration: false,
            credential_writes: 0,
            current_binding: Some(bound()),
        }
    }
}

struct Rig {
    data: Mutex<RigData>,
    installation: Uuid,
}

impl Rig {
    fn new(fault: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            data: Mutex::new(RigData {
                fault,
                ..RigData::default()
            }),
            installation: Uuid::new_v4(),
        })
    }

    fn data(&self) -> MutexGuard<'_, RigData> {
        self.data.lock().unwrap()
    }

    fn trigger(&self, stage: &str) -> Result<(), ClientError> {
        let mut data = self.data();
        if data.fault == Some(stage) {
            data.fault = None;
            return Err(ClientError::Storage(StoreError::new("模拟中断")));
        }
        Ok(())
    }

    async fn step(self: &Arc<Self>) -> Result<(), ClientError> {
        let paused = self.clone();
        let published = self.clone();
        let mut session = ReceptionSession::new(
            Arc::new(bound()),
            self.clone(),
            self.clone(),
            self.clone(),
            Arc::new(FixedClock),
            Arc::new(move || paused.data().paused),
            Arc::new(move |status| published.data().status = Some(status)),
        );
        session.step(&CancellationToken::new()).await
    }
}

#[async_trait]
impl NotificationApi for Rig {
    async fn register(
        &self,
        _origin: &ServerOrigin,
        _code: &PairingCode,
        request: &RegisterRequest,
        _cancel: &CancellationToken,
    ) -> Result<RegisterReceipt, ApiFailure> {
        let mut data = self.data();
        data.registration_ids.push(request.installation_id);
        if data.lose_registration {
            data.lose_registration = false;
            return Err(ApiFailure::new(ApiIssue::Offline));
        }
        Ok(RegisterReceipt {
            protocol_version: 1,
            user_id: USER,
            device_id: DEVICE,
            device_token: format!(
                "nky-device-v1.{}.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                DEVICE.as_simple()
            ),
            initial_sequence: Seq(0),
            poll_interval_seconds: 60,
            max_page_size: 100,
            server_time: now(),
            summary: PendingSummary {
                pending_task_count: 0,
            },
        })
    }

    async fn get_device(
        &self,
        _binding: &Binding,
        _cancel: &CancellationToken,
    ) -> Result<DeviceView, ApiFailure> {
        let mut data = self.data();
        data.calls.push("device".to_owned());
        if data.revoked {
            return Err(ApiFailure::new(ApiIssue::CredentialInvalid));
        }
        Ok(DeviceView {
            protocol_version: 1,
            user_id: USER,
            device_id: DEVICE,
            device_name: "desktop".to_owned(),
            initial_sequence: Seq(data.initial),
            poll_interval_seconds: 60,
            max_page_size: 100,
            server_time: now(),
        })
    }

    async fn get_feed(
        &self,
        _binding: &Binding,
        after: i64,
        _limit: i32,
        _cancel: &CancellationToken,
    ) -> Result<Feed, ApiFailure> {
        let mut data = self.data();
        data.calls.push(format!("feed:{after}"));
        if data.expired || after < data.watermark {
            return Err(ApiFailure::new(ApiIssue::HistoryExpired));
        }
        let head = data.initial.max(data.event_count);
        if after > head {
            return Err(ApiFailure::new(ApiIssue::StreamReset));
        }
        let items: Vec<FeedItem> = (after + 1..=head)
            .map(|sequence| FeedItem {
                id: Uuid::new_v4(),
                sequence: Seq(sequence),
                scenario: "todo".to_owned(),
                occurred_at: now() - time::Duration::hours(1),
                expires_at: if data.event_count == 7 && sequence == 4 {
                    now() - time::Duration::minutes(1)
                } else {
                    now() + time::Duration::hours(1)
                },
                title: "OA".to_owned(),
                body: "Test".to_owned(),
                target: "oa-home".to_owned(),
                payload_version: 1,
            })
            .collect();
        let next = items.last().map_or(after, |item| item.sequence.0);
        Ok(Feed {
            protocol_version: 1,
            server_time: now(),
            head_sequence: Seq(head),
            purged_through_sequence: Seq(data.watermark),
            next_sequence: Seq(next),
            has_more: false,
            items,
            summary: PendingSummary {
                pending_task_count: 1,
            },
        })
    }

    async fn ack(
        &self,
        _binding: &Binding,
        through: i64,
        _cancel: &CancellationToken,
    ) -> Result<i64, ApiFailure> {
        self.data().calls.push(format!("ack:{through}"));
        {
            let mut data = self.data();
            if data.fault == Some("ack") {
                data.fault = None;
                return Err(ApiFailure::new(ApiIssue::Offline));
            }
        }
        Ok(through)
    }

    async fn resync(
        &self,
        _binding: &Binding,
        _cancel: &CancellationToken,
    ) -> Result<ResyncReceipt, ApiFailure> {
        let mut data = self.data();
        data.calls.push("resync".to_owned());
        data.expired = false;
        data.initial = 10;
        Ok(ResyncReceipt {
            initial_sequence: Seq(10),
            server_time: now(),
            summary: PendingSummary {
                pending_task_count: 2,
            },
        })
    }

    async fn revoke(
        &self,
        _binding: &Binding,
        _cancel: &CancellationToken,
    ) -> Result<(), ApiFailure> {
        Ok(())
    }
}

#[async_trait]
impl SystemNotifier for Rig {
    async fn show(
        &self,
        popup: &Popup,
        _cancel: &CancellationToken,
    ) -> Result<DeliveryResult, ClientError> {
        self.trigger("notify-before")?;
        let result = self.data().result;
        if result != DeliveryResult::Accepted {
            return Ok(result);
        }
        {
            let mut data = self.data();
            data.popup_sizes.push(popup.event_count);
            data.notifications += 1;
        }
        self.trigger("notify-after")?;
        Ok(DeliveryResult::Accepted)
    }
}

#[async_trait]
impl ClientStateStore for Rig {
    async fn installation_id(&self, _cancel: &CancellationToken) -> Result<Uuid, ClientError> {
        Ok(self.installation)
    }

    async fn read(
        &self,
        _binding: &Binding,
        _cancel: &CancellationToken,
    ) -> Result<StateRead, ClientError> {
        let data = self.data();
        Ok(StateRead {
            state: data.state.clone(),
            corrupt: data.corrupt,
        })
    }

    async fn save(
        &self,
        _binding: &Binding,
        state: &ClientState,
        _cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        let mut data = self.data();
        let trigger = match data.fault {
            Some("save-page") => {
                state.pending.is_some()
                    && data
                        .state
                        .as_ref()
                        .is_none_or(|previous| previous.pending.is_none())
            }
            Some("ack-after") => {
                data.state
                    .as_ref()
                    .is_some_and(|previous| previous.ack_pending)
                    && !state.ack_pending
                    && state.pending.is_some()
            }
            Some("checkpoint") => {
                state
                    .pending
                    .as_ref()
                    .is_some_and(|page| page.processed > 0)
                    || (data
                        .state
                        .as_ref()
                        .is_some_and(|previous| previous.pending.is_some())
                        && state.pending.is_none())
            }
            Some("save-resync") => {
                data.state
                    .as_ref()
                    .is_some_and(|previous| previous.resync_pending.is_some())
                    && state.resync_pending.is_none()
            }
            _ => false,
        };
        if trigger {
            data.fault = None;
            return Err(ClientError::Storage(StoreError::new("模拟中断")));
        }
        data.calls.push("save".to_owned());
        data.state = Some(state.clone());
        data.corrupt = false;
        Ok(())
    }
}

#[async_trait]
impl CredentialStore for Rig {
    async fn read(&self, _cancel: &CancellationToken) -> Result<Option<Binding>, ClientError> {
        Ok(self.data().current_binding.clone())
    }

    async fn save(
        &self,
        binding: &Binding,
        _cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        let mut data = self.data();
        data.current_binding = Some(binding.clone());
        data.credential_writes += 1;
        Ok(())
    }

    async fn remove(&self, _cancel: &CancellationToken) -> Result<(), ClientError> {
        self.data().current_binding = None;
        Ok(())
    }
}

fn assert_failed(result: Result<(), ClientError>) {
    assert!(result.is_err());
}

#[tokio::test]
async fn restart_resumes_the_durable_page() {
    for (fault, saved, accepted) in [
        ("save-page", 0, 1),
        ("ack", 1, 1),
        ("ack-after", 1, 1),
        ("notify-before", 1, 1),
        ("notify-after", 1, 2),
        ("checkpoint", 1, 2),
    ] {
        let rig = Rig::new(Some(fault));
        assert_failed(rig.step().await);
        let through = rig.data().state.as_ref().map_or(0, |state| state.through.0);
        assert_eq!(through, saved, "{fault} 保存进度不符");
        if saved == 0 {
            assert!(!rig.data().calls.contains(&"ack:1".to_owned()));
        }

        rig.data().calls.clear();
        rig.step().await.unwrap();
        let data = rig.data();
        assert_eq!(data.calls.first().map(String::as_str), Some("device"));
        assert_eq!(data.notifications, accepted, "{fault} 弹窗次数不符");
        assert_eq!(data.state.as_ref().unwrap().through.0, 1);
        assert!(data.state.as_ref().unwrap().pending.is_none());
    }
}

#[tokio::test]
async fn revoked_device_clears_pending_without_replaying_it() {
    let rig = Rig::new(Some("ack"));
    assert_failed(rig.step().await);
    rig.data().revoked = true;
    let failure = rig.step().await.unwrap_err();
    assert!(matches!(
        failure,
        ClientError::Api(ref error) if error.issue == ApiIssue::CredentialInvalid
    ));
    let data = rig.data();
    assert!(data.state.as_ref().unwrap().pending.is_none());
    assert_eq!(data.notifications, 0);
}

#[tokio::test]
async fn pause_suppresses_delivery_but_still_persists_and_acknowledges() {
    let rig = Rig::new(None);
    rig.data().paused = true;
    rig.step().await.unwrap();
    {
        let data = rig.data();
        assert_eq!(data.state.as_ref().unwrap().through.0, 1);
        assert!(data.calls.contains(&"ack:1".to_owned()));
        assert_eq!(data.notifications, 0);
    }
    rig.data().paused = false;
    rig.step().await.unwrap();
    assert_eq!(rig.data().notifications, 0);
}

#[tokio::test]
async fn expired_history_replaces_cursor_and_drains_only_new_items() {
    let rig = Rig::new(None);
    rig.data().expired = true;
    rig.step().await.unwrap();
    {
        let data = rig.data();
        assert_eq!(data.state.as_ref().unwrap().through.0, 10);
        assert!(data.state.as_ref().unwrap().pending.is_none());
    }
    rig.data().calls.clear();
    rig.step().await.unwrap();
    let data = rig.data();
    assert!(data.calls.contains(&"feed:10".to_owned()));
    assert!(!data.calls.contains(&"ack:0".to_owned()));
}

#[tokio::test]
async fn lost_resync_checkpoint_adopts_the_server_start_without_resyncing_again() {
    let rig = Rig::new(Some("save-resync"));
    rig.data().expired = true;
    assert_failed(rig.step().await);
    assert!(rig.data().state.as_ref().unwrap().resync_pending.is_some());
    rig.step().await.unwrap();
    let data = rig.data();
    assert_eq!(data.state.as_ref().unwrap().through.0, 10);
    assert_eq!(
        data.calls.iter().filter(|call| *call == "resync").count(),
        1
    );
}

#[tokio::test]
async fn stale_and_burst_events_are_suppressed_or_combined_into_three_popups() {
    let rig = Rig::new(None);
    rig.data().event_count = 7;
    rig.step().await.unwrap();
    let data = rig.data();
    assert_eq!(data.popup_sizes, [1, 1, 4]);
    assert_eq!(
        data.status.as_ref().unwrap().delivery,
        Some(DeliveryReport {
            accepted_popups: 3,
            aggregated_events: 4,
            suppressed_events: 1,
        })
    );
}

#[tokio::test]
async fn failed_notifier_leaves_an_acknowledged_page_without_claiming_delivery() {
    for result in [
        DeliveryResult::Unavailable,
        DeliveryResult::RetryableFailure,
    ] {
        let rig = Rig::new(None);
        rig.data().result = result;
        let failure = rig.step().await.unwrap_err();
        assert!(matches!(
            failure,
            ClientError::Notification(value) if value == result
        ));
        {
            let data = rig.data();
            assert!(data.state.as_ref().unwrap().pending.is_some());
            assert!(!data.state.as_ref().unwrap().ack_pending);
            assert!(
                data.status
                    .as_ref()
                    .is_none_or(|status| status.delivery.is_none())
            );
        }

        rig.data().result = DeliveryResult::Accepted;
        rig.step().await.unwrap();
        let data = rig.data();
        assert_eq!(data.calls.iter().filter(|call| *call == "ack:1").count(), 1);
        assert_eq!(data.notifications, 1);
    }
}

#[tokio::test]
async fn corruption_recovers_the_retained_prefix_without_a_head_resync() {
    let rig = Rig::new(None);
    {
        let mut data = rig.data();
        data.watermark = 4;
        data.event_count = 7;
        data.corrupt = true;
    }
    rig.step().await.unwrap();
    let data = rig.data();
    assert_eq!(data.state.as_ref().unwrap().through.0, 7);
    assert!(data.calls.contains(&"feed:4".to_owned()));
    assert!(!data.calls.contains(&"resync".to_owned()));
    assert_eq!(
        data.status.as_ref().unwrap().recovery,
        Some(RecoveryReason::DamagedStateWithHistoryExpired)
    );
}

#[tokio::test]
async fn lost_registration_response_reuses_the_persisted_installation_id() {
    use tray_core::NotificationClient;

    let rig = Rig::new(None);
    rig.data().lose_registration = true;
    let client = Arc::new(NotificationClient::new(
        rig.clone(),
        rig.clone(),
        rig.clone(),
        rig.clone(),
        None,
        Some(Arc::new(FixedClock)),
    ));
    let registration = DeviceRegistration {
        name: "desktop".to_owned(),
        platform: "windows".to_owned(),
        architecture: "x64".to_owned(),
        client_version: "1.0.0".to_owned(),
    };
    let origin = ServerOrigin::parse("oa.example.test").unwrap();
    let code = PairingCode::parse(&format!(
        "nky-pair-v1.{}.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        Uuid::new_v4().as_simple()
    ))
    .unwrap();

    assert!(
        !client
            .bind(
                origin.clone(),
                code.clone(),
                registration.clone(),
                &CancellationToken::new()
            )
            .await
            .unwrap()
    );
    assert!(
        client
            .bind(origin, code, registration, &CancellationToken::new())
            .await
            .unwrap()
    );

    {
        let data = rig.data();
        assert_eq!(data.registration_ids.len(), 2);
        assert_eq!(
            data.registration_ids
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            1
        );
        assert_eq!(data.credential_writes, 1);
    }
    client.dispose().await;
}
