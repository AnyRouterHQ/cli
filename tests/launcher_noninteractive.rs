//! The launcher state machine (plan 4.2) walked with no TTY at all: every
//! state the HUD offers has a flag or a command, and a missing piece fails
//! with an exit code and the exact rerun instead of a prompt.

use std::io::Write;
use std::path::Path;
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

const KEY: &str = "sk-ar-v1-parity0123456789abcdefghij";
const MODEL: &str = "z-ai/glm-4.7";

fn anyr(home: &Path, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd // Not env_clear: Windows needs SystemRoot to open a socket.
        .env_remove("ANYROUTER_API_KEY")
        .env_remove("ANYROUTER_PROFILE")
        .env_remove("ANYROUTER_MANAGEMENT_KEY")
        .env_remove("ANYR_OUTPUT")
        .env_remove("ANYR_AGENTS")
        .env("HOME", home)
        .env("ANYROUTER_HOME", home)
        // No agents and no `sh`: an installer started by mistake fails.
        .env("PATH", home.join("bin"))
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("ANYR_TELEMETRY", "0")
        .env("NO_COLOR", "1")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = match stdin {
        None => cmd.stdin(Stdio::null()).output().unwrap(),
        Some(input) => {
            let mut child = cmd.stdin(Stdio::piped()).spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        }
    };
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn every_launcher_state_has_a_non_interactive_path() {
    let home = std::env::temp_dir().join(format!("anyr-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join("bin")).unwrap();

    // S0 profile: sign in from stdin, never argv.
    let (code, out, err) = anyr(
        &home,
        &["login", "--key-stdin", "--base-url", &gateway()],
        Some(&format!("{KEY}\n")),
    );
    assert_eq!(code, 0, "login: {out}{err}");

    // S3 model, saved for the agent with its account.
    let (code, out, err) = anyr(
        &home,
        &[
            "agents",
            "set",
            "claude",
            "--model",
            MODEL,
            "--profile",
            "default",
        ],
        None,
    );
    assert_eq!(code, 0, "agents set: {out}{err}");

    // S5: the plan, with the saved model and the account's masked key.
    let (code, out, err) = anyr(&home, &["claude", "--dry-run", "--json"], None);
    assert_eq!(code, 0, "dry run: {out}{err}");
    let plan: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(plan["model"], MODEL, "{out}");
    assert_eq!(plan["key_masked"], "sk-ar-v1-parit…ghij", "{out}");
    assert!(!out.contains(KEY), "{out}");

    let (code, out, _) = anyr(&home, &["agents", "list", "--json"], None);
    assert_eq!(code, 0);
    let rows: Vec<Value> = serde_json::from_str(&out).unwrap();
    let claude = rows.iter().find(|r| r["id"] == "claude").unwrap();
    assert_eq!(
        (&claude["model"], &claude["profile"], &claude["key_source"]),
        (&MODEL.into(), &"default".into(), &"profile".into())
    );
    assert_eq!(claude["installed"], false);

    // S2: a missing agent is exit 3 with the exact rerun, never an install.
    let (code, out, err) = anyr(&home, &["claude", "--json"], None);
    assert_eq!(code, 3, "{out}{err}");
    let e: Value = serde_json::from_str(err.trim()).unwrap();
    let rerun = e["error"]["message"].as_str().unwrap_or_default();
    assert!(
        rerun.contains("anyr claude --install")
            && rerun.contains("anyr agents install claude --yes"),
        "{err}"
    );

    // S1: bare `anyr` off a TTY is the help, exit 0, no prompt.
    let (code, out, err) = anyr(&home, &[], None);
    assert_eq!(code, 0, "{err}");
    let (_, help, _) = anyr(&home, &["--help"], None);
    assert_eq!(out, help);
    assert!(err.is_empty(), "{err}");
}
