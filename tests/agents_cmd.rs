//! `anyr agents`: per-agent settings without a TTY, over the same config
//! fields `config set launcher.<agent>.*` and `keys set --agent` write.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

mod common;

/// The sign-in check (`GET /v1/credits`); any other request gets a 404.
fn gateway() -> String {
    common::serve_routes(vec![(
        "/api/v1/credits",
        200,
        r#"{"data":{"total_credits":10,"total_usage":1}}"#.into(),
    )])
}

const KEY: &str = "sk-ar-v1-agentscmd0123456789abcdef";
const AGENT_KEY: &str = "sk-ar-v1-agentown0123456789abcdefgh";

fn home(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("anyr-agents-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    dir
}

fn anyr(home: &Path, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_anyr"))
        // Not env_clear: Windows needs SystemRoot to open a socket.
        .env_remove("ANYROUTER_API_KEY")
        .env_remove("ANYROUTER_PROFILE")
        .env_remove("ANYROUTER_MANAGEMENT_KEY")
        .env_remove("ANYR_OUTPUT")
        .env_remove("ANYR_AGENTS")
        .env("HOME", home)
        .env("ANYROUTER_HOME", home)
        .env("PATH", home.join("bin"))
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("ANYR_TELEMETRY", "0")
        .env("NO_COLOR", "1")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input = stdin.unwrap_or("").to_string();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn login(h: &Path) {
    let (code, out, err) = anyr(
        h,
        &["login", "--key-stdin", "--base-url", &gateway()],
        Some(&format!("{KEY}\n")),
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(!out.contains(KEY) && !err.contains(KEY), "{out}{err}");
}

fn config(h: &Path) -> String {
    std::fs::read_to_string(h.join("config.yaml")).unwrap_or_default()
}

#[test]
fn login_key_stdin_saves_the_key_without_argv() {
    let h = home("login");
    login(&h);
    assert!(config(&h).contains(KEY));
}

#[test]
fn list_has_one_row_per_launcher_and_no_key() {
    let h = home("list");
    login(&h);
    let (code, out, err) = anyr(&h, &["agents", "list", "--json"], None);
    assert_eq!(code, 0, "{err}");
    let rows: Vec<Value> = serde_json::from_str(&out).unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, anyr_cli::registry::LAUNCHERS);
    let claude = &rows[0];
    assert_eq!(claude["installed"], false, "PATH has no agents");
    assert_eq!(claude["key_source"], "profile");
    assert!(!out.contains("sk-ar-v1"), "no key in any form:\n{out}");
}

#[test]
fn set_get_unset_round_trip_and_set_is_idempotent() {
    let h = home("set");
    login(&h);
    std::fs::write(
        h.join("config.yaml"),
        format!("active_profile: default\nprofiles:\n  default:\n    api_key: {KEY}\n  work:\n    api_key: {KEY}\n"),
    )
    .unwrap();
    let set = [
        "agents",
        "set",
        "cc",
        "--model",
        "z-ai/glm-4.7",
        "--profile",
        "work",
        "--args=--verbose",
    ];
    let (code, _, err) = anyr(&h, &set, None);
    assert_eq!(code, 0, "{err}");
    // A hand-formatted file: a repeat must not even re-serialize it.
    let mut hand = config(&h);
    hand.insert_str(0, "# kept by hand\n");
    std::fs::write(h.join("config.yaml"), &hand).unwrap();
    let (code, _, err) = anyr(&h, &set, None);
    assert_eq!(code, 0, "{err}");
    assert_eq!(config(&h), hand, "same values leave the file byte for byte");

    let (_, out, _) = anyr(&h, &["agents", "get", "claude", "--json"], None);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["model"], "z-ai/glm-4.7");
    assert_eq!(v["profile"], "work");
    assert_eq!(v["args"], serde_json::json!(["--verbose"]));
    // Same storage as `config get`.
    let (_, out, _) = anyr(&h, &["config", "get", "launcher.claude.model"], None);
    assert_eq!(out.trim(), "z-ai/glm-4.7");

    let (code, _, err) = anyr(
        &h,
        &["agents", "unset", "claude", "--model", "--args"],
        None,
    );
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = anyr(&h, &["agents", "get", "claude", "--json"], None);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (v["model"].is_null(), &v["profile"]),
        (true, &Value::from("work"))
    );
    let (code, _, _) = anyr(&h, &["agents", "unset", "claude", "--all"], None);
    assert_eq!(code, 0);
    assert!(!config(&h).contains("agents:"), "{}", config(&h));
}

