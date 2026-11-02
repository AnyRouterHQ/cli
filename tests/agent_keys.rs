//! One key or account per coding agent: `keys set|unset|create --agent`,
//! `account use --agent [--clear]`, and what each launcher actually gets.

mod common;
use common::{serve, unique_temp_dir};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const DEFAULT_KEY: &str = "sk-ar-v1-defaultaccountkey000000aaaa";
const WORK_KEY: &str = "sk-ar-v1-workaccountkey000000000bbbb";
const CLAUDE_KEY: &str = "sk-ar-v1-claudeownkey00000000000cccc";
const CODEX_KEY: &str = "sk-ar-v1-codexownkey000000000000dddd";
const MINTED_KEY: &str = "sk-ar-v1-mintedforcodex000000000eeee";
const ALL_KEYS: [&str; 5] = [DEFAULT_KEY, WORK_KEY, CLAUDE_KEY, CODEX_KEY, MINTED_KEY];

fn home() -> PathBuf {
    let dir = unique_temp_dir("anyr-agent-keys");
    std::fs::write(
        dir.join("config.yaml"),
        format!(
            "active_profile: default\nprofiles:\n  default:\n    api_key: {DEFAULT_KEY}\n  work:\n    api_key: {WORK_KEY}\n"
        ),
    )
    .unwrap();
    dir
}

/// Run `anyr` against `home` with no ambient key, so only the config decides.
fn anyr(home: &Path, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    anyr_env(home, args, stdin, &[])
}

fn anyr_env(
    home: &Path,
    args: &[&str],
    stdin: Option<&str>,
    envs: &[(&str, &str)],
) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .env("ANYROUTER_HOME", home)
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("NO_COLOR", "1")
        .env_remove("ANYROUTER_API_KEY")
        .env_remove("ANYROUTER_PROFILE")
        .env_remove("ANYROUTER_MANAGEMENT_KEY")
        .envs(envs.iter().copied())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    if let Some(input) = stdin {
        pipe.write_all(input.as_bytes()).unwrap();
    }
    drop(pipe);
    let out = child.wait_with_output().unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    for key in ALL_KEYS {
        if args.contains(&"create") && key == MINTED_KEY {
            continue;
        }
        assert!(
            !stdout.contains(key) && !stderr.contains(key),
            "full key {key} printed by {args:?}:\n{stdout}{stderr}"
        );
    }
    (out.status.code().unwrap_or(-1), stdout, stderr)
}

fn ok(home: &Path, args: &[&str], stdin: Option<&str>) -> String {
    let (code, out, err) = anyr(home, args, stdin);
    assert_eq!(code, 0, "{args:?}\n{out}{err}");
    out
}

/// The `<auth_env>=` line `--dry-run` prints, which shows only the last four.
fn launch_auth(home: &Path, agent: &str, env_name: &str, extra: &[&str]) -> String {
    let mut args = vec![agent, "--dry-run"];
    args.extend_from_slice(extra);
    let out = ok(home, &args, None);
    out.lines()
        .find(|l| l.starts_with(&format!("{env_name}=")))
        .unwrap_or_else(|| panic!("no {env_name} in dry run:\n{out}"))
        .to_string()
}

fn last4(key: &str) -> &str {
    &key[key.len() - 4..]
}

fn config(home: &Path) -> String {
    std::fs::read_to_string(home.join("config.yaml")).unwrap()
}

#[test]
fn each_agent_launches_with_its_own_key_or_account() {
    // WHY: the point of per-agent keys is that spend, limits and revocation
    // stay apart. If claude and codex ever launch with the same key, they
    // don't.
    let h = home();
    ok(
        &h,
        &["keys", "set", "--agent", "claude", "-"],
        Some(CLAUDE_KEY),
    );
    ok(
        &h,
        &["keys", "set", "--agent", "codex", "-"],
        Some(CODEX_KEY),
    );
    ok(&h, &["account", "use", "work", "--agent", "opencode"], None);

    let claude = launch_auth(&h, "claude", "ANTHROPIC_AUTH_TOKEN", &[]);
    let codex = launch_auth(&h, "codex", "OPENAI_API_KEY", &[]);
    let opencode = launch_auth(&h, "opencode", "OPENAI_API_KEY", &[]);
    let pi = launch_auth(&h, "pi", "ANYROUTER_API_KEY", &[]);
    assert!(claude.ends_with(last4(CLAUDE_KEY)), "{claude}");
    assert!(codex.ends_with(last4(CODEX_KEY)), "{codex}");
    assert!(opencode.ends_with(last4(WORK_KEY)), "{opencode}");
    assert!(pi.ends_with(last4(DEFAULT_KEY)), "unbound agent: {pi}");

    let status = ok(
        &h,
        &[
            "auth",
            "status",
            "--json",
            "--base-url",
            "http://127.0.0.1:9",
        ],
        None,
    );
    let v: serde_json::Value = serde_json::from_str(&status).unwrap();
    let rows = v["agents"].as_array().unwrap();
    let row = |agent: &str| {
        rows.iter()
            .find(|r| r["agent"] == agent)
            .unwrap_or_else(|| panic!("{agent} missing: {status}"))
            .clone()
    };
    assert_eq!(row("claude")["source"], "agent_key");
    assert_eq!(row("codex")["source"], "agent_key");
    assert_eq!(row("opencode")["source"], "agent_account");
    assert_eq!(row("opencode")["account"], "work");
    assert!(row("opencode")["key_masked"]
        .as_str()
        .unwrap()
        .ends_with(last4(WORK_KEY)));
    assert!(rows.iter().all(|r| r["agent"] != "pi"), "{status}");
}

