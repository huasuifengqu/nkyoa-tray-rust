//! P00 冻结基线常量锚点。

use tray_core::protocol::*;

#[test]
fn protocol_version_is_frozen_v1() {
    assert_eq!(PROTOCOL_VERSION, 1);
}

#[test]
fn device_routes_are_exact_v1_paths() {
    assert_eq!(
        [
            PATH_REGISTER,
            PATH_DEVICE,
            PATH_NOTIFICATIONS,
            PATH_ACKS,
            PATH_RESYNC
        ],
        [
            "/api/desktop/v1/devices/register",
            "/api/desktop/v1/device",
            "/api/desktop/v1/notifications",
            "/api/desktop/v1/acks",
            "/api/desktop/v1/resync",
        ]
    );
    assert_eq!(PATH_DEVICE, PATH_REVOKE_SELF);
}

#[test]
fn seven_scenarios_match_frozen_keys_in_order() {
    assert_eq!(
        SCENARIO_KEYS,
        [
            "todo",
            "cc",
            "archive-cc",
            "completed",
            "workorder-close",
            "rejected",
            "workorder-deadline"
        ]
    );
}

#[test]
fn credential_prefixes_and_sequence_contract_are_frozen() {
    assert_eq!(PAIRING_CREDENTIAL_PREFIX, "nky-pair-v1");
    assert_eq!(DEVICE_CREDENTIAL_PREFIX, "nky-device-v1");
    assert_eq!(CREDENTIAL_FIELD_COUNT, 3);
    assert_eq!(SEQUENCE_FIELD_TYPE, "decimal-string");
    assert_eq!(CSRF_HEADER_NAME, "X-Requested-With");
    assert_eq!(CSRF_HEADER_VALUE, "XMLHttpRequest");
    assert_eq!(MAX_AUTHORIZATION_LENGTH, 256);
}

#[test]
fn defaults_match_the_agreed_baseline() {
    assert_eq!(MAX_DEVICES_PER_USER, 5);
    assert_eq!(POLL_INTERVAL_SECONDS, 60);
    assert_eq!(DEFAULT_PAGE_SIZE, 50);
    assert_eq!(MAX_PAGE_SIZE, 100);
    assert_eq!(RETENTION_DAYS, 30);
    assert_eq!(WORKFLOW_DISPLAY_TTL_HOURS, 24);
    assert_eq!(DEADLINE_DISPLAY_TTL_MINUTES, 60);
    assert_eq!(MAX_POPUPS_PER_PAGE, 3);
    assert_eq!(BACKOFF_SECONDS, [5, 15, 30, 60, 120, 300]);
    assert_eq!(REVOKED_DEVICE_RETENTION_DAYS, 30);
}

#[test]
fn defense_limits_match_the_agreed_baseline() {
    assert_eq!(MAX_REGISTER_BODY_BYTES, 8 * 1024);
    assert_eq!(MAX_ACK_BODY_BYTES, 2 * 1024);
    assert_eq!(MAX_FEED_RESPONSE_BYTES, 256 * 1024);
    assert_eq!(REQUEST_TIMEOUT, std::time::Duration::from_secs(15));
}

#[test]
fn https_root_is_required() {
    for input in [
        "http://oa.test",
        "https://user@oa.test",
        "https://oa.test?",
        "https://oa.test/#",
        "https://oa.test/sub",
        "https://oa.test/sub/..",
        "https://oa.test\\",
        "https://oa.test:0",
        "  ",
    ] {
        assert!(
            tray_core::ServerOrigin::parse(input).is_err(),
            "应拒绝 {input}"
        );
    }

    let origin = tray_core::ServerOrigin::parse("OA.example.test").unwrap();
    assert_eq!(origin.as_str(), "https://oa.example.test/");
    let origin = tray_core::ServerOrigin::parse("https://oa.example.test:8443/").unwrap();
    assert_eq!(origin.as_str(), "https://oa.example.test:8443/");
}
