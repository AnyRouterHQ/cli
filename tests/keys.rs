//! `anyr keys create|show|update` scoping flags against a local scripted server.

mod common;
use common::{run, serve};
use std::time::Duration;

const LIST: &str = r#"{"data":[{"hash":"abc123def456","name":"ci-bot","label":"sk-ar-v1-abc"}]}"#;
const SECRET: &str = "sk-ar-v1-brandnewsecret0001";

fn created() -> (u16, String) {
    (
        201,
        format!(r#"{{"key":"{SECRET}","data":{{"hash":"h2"}}}}"#),
    )
}

#[test]
fn create_sends_scope_fields_and_quiet_prints_only_the_secret() {
    // WHY: `KEY=$(anyr keys create ... --quiet)` must capture just the secret,
    // and the body must carry the scope the user asked for.
    let (base, rx) = serve(vec![created()]);
    let (code, out, err) = run(
        &base,
        &[
            "keys",
            "create",
            "ci-bot",
            "--limit",
            "25",
            "--reset",
            "monthly",
            "--endpoints",
            "chat,embeddings",
            "--expires",
            "30d",
            "--quiet",
        ],
        None,
    );
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(out.trim(), SECRET);
    assert!(!err.contains(SECRET), "{err}");
    let seen = rx.recv().unwrap();
    assert_eq!(
        (seen.method.as_str(), seen.path.as_str()),
        ("POST", "/api/v1/keys")
    );
    let b: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(b["name"], "ci-bot");
    assert_eq!(b["limit"], 25.0);
    assert_eq!(b["limit_reset"], "monthly");
    assert_eq!(
        b["allowed_endpoints"],
        serde_json::json!([
            "/api/v1/chat/completions",
            "/api/v1/messages",
            "/api/v1/responses",
            "/api/v1/embeddings"
        ])
    );
    assert!(b["expires_at"].as_str().unwrap().ends_with('Z'), "{b}");
    assert!(b.get("disabled").is_none());
}

#[test]
fn create_json_prints_one_object_and_does_not_prompt() {
    // WHY: #253 — scripts parse stdout as JSON; text broke them.
    let (base, _rx) = serve(vec![created()]);
    let (code, out, err) = run(&base, &["keys", "create", "ci", "--json"], None);
    assert_eq!(code, 0, "{out}{err}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["name"], "ci");
    assert_eq!(v["hash"], "h2");
    assert_eq!(v["key"], SECRET);
    assert_eq!(v["agent"], serde_json::Value::Null);
    assert_eq!(v["saved"], false);
    assert!(v["account"].as_str().is_some(), "{v}");
}

#[test]
fn create_without_flags_sends_only_the_name() {
    let (base, rx) = serve(vec![created()]);
    let (code, out, err) = run(&base, &["keys", "create", "plain", "--quiet"], None);
    assert_eq!(code, 0, "{out}{err}");
    let b: serde_json::Value = serde_json::from_str(&rx.recv().unwrap().body).unwrap();
    assert_eq!(b, serde_json::json!({"name": "plain"}));
}

#[test]
fn bad_scope_flags_are_exit_2_and_send_nothing() {
    let (base, rx) = serve(vec![created()]);
    for args in [
        ["keys", "create", "x", "--endpoints", "teleport"].as_slice(),
        ["keys", "create", "x", "--expires", "soon"].as_slice(),
        ["keys", "create", "x", "--limit", "-3"].as_slice(),
        ["keys", "create", "x", "--reset", "hourly"].as_slice(),
    ] {
        let (code, _, err) = run(&base, args, None);
        assert_eq!(code, 2, "{args:?}: {err}");
    }
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn update_patches_only_given_fields_by_hash_prefix() {
    let (base, rx) = serve(vec![
        (200, LIST.into()),
        (200, r#"{"ok":true}"#.into()),
        (
            200,
            r#"{"name":"ci-bot","hash":"abc123def456","limit":50.0,"disabled":true}"#.into(),
        ),
    ]);
    let (code, out, err) = run(
        &base,
        &[
            "keys",
            "update",
            "abc123",
            "--limit",
            "50",
            "--disable",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(rx.recv().unwrap().path, "/api/v1/keys");
    let patch = rx.recv().unwrap();
    assert_eq!(
        (patch.method.as_str(), patch.path.as_str()),
        ("PATCH", "/api/v1/keys/abc123def456")
    );
    let b: serde_json::Value = serde_json::from_str(&patch.body).unwrap();
    assert_eq!(b, serde_json::json!({"limit": 50.0, "disabled": true}));
}

#[test]
fn update_with_nothing_to_change_is_a_usage_error() {
    let (base, _rx) = serve(vec![(200, LIST.into())]);
    let (code, _, err) = run(&base, &["keys", "update", "abc123"], None);
    assert_eq!(code, 2, "{err}");
}

#[test]
fn show_prints_limit_spent_endpoints_expiry() {
    let key = r#"{"name":"ci-bot","hash":"abc123def456","limit":25.0,"limit_reset":"monthly","real_usage":1.5,"limit_remaining":23.5,"allowed_endpoints":["/api/v1/embeddings"],"expires_at":"2026-11-23T01:51:16Z"}"#;
    let (base, rx) = serve(vec![(200, LIST.into()), (200, key.into())]);
    let (code, out, err) = run(&base, &["keys", "show", "abc123"], None);
    assert_eq!(code, 0, "{out}{err}");
    rx.recv().unwrap();
    assert_eq!(rx.recv().unwrap().path, "/api/v1/keys/abc123def456");
    for want in [
        "$25.00 (monthly)",
        "$1.50",
        "embeddings",
        "2026-11-23T01:51:16Z",
    ] {
        assert!(out.contains(want), "{want} in {out}");
    }
}

#[test]
fn restricted_key_exits_4() {
    let (base, _rx) = serve(vec![(403, r#"{"error":{"message":"no scope"}}"#.into())]);
    let (code, _, err) = run(&base, &["keys", "create", "x", "--quiet"], None);
    assert_eq!(code, 4, "{err}");
    assert!(err.starts_with("error:"), "{err}");
}

#[test]
fn ls_and_rm_are_hidden_aliases_of_list_and_revoke() {
    // WHY: artifact, byok and relay devices take `ls` / `rm`; keys must too,
    // so one verb works everywhere.
    let (base, rx) = serve(vec![
        (200, LIST.into()),
        (200, LIST.into()),
        (200, LIST.into()),
    ]);
    let (code, out, err) = run(&base, &["keys", "ls", "--json"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("ci-bot"), "{out}");
    assert_eq!(rx.recv().unwrap().path, "/api/v1/keys");
    // Without a TTY or --yes, `rm` refuses like `revoke` (exit 6, nothing
    // sent), and the hint is the same command with --yes.
    let (code, _, err) = run(&base, &["keys", "rm", "abc123", "--json"], None);
    assert_eq!(code, 6, "{err}");
    let v: serde_json::Value = serde_json::from_str(err.trim()).unwrap();
    assert_eq!(v["error"]["code"], "confirm_required");
    assert_eq!(v["error"]["retryable"], false);
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(
        hint.starts_with("anyr keys rm abc123 --json") && hint.ends_with(" --yes"),
        "{err}"
    );
    let (code, _, err) = run(&base, &["keys", "rm", "zzz", "--yes"], None);
    assert_eq!(code, 3, "an unknown hash is not found: {err}");
}

#[test]
fn create_if_missing_returns_the_existing_key_without_minting() {
    // WHY: an agent re-running setup must not mint a second key per run, and
    // must never get a secret back for a key that already exists.
    let (base, rx) = serve(vec![(200, LIST.into())]);
    let (code, out, err) = run(
        &base,
        &[
            "keys",
            "create",
            "--name",
            "ci-bot",
            "--if-missing",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{out}{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["created"], false);
    assert_eq!(v["hash"], "abc123def456");
    assert!(
        v.get("key").is_none(),
        "no secret for an existing key: {out}"
    );
    assert_eq!(rx.recv().unwrap().method, "GET");
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "nothing created"
    );
}

#[test]
fn create_if_missing_creates_when_no_active_key_has_the_name() {
    let disabled = r#"{"data":[{"hash":"abc123def456","name":"ci-bot","label":"sk-ar-v1-abc","disabled":true}]}"#;
    let (base, rx) = serve(vec![(200, disabled.into()), created()]);
    let (code, out, err) = run(
        &base,
        &[
            "keys",
            "create",
            "--name",
            "ci-bot",
            "--if-missing",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{out}{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["created"], true);
    assert_eq!(v["key"], SECRET);
    assert_eq!(rx.recv().unwrap().method, "GET");
    let post = rx.recv().unwrap();
    assert_eq!(post.method, "POST");
    assert!(post.body.contains("\"name\":\"ci-bot\""), "{}", post.body);
}

#[test]
fn revoke_if_exists_on_a_missing_key_is_success_and_sends_no_delete() {
    let (base, rx) = serve(vec![(200, LIST.into())]);
    let (code, out, err) = run(
        &base,
        &["keys", "revoke", "zzz", "--yes", "--if-exists", "--json"],
        None,
    );
    assert_eq!(code, 0, "{out}{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["deleted"], false);
    assert_eq!(rx.recv().unwrap().method, "GET");
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    // Without the flag a missing key is still exit 3.
    let (base, _rx) = serve(vec![(200, LIST.into())]);
    let (code, _, err) = run(&base, &["keys", "revoke", "zzz", "--yes"], None);
    assert_eq!(code, 3, "{err}");
}

#[test]
fn create_if_missing_quiet_never_prints_a_sentence_as_the_key() {
    // WHY: `KEY=$(anyr keys create … --quiet)` must not capture prose.
    let (base, _rx) = serve(vec![(200, LIST.into())]);
    let (code, out, err) = run(
        &base,
        &[
            "keys",
            "create",
            "--name",
            "ci-bot",
            "--if-missing",
            "--quiet",
        ],
        None,
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.trim().is_empty(), "{out}");
    assert!(err.contains("already exists"), "{err}");
}

#[test]
fn create_if_missing_refuses_agent_and_revoke_if_exists_keeps_server_errors() {
    let (code, _, err) = run(
        "http://127.0.0.1:9/api",
        &["keys", "create", "--agent", "claude", "--if-missing"],
        None,
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--if-missing cannot save"), "{err}");
    // A failed DELETE that is not a 404 stays a failure.
    let (base, _rx) = serve(vec![(200, LIST.into()), (500, "{}".into())]);
    let (code, _, err) = run(
        &base,
        &["keys", "revoke", "abc123", "--yes", "--if-exists"],
        None,
    );
    assert_eq!(code, 1, "{err}");
    // A 404 on the DELETE itself (revoked since the list) is already gone.
    let (base, _rx) = serve(vec![(200, LIST.into()), (404, "{}".into())]);
    let (code, out, err) = run(
        &base,
        &["keys", "revoke", "abc123", "--yes", "--if-exists", "--json"],
        None,
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("\"deleted\":false"), "{out}");
}
