//! `anyr <launcher> --json --dry-run`: one JSON object on stdout with the
//! settled plan — `command`, `args`, redacted `env`, `model`, `base_url`,
//! masked key and the decision fields an agent can act on. Exit 0, nothing
//! else on stdout, and no full secret anywhere. Non-JSON `--dry-run` is
//! unchanged (text form).

mod common;

use std::process::{Command, Stdio};

const KEY: &str = "sk-ar-v1-fixture-key-0001";

fn anyr(extra_env: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env(
            "ANYROUTER_HOME",
            common::unique_temp_dir("anyr-json-dry-run"),
        )
        .env_remove("ANYROUTER_API_KEY");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// `anyr <agent> --json --dry-run ...` with a nonexistent agent binary so no
/// version probe can add flags (opencode) or hit the network.
fn json_dry_run(agent: &str, extra: &[&str], extra_env: &[(&str, &str)]) -> (i32, String, String) {
    let out = anyr(extra_env)
        .args([
            agent,
            "--json",
            "--dry-run",
            "--yes",
            "--no-mcp",
            "--key",
            KEY,
            "--command-path",
            "/nonexistent/agent-bin",
        ])
        .args(extra)
        .output()
        .expect("spawn anyr");
    (
        out.status.code().unwrap_or(1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn parse_plan(stdout: &str) -> serde_json::Value {
    // One JSON object on stdout: the whole trimmed output must parse, and
    // there must be exactly one non-empty line.
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "stdout is not one line:\n{stdout}");
    serde_json::from_str(stdout.trim()).expect("stdout is not JSON")
}

#[test]
fn json_dry_run_prints_the_settled_plan_and_redacts_secrets() {
    let (code, stdout, stderr) = json_dry_run("claude", &[], &[]);
    assert_eq!(code, 0, "stderr={stderr} stdout={stdout}");
    let v = parse_plan(&stdout);
    assert_eq!(v["agent"], "claude");
    assert_eq!(v["command"], "/nonexistent/agent-bin");
    assert!(v["args"].is_array(), "args: {v}");
    assert_eq!(v["model"], "anyrouter/auto");
    assert!(
        v["base_url"].as_str().unwrap().contains("anyrouter.dev"),
        "{v}"
    );
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["agent_mode"], true);
    assert!(v["unset"].is_array(), "{v}");
    assert!(v["mcp"].as_str().unwrap().contains("mcp:"), "{v}");
    // The key is present only masked; the full secret is nowhere.
    let masked = v["key_masked"].as_str().unwrap();
    assert_ne!(masked, KEY);
    assert!(masked.contains("sk-ar-v1-"), "{masked}");
    assert!(!stdout.contains(KEY), "full key leaked:\n{stdout}");
    assert!(
        !stderr.contains(KEY),
        "full key leaked on stderr:\n{stderr}"
    );
    // The token env is redacted, not the raw secret.
    let token = v["env"]["ANTHROPIC_AUTH_TOKEN"].as_str().unwrap();
    assert_ne!(token, KEY);
    assert!(!stdout.contains(KEY), "{stdout}");
    // Base URL env is still visible so the plan is actionable.
    let base = v["env"]["ANTHROPIC_BASE_URL"].as_str().unwrap();
    assert!(base.contains("anyrouter.dev/api"), "{base}");
}

#[test]
fn json_dry_run_reflects_model_and_effort_flags() {
    let (code, stdout, stderr) = json_dry_run(
        "claude",
        &["--model", "z-ai/glm-4.7-flash", "--effort", "high"],
        &[],
    );
    assert_eq!(code, 0, "stderr={stderr}");
    let v = parse_plan(&stdout);
    assert_eq!(v["model"], "z-ai/glm-4.7-flash");
    assert_eq!(v["effort"], "high");
}

#[test]
fn json_flag_is_owned_and_raw_separator_forwards_json() {
    let (code, stdout, stderr) = json_dry_run("claude", &["-p", "hi"], &[]);
    assert_eq!(code, 0, "stderr={stderr}");
    let v = parse_plan(&stdout);
    let args: Vec<&str> = v["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert!(!args.contains(&"--json"), "--json is anyr's: {args:?}");
    assert!(args.ends_with(&["-p", "hi"]), "{args:?}");

    // After `--`, the agent's own --json reaches the plan (text dry-run, so
    // it is not confused with anyr's json flag).
    let out = anyr(&[])
        .args([
            "claude",
            "--dry-run",
            "--yes",
            "--no-mcp",
            "--key",
            KEY,
            "--command-path",
            "/nonexistent/agent-bin",
            "--",
            "--json",
        ])
        .output()
        .expect("spawn anyr");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"--json\""), "raw tail:\n{stdout}");
    assert!(
        !stdout.trim_start().starts_with('{'),
        "text form:\n{stdout}"
    );
}

#[test]
fn anyr_output_json_env_also_prints_the_json_plan() {
    let out = anyr(&[("ANYR_OUTPUT", "json")])
        .args([
            "claude",
            "--dry-run",
            "--yes",
            "--no-mcp",
            "--key",
            KEY,
            "--command-path",
            "/nonexistent/agent-bin",
        ])
        .output()
        .expect("spawn anyr");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v = parse_plan(&stdout);
    assert_eq!(v["agent"], "claude");
    assert!(!stdout.contains(KEY), "{stdout}");
}

#[test]
fn text_dry_run_is_unchanged_without_json() {
    let out = anyr(&[])
        .args([
            "claude",
            "--dry-run",
            "--yes",
            "--no-mcp",
            "--key",
            KEY,
            "--command-path",
            "/nonexistent/agent-bin",
        ])
        .output()
        .expect("spawn anyr");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Text form: `command:` / `args:` / `env:` lines, not one JSON object.
    assert!(
        stdout.contains("command: /nonexistent/agent-bin"),
        "{stdout}"
    );
    assert!(stdout.contains("args: "), "{stdout}");
    assert!(stdout.contains("env:"), "{stdout}");
    assert!(stdout.contains("ANTHROPIC_BASE_URL="), "{stdout}");
    assert!(!stdout.trim_start().starts_with('{'), "{stdout}");
    assert!(!stdout.contains(KEY), "full key leaked:\n{stdout}{stderr}");
    assert!(serde_json::from_str::<serde_json::Value>(stdout.trim()).is_err());
    let _ = stderr;
}