#[test]
fn key_flag_beats_the_agent_key_for_one_run() {
    let h = home();
    ok(
        &h,
        &["keys", "set", "--agent", "claude", "-"],
        Some(CLAUDE_KEY),
    );
    let line = launch_auth(&h, "claude", "ANTHROPIC_AUTH_TOKEN", &["--key", WORK_KEY]);
    assert!(line.ends_with(last4(WORK_KEY)), "{line}");
}

#[test]
fn unset_falls_back_to_the_account_and_names_the_key_to_revoke() {
    let h = home();
    ok(&h, &["account", "use", "work", "--agent", "claude"], None);
    ok(
        &h,
        &["keys", "set", "--agent", "claude", "-"],
        Some(CLAUDE_KEY),
    );
    let out = ok(&h, &["keys", "unset", "--agent", "claude"], None);
    assert!(
        out.contains(&format!("keys revoke {}", last4(CLAUDE_KEY))),
        "{out}"
    );
    let line = launch_auth(&h, "claude", "ANTHROPIC_AUTH_TOKEN", &[]);
    assert!(line.ends_with(last4(WORK_KEY)), "{line}");
    assert!(!config(&h).contains(CLAUDE_KEY));

    let again = ok(&h, &["keys", "unset", "--agent", "claude"], None);
    assert!(again.contains("no key of its own"), "{again}");

    ok(
        &h,
        &["account", "use", "--agent", "claude", "--clear"],
        None,
    );
    let line = launch_auth(&h, "claude", "ANTHROPIC_AUTH_TOKEN", &[]);
    assert!(line.ends_with(last4(DEFAULT_KEY)), "{line}");
    assert!(!config(&h).contains("agents:"), "{}", config(&h));
}

#[test]
fn set_refuses_a_bad_key_without_echoing_or_saving_it() {
    let h = home();
    let before = config(&h);
    for bad in [
        "sk-ar-v1-short",
        "ak_managementkey0000000000000000",
        "sk-ar-v1-has space in it 000000000",
        "sk-ar-v1-testkey",
        &format!("{CLAUDE_KEY}{CODEX_KEY}"),
    ] {
        let (code, out, err) = anyr(&h, &["keys", "set", "--agent", "claude", "-"], Some(bad));
        assert_eq!(code, 1, "{bad}: {out}{err}");
        assert!(!out.contains(bad) && !err.contains(bad), "{bad} echoed");
    }
    let (code, _, err) = anyr(
        &h,
        &["keys", "set", "--agent", "vim", "-"],
        Some(CLAUDE_KEY),
    );
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("Unknown coding agent"), "{err}");
    let (code, _, err) = anyr(&h, &["keys", "set", "-"], Some(CLAUDE_KEY));
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = anyr(&h, &["keys", "set", "--agent", "claude", CLAUDE_KEY], None);
    assert_eq!(code, 2, "a key in argv is refused: {err}");
    assert_eq!(config(&h), before, "nothing saved");
}