#[test]
fn key_stdin_is_stored_never_echoed_and_unset_by_key() {
    let h = home("key");
    login(&h);
    let (code, out, err) = anyr(
        &h,
        &["agents", "set", "codex", "--key-stdin", "--json"],
        Some(&format!("{AGENT_KEY}\n")),
    );
    assert_eq!(code, 0, "{err}");
    assert!(
        !out.contains("sk-ar-v1") && !err.contains("sk-ar-v1"),
        "{out}{err}"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["key_source"],
        "agent"
    );
    assert!(config(&h).contains(AGENT_KEY));
    let (code, _, _) = anyr(&h, &["agents", "unset", "codex", "--key"], None);
    assert_eq!(code, 0);
    assert!(!config(&h).contains(AGENT_KEY));
    // A key in argv is refused: it would land in `ps` and shell history.
    let (code, _, _) = anyr(&h, &["agents", "set", "codex", "--key", AGENT_KEY], None);
    assert_eq!(code, 2);
    // A malformed key is refused before anything is written.
    let before = config(&h);
    let (code, _, _) = anyr(
        &h,
        &["agents", "set", "codex", "--key-stdin"],
        Some("nope\n"),
    );
    assert_ne!(code, 0);
    assert_eq!(config(&h), before);
}

#[test]
fn unknown_agent_and_unknown_profile_exit_3_with_a_hint() {
    let h = home("notfound");
    login(&h);
    let (code, _, err) = anyr(&h, &["agents", "get", "vim", "--json"], None);
    assert_eq!(code, 3, "{err}");
    let v: Value = serde_json::from_str(err.trim()).unwrap();
    assert!(
        v["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("claude, codex"),
        "{err}"
    );
    let before = config(&h);
    let (code, _, err) = anyr(&h, &["agents", "set", "claude", "--profile", "nope"], None);
    assert_eq!(code, 3, "{err}");
    assert_eq!(config(&h), before);
}

#[test]
fn install_off_a_tty_needs_yes() {
    let h = home("install");
    let (code, _, err) = anyr(&h, &["agents", "install", "claude", "--json"], None);
    assert_eq!(code, 6, "{err}");
    let v: Value = serde_json::from_str(err.trim()).unwrap();
    assert!(
        v["error"]["hint"].as_str().unwrap().contains("--yes"),
        "{err}"
    );
}

#[test]
fn agents_help_lists_its_verbs_and_help_agents_stays_the_guide() {
    let h = home("help");
    let (code, out, _) = anyr(&h, &["agents", "--help"], None);
    assert_eq!(code, 0);
    for verb in ["list", "get", "set", "unset", "install", "--key-stdin"] {
        assert!(out.contains(verb), "{verb}\n{out}");
    }
    let (_, guide, _) = anyr(&h, &["help", "agents"], None);
    assert!(guide.contains("Exit codes"), "{guide}");
}

#[test]
fn a_key_pasted_as_a_switch_value_is_never_echoed() {
    let h = home("echo");
    login(&h);
    for args in [
        &["agents", "unset", "claude", &format!("--key={AGENT_KEY}")][..],
        &["agents", "--json", "unset", "--key", AGENT_KEY][..],
    ] {
        let (code, out, err) = anyr(&h, args, None);
        assert_ne!(code, 0, "{args:?}");
        assert!(
            !out.contains(AGENT_KEY) && !err.contains(AGENT_KEY),
            "{args:?}: {out}{err}"
        );
    }
}

#[test]
fn an_unreadable_config_is_an_error_not_overwritten() {
    let h = home("unreadable");
    // A directory where the file should be: it cannot be read.
    std::fs::create_dir_all(h.join("config.yaml")).unwrap();
    let (code, _, _) = anyr(&h, &["agents", "set", "claude", "--model", "a/b"], None);
    assert_ne!(code, 0);
    assert!(h.join("config.yaml").is_dir());
}
