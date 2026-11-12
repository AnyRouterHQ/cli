//! `anyr relay devices` and `relay models --devices` against a local scripted server.

mod common;
use common::{run, serve, KEY};
use std::time::Duration;

const LIST: &str = r#"{"object":"list","data":[{"id":"dev_1","name":"Studio Mac","status":"active","connected":true,"pool_enabled":true,"models":["apple/foundation-model"],"last_seen_at":1}]}"#;

#[test]
fn list_hits_devices_with_the_key_and_prints_a_row() {
    let (base, rx) = serve(vec![(200, LIST.into())]);
    let (code, out, err) = run(&base, &["relay", "devices"], None);
    assert_eq!(code, 0, "{out}{err}");
    let seen = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        (seen.method.as_str(), seen.path.as_str()),
        ("GET", "/api/v1/relay/devices")
    );
    assert_eq!(seen.auth, format!("Bearer {KEY}"));
    for want in [
        "dev_1",
        "Studio Mac",
        "online",
        "apple/foundation-model",
        "on",
    ] {
        assert!(out.contains(want), "missing {want}: {out}");
    }
}

#[test]
fn json_prints_the_raw_body() {
    let (base, _rx) = serve(vec![(200, LIST.into())]);
    let (code, out, _) = run(&base, &["relay", "devices", "--json"], None);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["data"][0]["id"], "dev_1");
    assert_eq!(v["data"][0]["last_seen_at"], 1);
}

#[test]
fn rename_patches_name_only() {
    let (base, rx) = serve(vec![(200, r#"{"id":"dev_1","name":"Studio Mac"}"#.into())]);
    let (code, out, err) = run(
        &base,
        &["relay", "devices", "rename", "dev_1", "Studio Mac"],
        None,
    );
    assert_eq!(code, 0, "{out}{err}");
    let seen = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        (seen.method.as_str(), seen.path.as_str()),
        ("PATCH", "/api/v1/relay/devices/dev_1")
    );
    let body: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(body, serde_json::json!({"name": "Studio Mac"}));
}

#[test]
fn pool_off_patches_a_boolean_pool_enabled() {
    let (base, rx) = serve(vec![(200, r#"{"id":"dev_1","pool_enabled":false}"#.into())]);
    let (code, out, err) = run(&base, &["relay", "devices", "pool", "dev_1", "off"], None);
    assert_eq!(code, 0, "{out}{err}");
    let seen = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(seen.method, "PATCH");
    let body: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(body, serde_json::json!({"pool_enabled": false}));
}

#[test]
fn rm_without_yes_sends_nothing_and_exits_6() {
    let (base, rx) = serve(vec![(200, r#"{"id":"dev_1","status":"revoked"}"#.into())]);
    let (code, out, err) = run(&base, &["relay", "devices", "rm", "dev_1"], None);
    assert_eq!(code, 6, "{out}{err}");
    assert!(err.contains("--yes"), "{err}");
    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "no request may be sent"
    );
}

#[test]
fn rm_with_yes_sends_delete() {
    let (base, rx) = serve(vec![(200, r#"{"id":"dev_1","status":"revoked"}"#.into())]);
    let (code, out, err) = run(&base, &["relay", "devices", "rm", "dev_1", "--yes"], None);
    assert_eq!(code, 0, "{out}{err}");
    let seen = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        (seen.method.as_str(), seen.path.as_str()),
        ("DELETE", "/api/v1/relay/devices/dev_1")
    );
    assert!(out.contains("Removed dev_1"), "{out}");
}

#[test]
fn models_devices_aggregates_per_model() {
    let (base, rx) = serve(vec![(200, LIST.into())]);
    let (code, out, err) = run(&base, &["relay", "models", "--devices"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap().path,
        "/api/v1/relay/devices"
    );
    assert!(out.contains("apple/foundation-model"), "{out}");
}

#[test]
fn not_found_exits_nonzero_on_stderr() {
    let (base, _rx) = serve(vec![(
        404,
        r#"{"error":{"message":"Device not found"}}"#.into(),
    )]);
    let (code, out, err) = run(&base, &["relay", "devices", "pool", "nope", "on"], None);
    assert_ne!(code, 0);
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("Device not found"), "{err}");
}

#[test]
fn help_lists_the_device_verbs() {
    let (base, _rx) = serve(vec![]);
    let (code, out, _) = run(&base, &["relay", "--help"], None);
    assert_eq!(code, 0);
    for want in [
        "relay devices rename",
        "relay devices pool",
        "relay devices rm",
        "models --devices",
    ] {
        assert!(out.contains(want), "missing {want}: {out}");
    }
}