#[test]
fn create_for_an_agent_mints_in_that_agents_account_and_saves_it() {
    // WHY: a key minted with the active account's credential but stored on an
    // agent bound to `work` would bill the wrong account.
    let h = home();
    ok(&h, &["account", "use", "work", "--agent", "codex"], None);
    let (base, rx) = serve(vec![(
        201,
        format!(r#"{{"key":"{MINTED_KEY}","data":{{"hash":"h1"}}}}"#),
    )]);
    let out = ok(
        &h,
        &[
            "keys",
            "create",
            "--agent",
            "codex",
            "--limit",
            "5",
            "--base-url",
            &base,
        ],
        None,
    );
    let seen = rx.recv().unwrap();
    assert_eq!(seen.auth, format!("Bearer {WORK_KEY}"));
    let body: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
    assert!(
        body["name"].as_str().unwrap().starts_with("cli-codex-"),
        "{body}"
    );
    assert_eq!(body["limit"], 5.0);
    assert_eq!(out.matches(MINTED_KEY).count(), 1, "shown once:\n{out}");

    let codex = launch_auth(&h, "codex", "OPENAI_API_KEY", &[]);
    assert!(codex.ends_with(last4(MINTED_KEY)), "{codex}");
    let claude = launch_auth(&h, "claude", "ANTHROPIC_AUTH_TOKEN", &[]);
    assert!(claude.ends_with(last4(DEFAULT_KEY)), "{claude}");
}

#[test]
fn create_quiet_for_an_agent_still_saves_and_prints_only_the_secret() {
    let h = home();
    let (base, _rx) = serve(vec![(
        201,
        format!(r#"{{"key":"{MINTED_KEY}","data":{{"hash":"h1"}}}}"#),
    )]);
    let out = ok(
        &h,
        &[
            "keys",
            "create",
            "--agent",
            "claude",
            "--quiet",
            "--base-url",
            &base,
        ],
        None,
    );
    assert_eq!(out.trim(), MINTED_KEY);
    let line = launch_auth(&h, "claude", "ANTHROPIC_AUTH_TOKEN", &[]);
    assert!(line.ends_with(last4(MINTED_KEY)), "{line}");
}

#[test]
fn renaming_or_removing_an_account_keeps_agent_bindings_consistent() {
    let h = home();
    ok(&h, &["account", "use", "work", "--agent", "codex"], None);
    ok(&h, &["account", "rename", "work", "job"], None);
    let codex = launch_auth(&h, "codex", "OPENAI_API_KEY", &[]);
    assert!(
        codex.ends_with(last4(WORK_KEY)),
        "rename kept binding: {codex}"
    );

    let out = ok(&h, &["account", "remove", "job"], None);
    assert!(out.contains("Unbound codex"), "{out}");
    let codex = launch_auth(&h, "codex", "OPENAI_API_KEY", &[]);
    assert!(codex.ends_with(last4(DEFAULT_KEY)), "{codex}");
    assert!(!config(&h).contains("job"), "{}", config(&h));
}

#[test]
fn create_for_an_agent_refuses_a_key_override_and_sends_nothing() {
    // WHY: `--key` or ANYROUTER_API_KEY would mint in that key's account, and
    // the env var would keep beating the new key at every launch.
    let h = home();
    ok(&h, &["account", "use", "work", "--agent", "codex"], None);
    let before = config(&h);
    let (base, rx) = serve(vec![(
        201,
        format!(r#"{{"key":"{MINTED_KEY}","data":{{"hash":"h1"}}}}"#),
    )]);
    let (code, _, err) = anyr_env(
        &h,
        &["keys", "create", "--agent", "codex", "--base-url", &base],
        None,
        &[("ANYROUTER_API_KEY", DEFAULT_KEY)],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("ANYROUTER_API_KEY is set"), "{err}");
    let (code, _, err) = anyr(
        &h,
        &[
            "keys",
            "create",
            "--agent",
            "codex",
            "--key",
            DEFAULT_KEY,
            "--base-url",
            &base,
        ],
        None,
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--key is set"), "{err}");
    assert!(
        matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
        "no request sent"
    );
    assert_eq!(config(&h), before);
}

#[test]
fn create_for_an_agent_names_a_bound_account_with_no_key() {
    let h = home();
    std::fs::write(
        h.join("config.yaml"),
        format!("{}  empty:\n    default_model: auto\n", config(&h)),
    )
    .unwrap();
    ok(&h, &["account", "use", "empty", "--agent", "pi"], None);
    let (code, _, err) = anyr(&h, &["keys", "create", "--agent", "pi"], None);
    assert_eq!(code, 4, "{err}");
    assert!(err.contains("\"empty\" (bound to pi) has no key"), "{err}");
}

#[test]
fn create_for_an_agent_under_another_profile_says_its_key_wins() {
    let h = home();
    ok(&h, &["account", "use", "work", "--agent", "codex"], None);
    let (base, rx) = serve(vec![(
        201,
        format!(r#"{{"key":"{MINTED_KEY}","data":{{"hash":"h1"}}}}"#),
    )]);
    let (code, out, err) = anyr_env(
        &h,
        &[
            "keys",
            "create",
            "--agent",
            "codex",
            "--quiet",
            "--base-url",
            &base,
        ],
        None,
        &[("ANYROUTER_PROFILE", "default")],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), MINTED_KEY);
    assert_eq!(rx.recv().unwrap().auth, format!("Bearer {DEFAULT_KEY}"));
    assert!(
        err.contains("bound to work, but this key is in default"),
        "{err}"
    );
}
