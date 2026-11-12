//! `anyr keys rotate` against a local scripted server: management-key auth,
//! the secret shown once, and every stored copy of the old key replaced.

mod common;
use common::{serve, unique_temp_dir};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const OLD_KEY: &str = "sk-ar-v1-oldsecret00000000000000000aaaa";
const WORK_KEY: &str = "sk-ar-v1-workaccountkey000000000bbbb";
const CI_KEY: &str = "sk-ar-v1-cikeystorednowhere0000000dddd";
const NEW_KEY: &str = "sk-ar-v1-rotatednewsecret000000000cccc";
const MGMT: &str = "ak_test_management_key_0001";

fn hash(key: &str) -> String {
    format!("{:x}", Sha256::digest(key.as_bytes()))
}

fn list() -> (u16, String) {
    let row = |name: &str, key: &str, masked: &str| {
        format!(
            r#"{{"id":"id-{name}","hash":"{}","name":"{name}","label":"{masked}"}}"#,
            hash(key)
        )
    };
    (
        200,
        format!(
            r#"{{"data":[{},{},{}]}}"#,
            row("laptop", OLD_KEY, "sk-ar-v1-...aaaa"),
            row("work", WORK_KEY, "sk-ar-v1-...bbbb"),
            row("ci", CI_KEY, "sk-ar-v1-...dddd")
        ),
    )
}

fn rotated() -> (u16, String) {
    (
        200,
        format!(
            r#"{{"key":"{NEW_KEY}","data":{{"hash":"{}","name":"laptop"}}}}"#,
            hash(NEW_KEY)
        ),
    )
}

/// A home whose default account and claude's own key are the key being
/// rotated; the work account holds another key.
fn home(base: &str, management_key: bool) -> PathBuf {
    let dir = unique_temp_dir("anyr-keys-rotate");
    let mk = if management_key {
        format!("    management_key: {MGMT}\n")
    } else {
        String::new()
    };
    std::fs::write(
        dir.join("config.yaml"),
        format!(
            "active_profile: default\nprofiles:\n  default:\n    api_key: {OLD_KEY}\n    base_url: {base}\n{mk}  work:\n    api_key: {WORK_KEY}\nagents:\n  claude:\n    api_key: {OLD_KEY}\n"
        ),
    )
    .unwrap();
    dir
}

fn anyr(home: &Path, args: &[&str]) -> (i32, String, String) {
    anyr_env(home, args, &[])
}

