//! `anyr opencode` must reach the AnyRouter provider on both OpenCode 1.x and 2.x.
//!
//! The fake `opencode` mimics the one behaviour that differs: 2.x sends `run`
//! and the TUI to a background service started earlier with its own
//! environment, so the injected `OPENCODE_CONFIG_CONTENT` is ignored unless
//! `--standalone` is passed. 1.x has no service and rejects `--standalone`.
#![cfg(unix)]

mod common;
use common::{serve, KEY};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::Duration;

const FAKE: &str = r#"#!/bin/sh
case "$1" in --version) echo "$FAKE_OC_VERSION"; exit 0;; esac
standalone=0
for a in "$@"; do
  if [ "$a" = "--standalone" ]; then standalone=1; fi
done
case "$FAKE_OC_VERSION" in
  *v2.*)
    if [ "$standalone" = 0 ]; then
      echo "Error: This API key has expired. Create a new API key in OpenCode Console" >&2
      exit 3
    fi ;;
  *)
    if [ "$standalone" = 1 ]; then echo "Unrecognized flag: --standalone" >&2; exit 2; fi ;;
esac
base=$(printf '%s' "$OPENCODE_CONFIG_CONTENT" | sed -n 's/.*"baseURL":"\([^"]*\)".*/\1/p')
curl -s -o /dev/null -X POST "$base/chat/completions" \
  -H "Authorization: Bearer $OPENAI_API_KEY" -H 'content-type: application/json' -d '{}'
"#;

fn run_opencode(
    version: &str,
    tag: &str,
) -> (i32, String, std::sync::mpsc::Receiver<common::Seen>) {
    let (base, rx) = serve(vec![(200, "{}".into())]);
    let dir = std::env::temp_dir().join(format!("anyr-opencode-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("fake-opencode.sh");
    std::fs::write(&script, FAKE).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("NO_COLOR", "1")
        .env("ANYROUTER_HOME", &dir)
        .env("ANYROUTER_OPENCODE_PATH", &script)
        .env("FAKE_OC_VERSION", version)
        .args(["opencode", "--base-url", &base, "--key", KEY])
        .args([
            "--model",
            "deepseek/deepseek-v4-flash",
            "--yes",
            "--no-mcp",
            "--",
            "run",
            "say hi",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    (out.status.code().unwrap_or(-1), err, rx)
}

fn assert_hits_anyrouter(version: &str, tag: &str) {
    let (code, err, rx) = run_opencode(version, tag);
    assert_eq!(code, 0, "{err}");
    let seen = rx.recv_timeout(Duration::from_secs(5)).expect(&err);
    assert_eq!(seen.method, "POST");
    assert!(seen.path.ends_with("/chat/completions"), "{}", seen.path);
    assert_eq!(seen.auth, format!("Bearer {KEY}"));
}

#[test]
fn opencode_2_run_reaches_the_anyrouter_provider() {
    assert_hits_anyrouter("opencode v2.0.24", "v2");
}

#[test]
fn opencode_1_run_reaches_the_anyrouter_provider() {
    assert_hits_anyrouter("1.4.3", "v1");
}

fn dry_run_args(version: &str, tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("anyr-opencode-dry-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("fake-opencode.sh");
    std::fs::write(&script, FAKE).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("NO_COLOR", "1")
        .env("ANYROUTER_HOME", &dir)
        .env("ANYROUTER_OPENCODE_PATH", &script)
        .env("FAKE_OC_VERSION", version)
        .args([
            "opencode",
            "--base-url",
            "http://127.0.0.1:1/api",
            "--key",
            KEY,
        ])
        .args([
            "--model",
            "deepseek/deepseek-v4-flash",
            "--yes",
            "--dry-run",
            "--",
            "run",
            "hi",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn dry_run_shows_standalone_for_opencode_2_only() {
    assert!(dry_run_args("opencode v2.0.24", "v2").contains(r#"args: ["run","--standalone","hi"]"#));
    assert!(dry_run_args("1.4.3", "v1").contains(r#"args: ["run","hi"]"#));
}

fn dry_run_auto(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("anyr-opencode-auto-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("fake-opencode.sh");
    std::fs::write(&script, FAKE).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("NO_COLOR", "1")
        .env("ANYROUTER_HOME", &dir)
        .env("ANYROUTER_OPENCODE_PATH", &script)
        .env("FAKE_OC_VERSION", "opencode v2.0.24")
        .args([
            "opencode",
            "--base-url",
            "http://127.0.0.1:1/api",
            "--key",
            KEY,
            "--model",
            "auto",
            "--yes",
            "--dry-run",
            "--",
            "run",
            "hi",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn auto_registers_an_anyrouter_model_and_sets_the_session_model() {
    // WHY #230: --model auto left models empty and no top-level model.
    let out = dry_run_auto("auto");
    assert!(out.contains("anyrouter/anyrouter/auto"), "{out}");
    assert!(out.contains("anyrouter/auto"), "{out}");
}
