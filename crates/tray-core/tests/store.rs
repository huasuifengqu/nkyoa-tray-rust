//! 接收状态持久化与安装标识并发。

use std::collections::HashSet;
use std::path::Path;

use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tray_core::protocol::Binding;
use tray_core::protocol::dto::{FeedItem, Seq};
use tray_core::reception::contracts::{ClientError, ClientStateStore};
use tray_core::reception::state::{ClientState, PERSISTED_BODY, PERSISTED_TITLE, PendingPage};
use tray_core::reception::store::JsonClientStateStore;
use tray_core::{DeviceToken, ServerOrigin};
use uuid::Uuid;

const TOKEN_SECRET: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn binding(device: Uuid) -> Binding {
    Binding::new(
        ServerOrigin::parse("oa.example.test").unwrap(),
        Uuid::new_v4(),
        device,
        DeviceToken::parse(&format!(
            "nky-device-v1.{}.{TOKEN_SECRET}",
            device.as_simple()
        ))
        .unwrap(),
    )
    .unwrap()
}

fn state_path(root: &Path, binding: &Binding) -> std::path::PathBuf {
    root.join(binding.profile_id()).join("state.json")
}

#[test]
fn installation_id_is_stable_and_concurrent_creation_has_one_winner() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let mut handles = Vec::new();
    for _ in 0..8 {
        let root = root.clone();
        handles.push(std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            let store = JsonClientStateStore::new(root);
            runtime
                .block_on(store.installation_id(&CancellationToken::new()))
                .unwrap()
        }));
    }

    let ids: HashSet<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(ids.len(), 1);
    assert!(!ids.iter().next().unwrap().is_nil());

    let store = JsonClientStateStore::new(root);
    let first = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(store.installation_id(&CancellationToken::new()))
        .unwrap();
    assert_eq!(first, *ids.iter().next().unwrap());
}

#[tokio::test]
async fn state_is_partitioned_atomic_secret_free_and_restorable() {
    let directory = tempfile::tempdir().unwrap();
    let store = JsonClientStateStore::new(directory.path());
    let device = Uuid::new_v4();
    let bound = binding(device);
    let cancel = CancellationToken::new();

    assert!(store.read(&bound, &cancel).await.unwrap().state.is_none());

    const CURSOR: i64 = 9_007_199_254_740_993;
    let start = ClientState::at(CURSOR - 1);
    store.save(&bound, &start, &cancel).await.unwrap();

    let now = OffsetDateTime::from_unix_timestamp(86_400).unwrap();
    let mut advanced = start.clone();
    advanced.through = Seq(CURSOR);
    let expected = advanced
        .with_pending(Some(PendingPage {
            items: vec![FeedItem {
                id: Uuid::new_v4(),
                sequence: Seq(CURSOR),
                scenario: "todo".to_owned(),
                occurred_at: now,
                expires_at: now + time::Duration::hours(1),
                title: "关于开展年度档案整理工作的通知".to_owned(),
                body: "关于开展年度档案整理工作的通知\n张三：请按制度办理".to_owned(),
                target: "oa-home".to_owned(),
                payload_version: 1,
            }],
            processed: 0,
            accepted: 0,
            suppress: false,
        }))
        .with_ack_pending(true);
    store.save(&bound, &expected, &cancel).await.unwrap();

    let restored = JsonClientStateStore::new(directory.path())
        .read(&bound, &cancel)
        .await
        .unwrap()
        .state
        .unwrap();
    assert_eq!(restored.through.0, CURSOR);
    // 预览文案只允许在内存与系统通知路径出现：落盘条目必须换成通用提示
    let restored_item = &restored.pending.as_ref().unwrap().items[0];
    assert_eq!(restored_item.sequence, Seq(CURSOR));
    assert_eq!(restored_item.title, PERSISTED_TITLE);
    assert_eq!(restored_item.body, PERSISTED_BODY);
    let raw = std::fs::read(state_path(directory.path(), &bound)).unwrap();
    let text = String::from_utf8(raw).unwrap();
    assert!(!text.contains("年度档案整理"), "落盘文件泄漏了流程标题");
    assert!(!text.contains("请按制度办理"), "落盘文件泄漏了流转意见");

    let other = binding(Uuid::new_v4());
    assert!(store.read(&other, &cancel).await.unwrap().state.is_none());

    let state_file = state_path(directory.path(), &bound);
    let raw = std::fs::read_to_string(&state_file).unwrap();
    assert!(!raw.contains(TOKEN_SECRET));
    assert!(raw.contains("\"through\":\"9007199254740993\""));
    assert!(
        std::fs::read_dir(state_file.parent().unwrap())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
    );

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let result = store.save(&bound, &start, &cancelled).await;
    assert!(matches!(result, Err(ClientError::Cancelled)));
    assert_eq!(
        store
            .read(&bound, &cancel)
            .await
            .unwrap()
            .state
            .unwrap()
            .through
            .0,
        CURSOR
    );
}

#[tokio::test]
async fn corruption_is_reported_without_copying_raw_content() {
    let directory = tempfile::tempdir().unwrap();
    let store = JsonClientStateStore::new(directory.path());
    let bound = binding(Uuid::new_v4());
    let cancel = CancellationToken::new();
    store
        .save(&bound, &ClientState::at(13), &cancel)
        .await
        .unwrap();
    let file = state_path(directory.path(), &bound);

    for corrupted in [
        format!("nky-device-v1.{TOKEN_SECRET}"),
        "{\"version\":1,\"initial\":\"13\",\"through\":\"12\",\"ackPending\":false,\"pending\":null,\"resyncPending\":null}".to_owned(),
        "x".repeat(1024 * 1024 + 1),
    ] {
        std::fs::write(&file, corrupted).unwrap();
        let read = store.read(&bound, &cancel).await.unwrap();
        assert!(read.corrupt);
        assert!(read.state.is_none());
        let marker = std::fs::read_to_string(format!("{}.corrupt.json", file.display())).unwrap();
        assert_eq!(marker, "{\"reason\":\"invalid-state\"}\n");
    }
}

#[tokio::test]
async fn future_state_version_is_never_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    let store = JsonClientStateStore::new(directory.path());
    let bound = binding(Uuid::new_v4());
    let cancel = CancellationToken::new();
    store
        .save(&bound, &ClientState::at(0), &cancel)
        .await
        .unwrap();
    let file = state_path(directory.path(), &bound);
    std::fs::write(&file, "{\"version\":2}").unwrap();

    let result = store.read(&bound, &cancel).await;
    assert!(matches!(
        result,
        Err(ClientError::Api(ref failure))
            if failure.issue == tray_core::ApiIssue::UnsupportedVersion
    ));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"version\":2}");
}
