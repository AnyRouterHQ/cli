//! `anyr auth status` against a local scripted server: the account summary a
//! signed-in user reads, and the exit code a script branches on.

mod common;
use common::{run, serve_routes, unique_temp_dir, KEY};
use std::process::{Command, Stdio};

const ME: &str =
    r#"{"id":"user_1","email":"me@example.com","name":null,"username":"duyet","balance":53.0794}"#;
const KEY_INFO: &str = r#"{"data":{"name":"laptop","label":"sk-ar-v1-...6789","is_free_tier":true,"last_used_at":"2026-10-27T15:31:41Z","allowed_endpoints":null,"allowed_models":null,"hash":"deadbeef"}}"#;

fn signed_in_server() -> String {
    serve_routes(vec![
        ("/api/v1/me", 200, ME.into()),
        ("/api/v1/key", 200, KEY_INFO.into()),
    ])
}

#[test]
fn signed_in_shows_identity_credits_plan_and_key() {
    let base = signed_in_server();
    let (code, out, err) = run(&base, &["auth", "status"], None);
    assert_eq!(code, 0, "{out}{err}");
    // WHY: this is the whole point of the command: who am I, what can I spend,
    // and which key is this.
    for want in [
        "duyet · me@example.com",
        "$53.08",
        "plan",
        "free",
        "laptop",
        "sk-ar-v1-testk…6789",
        "2026-10-27 15:31 UTC",
        "all endpoints",
        &base,
        "config",
    ] {
        assert!(out.contains(want), "missing {want:?}:\n{out}");
    }
    assert!(!out.contains(KEY), "full key leaked:\n{out}");
    assert!(!out.contains("deadbeef"), "key hash leaked:\n{out}");
}

#[test]
fn signed_in_json_adds_credits_plan_and_key_fields() {
    let base = signed_in_server();
    let (code, out, err) = run(&base, &["auth", "status", "--json"], None);
    assert_eq!(code, 0, "{out}{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["signed_in"], true);
    assert_eq!(v["user"]["email"], "me@example.com");
    assert_eq!(v["user"]["username"], "duyet");
    assert_eq!(v["credits"], 53.0794);
    assert_eq!(v["plan"], "free");
    assert_eq!(v["key"]["name"], "laptop");
    assert_eq!(v["key"]["last_used_at"], "2026-10-27T15:31:41Z");
    assert_eq!(v["key_masked"], "sk-ar-v1-testk…6789");
    assert_eq!(v["base_url"], base);
    assert!(!out.contains(KEY) && !out.contains("deadbeef"), "{out}");
}

#[test]
fn api_error_still_prints_local_status_and_says_why() {
    // WHY: offline or a revoked key must not hide the local facts (which key,
    // which config); the summary says what failed instead.
    let (code, out, err) = run("http://127.0.0.1:9/api", &["auth", "status"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("sk-ar-v1-testk…6789"), "{out}");
    assert!(out.contains("unavailable"), "{out}");
    assert!(out.contains("http://127.0.0.1:9/api"), "{out}");
}

#[test]
fn signed_out_prints_the_login_hint_and_exits_4() {
    let home = unique_temp_dir("anyr-status-signed-out");
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .env_clear()
        .env("HOME", &home)
        .env("ANYROUTER_HOME", &home)
        .env("ANYR_NO_UPDATE", "1")
        .env("NO_COLOR", "1")
        .args(["auth", "status"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // WHY: scripts branch on exit 4 (auth) to decide whether to log in.
    assert_eq!(out.status.code(), Some(4), "{stdout}");
    assert!(stdout.contains("Not signed in"), "{stdout}");
    assert!(stdout.contains("anyr auth login"), "{stdout}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn failed_key_lookup_is_short_on_stdout_and_full_with_verbose() {
    let base = serve_routes(vec![
        ("/api/v1/me", 200, ME.into()),
        ("/api/v1/key", 500, r#"{"error":{"message":"boom"}}"#.into()),
    ]);
    let (code, out, err) = run(&base, &["auth", "status"], None);
    assert_eq!(code, 0, "{out}{err}");
    // WHY: a long error wrapped across the terminal and broke the rows.
    assert!(out.contains("scopes    unavailable (HTTP 500)"), "{out}");
    assert!(!out.contains("plan"), "unknown plan must be hidden:\n{out}");
    assert!(
        !err.contains("key lookup"),
        "quiet without --verbose: {err}"
    );
    let (code, out, err) = run(&base, &["auth", "status", "--verbose"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        err.contains("key lookup: Could not fetch key info (HTTP 500)."),
        "{err}"
    );
}

#[test]
fn piped_config_get_makes_no_network_call() {
    // WHY: `config get` in a script or pipe reads local settings; it waited
    // on /me and /credits in series (p50 369 ms) for lines nobody parses.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/api", listener.local_addr().unwrap());
    for args in [&["config", "get"][..], &["config"][..]] {
        let (code, out, err) = run(&base, args, None);
        assert_eq!(code, 0, "{args:?}: {out}{err}");
        assert!(out.contains("sk-ar-v1-testk…6789"), "{args:?}: {out}");
        assert!(out.contains("model"), "{args:?}: {out}");
        assert!(!out.contains("credits "), "{args:?}: {out}");
    }
    listener.set_nonblocking(true).unwrap();
    assert!(
        matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "config get opened a connection"
    );
}

#[test]
fn piped_config_get_is_fast_against_an_unroutable_host() {
    // WHY: offline (192.0.2.0/24 never routes) the old dump hung on connect.
    let t0 = std::time::Instant::now();
    let (code, out, err) = run("http://192.0.2.1:9/api", &["config", "get"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("sk-ar-v1-testk…6789"), "{out}");
    assert!(
        t0.elapsed() < std::time::Duration::from_secs(3),
        "took {:?}",
        t0.elapsed()
    );
}

#[test]
fn verbose_config_get_still_shows_account_and_credits() {
    let base = serve_routes(vec![
        ("/api/v1/me", 200, ME.into()),
        ("/api/v1/credits", 200, r#"{"balance":53.0794}"#.into()),
    ]);
    let (code, out, err) = run(&base, &["config", "get", "--verbose"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("duyet"), "{out}");
    assert!(out.contains("$53.08"), "{out}");
}