fn anyr_env(home: &Path, args: &[&str], envs: &[(&str, &str)]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .env("ANYROUTER_HOME", home)
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("ANYR_TELEMETRY", "0")
        .env("NO_COLOR", "1")
        .env_remove("ANYROUTER_API_KEY")
        .env_remove("ANYROUTER_PROFILE")
        .env_remove("ANYROUTER_MANAGEMENT_KEY")
        .envs(envs.iter().copied())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn config(home: &Path) -> String {
    std::fs::read_to_string(home.join("config.yaml")).unwrap()
}

#[test]
fn rotate_posts_with_the_management_key_and_replaces_every_stored_copy() {
    // WHY: the server kills the old secret at once; a stored copy left behind
    // would make this machine and claude fail on their next request.
    let (base, rx) = serve(vec![list(), rotated()]);
    let h = home(&base, true);
    let (code, out, err) = anyr(&h, &["keys", "rotate", "aaaa", "--yes"]);
    assert_eq!(code, 0, "{out}{err}");
    let listed = rx.recv().unwrap();
    assert_eq!(
        (listed.method.as_str(), listed.path.as_str()),
        ("GET", "/api/v1/keys")
    );
    let post = rx.recv().unwrap();
    assert_eq!(post.method, "POST");
    assert_eq!(post.path, format!("/api/v1/keys/{}/rotate", hash(OLD_KEY)));
    assert!(
        post.auth.contains(MGMT),
        "rotate must send the ak_ key: {}",
        post.auth
    );
    // The secret is printed exactly once, and nowhere on stderr.
    assert_eq!(out.matches(NEW_KEY).count(), 1, "{out}");
    assert!(!err.contains(NEW_KEY), "{err}");
    assert!(
        out.contains("account default") && out.contains("agent claude"),
        "{out}"
    );
    let cfg = config(&h);
    assert!(!cfg.contains(OLD_KEY), "{cfg}");
    assert_eq!(cfg.matches(NEW_KEY).count(), 2, "{cfg}");
    assert!(cfg.contains(WORK_KEY), "{cfg}");
    assert!(cfg.contains(&format!("management_key: {MGMT}")), "{cfg}");
}

#[test]
fn rotate_json_returns_the_secret_and_where_it_was_saved() {
    let (base, _rx) = serve(vec![list(), rotated()]);
    let h = home(&base, true);
    let (code, out, err) = anyr(&h, &["keys", "rotate", "id-laptop", "--yes", "--json"]);
    assert_eq!(code, 0, "{out}{err}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["key"], NEW_KEY);
    assert_eq!(v["hash"], hash(NEW_KEY));
    assert_eq!(v["saved"], true);
    assert_eq!(
        v["stored"],
        serde_json::json!(["account default", "agent claude"])
    );
    assert!(!err.contains(NEW_KEY), "{err}");
}

#[test]
fn rotating_a_key_that_is_not_stored_leaves_config_alone() {
    let (base, _rx) = serve(vec![list(), rotated()]);
    let h = home(&base, true);
    let before = config(&h);
    let (code, out, err) = anyr(&h, &["keys", "rotate", "dddd", "--yes", "--json"]);
    assert_eq!(code, 0, "{out}{err}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["saved"], false);
    assert_eq!(v["stored"], serde_json::json!([]));
    assert_eq!(config(&h), before);
}

#[test]
fn a_failed_config_write_still_prints_the_new_key_and_fails() {
    // WHY: the old secret is already dead; losing the only copy of the new one
    // would lock the user out of that key.
    let (base, _rx) = serve(vec![list(), rotated()]);
    let h = home(&base, true);
    // A directory where the temp file goes makes the write fail.
    std::fs::create_dir(h.join("config.yaml.tmp")).unwrap();
    let (code, out, err) = anyr(&h, &["keys", "rotate", "aaaa", "--yes"]);
    assert_eq!(code, 1, "{out}{err}");
    assert_eq!(out.matches(NEW_KEY).count(), 1, "{out}");
    assert!(err.contains("not saved"), "{err}");
    assert!(!err.contains(NEW_KEY), "{err}");
}

#[test]
fn without_a_management_key_it_exits_4_and_sends_nothing() {
    let (base, rx) = serve(vec![list(), rotated()]);
    let h = home(&base, false);
    let (code, out, err) = anyr(&h, &["keys", "rotate", "aaaa", "--yes"]);
    assert_eq!(code, 4, "{out}{err}");
    assert!(err.contains("ak_"), "{err}");
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn off_a_terminal_it_needs_yes_and_sends_nothing() {
    let (base, rx) = serve(vec![list(), rotated()]);
    let h = home(&base, true);
    let (code, _, err) = anyr(&h, &["keys", "rotate", "aaaa"]);
    assert_eq!(code, 6, "{err}");
    assert!(err.contains("--yes"), "{err}");
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn server_errors_map_to_exit_codes_and_keep_the_old_key() {
    for (status, want) in [(401, 4), (404, 3), (402, 8)] {
        let (base, _rx) = serve(vec![
            list(),
            (status, r#"{"error":{"message":"nope"}}"#.into()),
        ]);
        let h = home(&base, true);
        let (code, out, err) = anyr(&h, &["keys", "rotate", "aaaa", "--yes", "--json"]);
        assert_eq!(code, want, "HTTP {status}: {out}{err}");
        assert!(config(&h).contains(OLD_KEY));
    }
}

#[test]
fn an_unknown_key_is_exit_3_before_any_rotate() {
    let (base, rx) = serve(vec![list()]);
    let h = home(&base, true);
    let (code, _, err) = anyr(&h, &["keys", "rotate", "zzzz", "--yes"]);
    assert_eq!(code, 3, "{err}");
    rx.recv().unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn an_env_key_holding_the_old_secret_is_named_without_printing_it() {
    // WHY: ANYROUTER_API_KEY beats config.yaml, so after the rotate every
    // command would fail until the user replaces it.
    let (base, _rx) = serve(vec![list(), rotated()]);
    let h = home(&base, true);
    let (code, out, err) = anyr_env(
        &h,
        &["keys", "rotate", "aaaa", "--yes"],
        &[("ANYROUTER_API_KEY", OLD_KEY)],
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        err.contains("ANYROUTER_API_KEY holds the old secret"),
        "{err}"
    );
    assert!(!err.contains(OLD_KEY) && !err.contains(NEW_KEY), "{err}");
}
