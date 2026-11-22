//! 契约快照、黄金样本重放与错误分类。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use tray_core::protocol::*;
use tray_core::{
    Binding, DesktopApi, DeviceToken, HttpRequest, HttpResponse, HttpTransport, PairingCode,
    ServerOrigin, TransportError,
};
use uuid::Uuid;

const LARGE: i64 = 9_007_199_254_740_993;

fn contracts() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts")
}

fn golden(name: &str) -> Vec<u8> {
    std::fs::read(contracts().join("golden").join(format!("{name}.json"))).unwrap()
}

fn golden_value(name: &str) -> Value {
    serde_json::from_slice(&golden(name)).unwrap()
}

struct Reply {
    status: u16,
    content_type: String,
    retry_after: Option<String>,
    body: Vec<u8>,
}

impl Reply {
    fn ok(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: "application/json".to_owned(),
            retry_after: None,
            body,
        }
    }

    fn problem(status: u16, body: &[u8]) -> Self {
        Self {
            status,
            content_type: "application/problem+json".to_owned(),
            retry_after: None,
            body: body.to_vec(),
        }
    }
}

#[derive(Debug)]
struct Recorded {
    method: String,
    path: String,
    query: Option<String>,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Default)]
struct Stub {
    replies: Mutex<VecDeque<Reply>>,
    recorded: Mutex<Vec<Recorded>>,
}

impl Stub {
    fn new(replies: Vec<Reply>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            recorded: Mutex::new(Vec::new()),
        })
    }

    fn api(self: &Arc<Self>) -> DesktopApi {
        DesktopApi::with_transport(self.clone())
    }

    fn taken(&self) -> Vec<Recorded> {
        let mut recorded = self.recorded.lock().unwrap();
        std::mem::take(&mut *recorded)
    }
}

#[async_trait]
impl HttpTransport for Stub {
    async fn send(
        &self,
        request: HttpRequest,
        _cancel: &CancellationToken,
    ) -> Result<HttpResponse, TransportError> {
        self.recorded.lock().unwrap().push(Recorded {
            method: request.method,
            path: request.url.path().to_owned(),
            query: request.url.query().map(str::to_owned),
            headers: request.headers,
            body: request.body,
        });
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("缺少预置响应");
        Ok(HttpResponse {
            status: reply.status,
            content_type: Some(reply.content_type),
            retry_after: reply.retry_after,
            body: reply.body,
        })
    }
}

fn binding_from_golden() -> Binding {
    let value = golden_value("register");
    let data = &value["data"];
    let user = Uuid::parse_str(data["userId"].as_str().unwrap()).unwrap();
    let device = Uuid::parse_str(data["deviceId"].as_str().unwrap()).unwrap();
    let token = DeviceToken::parse(data["deviceToken"].as_str().unwrap()).unwrap();
    Binding::new(
        ServerOrigin::parse("oa.example.test").unwrap(),
        user,
        device,
        token,
    )
    .unwrap()
}

fn pairing() -> PairingCode {
    PairingCode::parse(&format!(
        "nky-pair-v1.{}.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        Uuid::new_v4().as_simple()
    ))
    .unwrap()
}

#[tokio::test]
async fn contract_hash_and_golden_requests_match_the_server_snapshot() {
    let bytes = std::fs::read(contracts().join("nkyoa-desktop-v1.openapi.json")).unwrap();
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(contracts().join("source-manifest.json")).unwrap())
            .unwrap();
    let digest = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(digest, manifest["sha256"].as_str().unwrap());
    assert_eq!(manifest["protocolVersion"], 1);

    let contract: Value = serde_json::from_slice(&bytes).unwrap();
    let paths = contract["paths"].as_object().unwrap();
    assert_eq!(paths.len(), 5);

    let stub = Stub::new(vec![
        Reply::ok(golden("register")),
        Reply::ok(golden("device")),
        Reply::ok(golden("feed")),
        Reply::ok(golden("ack")),
        Reply::problem(410, &golden("history-expired")),
        Reply::ok(golden("resync")),
        Reply::ok(golden("revoke")),
    ]);
    let api = stub.api();

    let origin = ServerOrigin::parse("OA.example.test/").unwrap();
    let installation = Uuid::new_v4();
    let code = pairing();
    let receipt = api
        .register(
            &origin,
            &code,
            &RegisterRequest::new(installation, " 办公电脑 ", "windows", "x64", "1.0.0"),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(receipt.device_id, binding_from_golden().device_id());

    let binding = binding_from_golden();
    let device = api
        .get_device(&binding, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(device.device_id, binding.device_id());

    let feed = api
        .get_feed(&binding, LARGE - 1, 50, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(feed.items.len(), 1);
    assert_eq!(feed.items[0].sequence, Seq(LARGE));

    let acked = api
        .ack(&binding, LARGE, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(acked, LARGE);

    let expired = api
        .get_feed(&binding, 0, 50, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(expired.issue, ApiIssue::HistoryExpired);

    let resync = api
        .resync(&binding, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(resync.initial_sequence, Seq(LARGE));

    api.revoke(&binding, &CancellationToken::new())
        .await
        .unwrap();

    let calls = stub.taken();
    assert_eq!(calls.len(), 7);
    let methods = calls
        .iter()
        .map(|call| call.method.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        methods,
        ["POST", "GET", "GET", "POST", "GET", "POST", "DELETE"]
    );
    let expected_paths = [
        PATH_REGISTER,
        PATH_DEVICE,
        PATH_NOTIFICATIONS,
        PATH_ACKS,
        PATH_NOTIFICATIONS,
        PATH_RESYNC,
        PATH_REVOKE_SELF,
    ];
    for (call, path) in calls.iter().zip(expected_paths) {
        assert_eq!(call.path, path);
        assert_eq!(call.header("Cookie"), None);
        if call.method == "GET" {
            assert_eq!(call.header(CSRF_HEADER_NAME), None);
        } else {
            assert_eq!(call.header(CSRF_HEADER_NAME), Some(CSRF_HEADER_VALUE));
        }
        if call.body.is_some() {
            // 服务端端点带 JSON 内容类型约束：缺 Content-Type 会匹配不到端点而落兜底 404。
            assert_eq!(call.header("Content-Type"), Some(JSON_CONTENT_TYPE));
        }
        let authorization = call.header("Authorization").unwrap();
        assert!(authorization.starts_with("Bearer "));

        let method_key = call.method.to_lowercase();
        let security = &contract["paths"][path][method_key.as_str()]["security"][0];
        let scheme = if path == PATH_REGISTER {
            "pairingAuth"
        } else {
            "deviceAuth"
        };
        assert!(security.get(scheme).is_some());

        if path == PATH_REGISTER {
            assert!(authorization.starts_with("Bearer nky-pair-v1."));
            let body: Value = serde_json::from_slice(call.body.as_ref().unwrap()).unwrap();
            assert_eq!(body["deviceName"], "办公电脑");
            assert_eq!(body["clientVersion"], "1.0.0");
            // v1 新客户端声明预览能力；服务端只对声明的设备下发流程标题/流转意见
            assert_eq!(body["previewCapable"], true);
            assert_eq!(
                Uuid::parse_str(body["installationId"].as_str().unwrap()).unwrap(),
                installation
            );
        } else {
            assert!(authorization.starts_with("Bearer nky-device-v1."));
        }
    }

    assert_eq!(
        calls[2].query.as_deref(),
        Some("afterSequence=9007199254740992&limit=50")
    );
    assert_eq!(
        std::str::from_utf8(calls[3].body.as_ref().unwrap()).unwrap(),
        "{\"receivedThroughSequence\":\"9007199254740993\"}"
    );
    assert_eq!(
        std::str::from_utf8(calls[5].body.as_ref().unwrap()).unwrap(),
        "{}"
    );

    assert!(!format!("{receipt:?}").contains("AAAA"));
    assert!(!format!("{code:?}").contains("AAAA"));
}

#[tokio::test]
async fn failure_matrix_uses_status_and_code_without_server_text() {
    let binding = binding_from_golden();
    let cases = [
        (401, "", ApiIssue::CredentialInvalid),
        (403, "CSRF_REQUIRED", ApiIssue::Csrf),
        (409, "NOTIFY_DEVICE_LIMIT", ApiIssue::DeviceLimit),
        (409, "VERSION_CONFLICT", ApiIssue::VersionConflict),
        (409, "NOTIFY_STREAM_RESET_REQUIRED", ApiIssue::StreamReset),
        (410, "NOTIFY_HISTORY_EXPIRED", ApiIssue::HistoryExpired),
        (422, "VALIDATION_FAILED", ApiIssue::Validation),
        (429, "RATE_LIMITED", ApiIssue::RateLimited),
        (
            503,
            "DESKTOP_NOTIFICATIONS_DISABLED",
            ApiIssue::ServerDisabled,
        ),
        (503, "", ApiIssue::ServerUnavailable),
        (502, "", ApiIssue::ServerUnavailable),
        (302, "", ApiIssue::InvalidResponse),
    ];

    for (status, code, issue) in cases {
        let body = serde_json::to_vec(&json!({
            "code": code,
            "detail": "sensitive server error text"
        }))
        .unwrap();
        let mut reply = Reply::problem(status, &body);
        reply.retry_after = Some("35".to_owned());
        let stub = Stub::new(vec![reply]);
        let failure = stub
            .api()
            .get_device(&binding, &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(failure.issue, issue, "状态 {status} 分类不应漂移");
        assert!(!failure.to_string().contains("sensitive"));
        if status == 429 {
            assert_eq!(failure.retry_after, Some(Duration::from_secs(35)));
        }
    }
}

#[tokio::test]
async fn invalid_page_cannot_advance_the_cursor() {
    let binding = binding_from_golden();
    let cases: Vec<(&str, ApiIssue)> = vec![
        ("head", ApiIssue::InvalidResponse),
        ("watermark", ApiIssue::InvalidResponse),
        ("next", ApiIssue::InvalidResponse),
        ("number", ApiIssue::InvalidResponse),
        ("overflow", ApiIssue::InvalidResponse),
        ("duplicate", ApiIssue::InvalidResponse),
        ("out-of-order", ApiIssue::InvalidResponse),
        ("gap", ApiIssue::InvalidResponse),
        ("unknown-scenario", ApiIssue::InvalidResponse),
        ("unknown-target", ApiIssue::InvalidResponse),
        ("payload-version", ApiIssue::UnsupportedVersion),
        ("protocol-version", ApiIssue::UnsupportedVersion),
        ("missing", ApiIssue::InvalidResponse),
        ("null-item", ApiIssue::InvalidResponse),
        ("wrong-type", ApiIssue::InvalidResponse),
        ("too-large", ApiIssue::InvalidResponse),
    ];

    for (mutation, expected) in cases {
        let mut document = golden_value("feed");
        let data = document["data"].as_object_mut().unwrap();
        match mutation {
            "missing" => {
                data.remove("items");
            }
            "protocol-version" => data["protocolVersion"] = json!(2),
            _ => {
                let items = data["items"].as_array_mut().unwrap();
                match mutation {
                    "head" => data["headSequence"] = json!(format!("{}", LARGE - 1)),
                    "watermark" => data["purgedThroughSequence"] = json!(format!("{LARGE}")),
                    "next" => data["nextSequence"] = json!(format!("{}", LARGE - 1)),
                    "number" => items[0]["sequence"] = json!(LARGE),
                    "overflow" => items[0]["sequence"] = json!("9223372036854775808"),
                    "duplicate" => items.push(items[0].clone()),
                    "out-of-order" => {
                        let mut duplicate = items[0].clone();
                        duplicate["sequence"] = json!(format!("{}", LARGE - 1));
                        items.push(duplicate);
                    }
                    "gap" => items[0]["sequence"] = json!(format!("{}", LARGE + 1)),
                    "unknown-scenario" => items[0]["scenario"] = json!("bad"),
                    "unknown-target" => items[0]["target"] = json!("https://elsewhere.example/"),
                    "payload-version" => items[0]["payloadVersion"] = json!(2),
                    "null-item" => items[0] = Value::Null,
                    "too-large" => items[0]["body"] = json!("a".repeat(MAX_FEED_RESPONSE_BYTES)),
                    "wrong-type" => {}
                    _ => unreachable!(),
                }
            }
        }

        let mut reply = Reply::ok(serde_json::to_vec(&document).unwrap());
        if mutation == "wrong-type" {
            reply.content_type = "text/plain".to_owned();
        }
        let stub = Stub::new(vec![reply]);
        let failure = stub
            .api()
            .get_feed(&binding, LARGE - 1, 50, &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(failure.issue, expected, "变异 {mutation} 分类错误");
    }
}
