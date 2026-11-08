mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::thread;

fn anyr() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.env("ANYR_NO_UPDATE", "1");
    // Skip live catalog lookup so auto stays anyrouter/auto in tests.
    cmd.env("ANYR_NO_CATALOG", "1");
    cmd
}

/// Fresh empty ANYROUTER_HOME so launch tests assert on built-in defaults
/// instead of whatever happens to live in the developer's real config.
fn temp_home() -> std::path::PathBuf {
    common::unique_temp_dir("anyr-cli-test")
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = anyr().args(args).output().expect("spawn anyr");
    (
        out.status.code().unwrap_or(1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn decision_server() -> (String, thread::JoinHandle<(String, String)>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind decision test server");
    let address = listener.local_addr().expect("decision test address");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept decision request");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .expect("set decision test timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let (header_end, content_length) = loop {
            let count = stream.read(&mut buffer).expect("read decision request");
            assert!(count > 0, "decision request closed before headers");
            request.extend_from_slice(&buffer[..count]);
            if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let header_end = end + 4;
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.trim()
                            .eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + length {
                    break (header_end, length);
                }
            }
        };
        let body = String::from_utf8(request[header_end..header_end + content_length].to_vec())
            .expect("decision request body utf8");
        let response = br#"{"model":"typesafe/jev","answers":{"urgent":{"type":"noul","noul":0.9}},"usage":{"input_tokens":4,"output_tokens":1}}"#;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .as_bytes(),
            )
            .expect("write decision response headers");
        stream.write_all(response).expect("write decision response");
        (
            String::from_utf8_lossy(&request[..header_end]).into_owned(),
            body,
        )
    });
    (format!("http://{address}/api"), handle)
}

/// Write a stand-in "claude" executable that exits 0 whatever args it receives
/// (claude-style flags break bare `cmd` on Windows). Returns the path to pass
/// as ANYROUTER_CLAUDE_PATH; the file lives in its own temp dir.
fn exit_zero_stub() -> std::path::PathBuf {
    let dir = common::unique_temp_dir("anyr-stub");
    #[cfg(windows)]
    let path = dir.join("stub.cmd");
    #[cfg(not(windows))]
    let path = dir.join("stub.sh");
    #[cfg(windows)]
    let body = "@echo off\r\nexit /b 0\r\n";
    #[cfg(not(windows))]
    let body = "#!/bin/sh\nexit 0\n";
    std::fs::write(&path, body).expect("write stub");
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod stub");
    }
    path
}

#[test]
fn help_lists_login_claude_account_and_spawn_targets() {
    let (code, stdout, stderr) = run(&["--help"]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("point any coding agent"), "{stdout}");
    assert!(stdout.contains("anyr login"), "{stdout}");
    assert!(stdout.contains("anyr claude"), "{stdout}");
    assert!(stdout.contains("anyr help --all"), "{stdout}");
    assert!(
        !stdout.contains("Install:"),
        "help must not dump an Install: heading, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("anyrouter.dev/docs/cli"),
        "help must not end with a docs URL, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("npx @anyr/cli"),
        "native anyr --help must not tell people to type npx, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("CORE COMMANDS"),
        "examples-first --help must not dump the catalog:\n{stdout}"
    );
    let (code, map, map_err) = run(&["help", "--all"]);
    assert_eq!(code, 0, "{map}{map_err}");
    let (_, legacy, _) = run(&["help", "commands"]);
    assert_eq!(
        map, legacy,
        "`help commands` stays an alias of `help --all`"
    );
    for heading in ["SIGN IN", "CHAT AND MODELS", "LAUNCH", "ACCOUNT"] {
        assert!(map.contains(heading), "missing {heading} in:\n{map}");
    }
    for target in anyr_cli::registry::LAUNCHERS {
        assert!(map.contains(target), "missing {target} in:\n{map}");
    }
    assert!(map.contains("▀█████████▄"), "{map}");
}

#[test]
fn decision_help_and_aliases_are_documented() {
    for command in ["decision", "decisions", "systemone"] {
        let (code, stdout, stderr) = run(&[command, "--help"]);
        assert_eq!(code, 0, "{command} --help failed: {stdout}{stderr}");
        for needle in ["/api/v1/decisions", "--model", "--state", "--questions"] {
            assert!(
                stdout.contains(needle),
                "{command} help missing {needle}: {stdout}"
            );
        }
    }
}

#[test]
fn decision_posts_native_envelope_with_configured_auth() {
    let (base_url, server) = decision_server();
    let key = "sk-ar-v1-local-decision-key-1234";
    let (code, stdout, stderr) = {
        let out = anyr()
            .args([
                "decision",
                "--model",
                "typesafe/jev",
                "--state",
                "Help! My payouts have been failing for 3 days.",
                "--questions",
                r#"{"is_urgent":{"type":"noul","instructions":"Does this convey urgency?"}}"#,
                "--key",
                key,
                "--base-url",
                base_url.as_str(),
            ])
            .env("ANYROUTER_HOME", temp_home())
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("decision request");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let (headers, body) = server.join().expect("decision server");
    assert_eq!(code, 0, "stderr={stderr} stdout={stdout}");
    assert!(stdout.contains("\"answers\""), "{stdout}");
    assert!(
        !stdout.contains(key),
        "decision output leaked the API key: {stdout}"
    );
    assert!(headers.starts_with("POST /api/v1/decisions "), "{headers}");
    assert!(
        headers
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {key}")),
        "missing authorization header: {headers}"
    );
    let payload: serde_json::Value = serde_json::from_str(&body).expect("decision request JSON");
    assert_eq!(payload["model"], "typesafe/jev");
    assert_eq!(
        payload["state"],
        "Help! My payouts have been failing for 3 days."
    );
    assert_eq!(payload["questions"]["is_urgent"]["type"], "noul");
    assert_eq!(payload.as_object().unwrap().len(), 3);
}

#[test]
fn decision_accepts_a_complete_json_object_on_stdin() {
    let (base_url, server) = decision_server();
    let key = "sk-ar-v1-local-decision-key-5678";
    let mut child = anyr()
        .args(["decision", "--key", key, "--base-url", base_url.as_str()])
        .env("ANYROUTER_HOME", temp_home())
        .env_remove("ANYROUTER_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdin decision");
    child
        .stdin
        .take()
        .expect("decision stdin")
        .write_all(
            br#"{"model":"anyrouter/decision","state":{"order":"failed"},"questions":{"urgent":{"type":"noul"}}}"#,
        )
        .expect("write decision stdin");
    let output = child.wait_with_output().expect("wait for stdin decision");
    let (headers, body) = server.join().expect("decision stdin server");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(headers.starts_with("POST /api/v1/decisions "), "{headers}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("stdin request JSON");
    assert_eq!(payload["model"], "anyrouter/decision");
    assert_eq!(payload["state"]["order"], "failed");
    assert_eq!(payload["questions"]["urgent"]["type"], "noul");
}

#[test]
fn chat_launch_refuses_systemone_models_before_login() {
    let (code, _stdout, stderr) = run(&["claude", "--model", "typesafe/jev", "--dry-run"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("Decisions model"), "{stderr}");
    assert!(stderr.contains("anyr decision"), "{stderr}");
}

#[test]
fn no_args_non_tty_prints_grouped_help() {
    let (code, stdout, stderr) = run(&[]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("point any coding agent") && stdout.contains("anyr login"),
        "no-args should print examples-first help when not a TTY, got:\n{stdout}"
    );
}

#[test]
fn help_follows_anyr_display_bin() {
    for (name, needle) in [
        ("ar", "ar claude"),
        ("anyrouter", "anyrouter claude"),
        ("npx @anyr/cli", "npx @anyr/cli claude"),
    ] {
        let out = anyr()
            .args(["--help"])
            .env("ANYR_DISPLAY_BIN", name)
            .output()
            .expect("spawn anyr");
        assert_eq!(out.status.code().unwrap_or(1), 0);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(needle),
            "ANYR_DISPLAY_BIN={name} missing {needle:?} in:\n{stdout}"
        );
        assert!(
            stdout.contains(&format!("{name} help --all"))
                || stdout.contains(&format!("{name} login")),
            "ANYR_DISPLAY_BIN={name} missing usage line in:\n{stdout}"
        );
    }
}

#[test]
fn login_help_follows_display_bin() {
    let out = anyr()
        .args(["login", "--help"])
        .env("ANYR_DISPLAY_BIN", "ar")
        .output()
        .expect("spawn anyr");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code().unwrap_or(1), 0);
    assert!(stdout.contains("ar auth login"), "{stdout}");
    assert!(!stdout.contains("npx @anyr/cli"), "{stdout}");
}

#[test]
fn auth_help_lists_gh_style_subcommands() {
    let (code, stdout, stderr) = run(&["auth", "--help"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    for sub in ["login", "logout", "status", "token", "switch"] {
        assert!(stdout.contains(sub), "auth --help missing {sub}:\n{stdout}");
    }
}

#[test]
fn auth_login_help_is_nested() {
    let (code, stdout, stderr) = run(&["auth", "login", "--help"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("auth login"), "{stdout}");
}

#[test]
fn auth_unknown_subcommand_errors() {
    let (code, stdout, stderr) = run(&["auth", "nope"]);
    assert_ne!(code, 0);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("unknown command") && combined.contains("auth"),
        "{combined}"
    );
}

#[test]
fn version_matches_0_1() {
    let (code, stdout, stderr) = run(&["--version"]);
    assert_eq!(code, 0, "stderr={stderr}");
    let ver = stdout.trim();
    assert!(
        ver.starts_with("0.1."),
        "expected --version to match ^0\\.1\\. got {ver:?}"
    );
}

#[test]
fn login_usage_whoami_help_exit_zero() {
    for cmd in ["login", "usage", "whoami"] {
        let (code, stdout, stderr) = run(&[cmd, "--help"]);
        assert_eq!(code, 0, "{cmd} --help failed: {stdout}{stderr}");
    }
}

#[test]
fn documented_commands_are_known_not_unknown() {
    // Drive the shipped binary — not packages/cli/bin/cli.mjs.
    let commands = [
        "chat",
        "setup",
        "models",
        "config",
        "skills",
        "relay",
        "byok",
        "task",
        "delegate",
        "keys",
        "logs",
        "generation",
        "gen",
        "presets",
        "mcp",
        "audit",
        "logout",
        "transactions",
        "account",
        "auth",
        "login",
        "usage",
        "whoami",
        "claude",
        "codex",
        "grok",
        "opencode",
        "pi",
        "pool",
        "poolside",
        "qwen",
        "goose",
        "aider",
        "crush",
        "kilo",
        "fx",
        "upgrade",
        "update",
        "onboard",
        "impl",
        "plan",
        "fix",
        "deploy",
        "cp",
    ];
    for cmd in commands {
        let (code, stdout, stderr) = run(&[cmd, "--help"]);
        let combined = format!("{stdout}{stderr}");
        assert_eq!(code, 0, "{cmd} --help failed: {combined}");
        assert!(
            !combined.contains("Unknown command"),
            "{cmd} treated as unknown:\n{combined}"
        );
        assert!(!stdout.is_empty(), "{cmd} --help printed nothing");
    }
}

#[test]
fn onboard_impl_prints_contract_prompt() {
    let (code, stdout, stderr) = run(&["onboard", "impl"]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("ANYROUTER_API_KEY"), "{stdout}");
    assert!(stdout.contains("https://anyrouter.dev/api/v1"), "{stdout}");
    assert!(stdout.contains("https://anyrouter.dev/api"), "{stdout}");
}

#[test]
fn onboard_shortcuts_and_json() {
    for cmd in ["impl", "plan", "fix", "deploy"] {
        let (code, stdout, stderr) = run(&[cmd]);
        assert_eq!(code, 0, "{cmd} stderr={stderr}");
        assert!(!stdout.is_empty(), "{cmd} empty");
    }
    let (code, stdout, stderr) = run(&["onboard", "plan", "--json"]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("\"mode\":\"plan\"") || stdout.contains("\"mode\": \"plan\""),
        "{stdout}"
    );
    assert!(
        stdout.to_ascii_lowercase().contains("do not change"),
        "{stdout}"
    );
}

#[test]
fn onboard_without_mode_non_tty_errors() {
    let (code, stdout, stderr) = run(&["onboard"]);
    assert_ne!(code, 0);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("Specify a mode") || combined.contains("onboard --help"),
        "{combined}"
    );
}

#[test]
fn stub_help_is_honest() {
    let (code, stdout, stderr) = run(&["skills", "--help"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains("not yet in the native CLI"),
        "expected honest stub help, got:\n{stdout}"
    );
}

#[test]
fn spawn_targets_dry_run_inject_gateway_and_redact_key() {
    let key = "sk-ar-v1-fixture-key-0001";
    let cases: &[(&[&str], &str)] = &[
        (
            &["claude", "--dry-run", "--yes", "--key", key],
            "ANTHROPIC_BASE_URL",
        ),
        (
            &["codex", "--dry-run", "--yes", "--key", key],
            "OPENAI_BASE_URL",
        ),
        (
            &["opencode", "--dry-run", "--yes", "--key", key],
            "OPENCODE_CONFIG_CONTENT",
        ),
        (
            &["pi", "--dry-run", "--yes", "--key", key],
            "ANYROUTER_API_KEY",
        ),
    ];
    for (args, marker) in cases {
        let out = anyr()
            .args(*args)
            .env("ANYROUTER_HOME", temp_home())
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("dry-run");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code().unwrap_or(1),
            0,
            "{args:?} stderr={stderr} stdout={stdout}"
        );
        assert!(
            stdout.contains(marker),
            "{args:?} missing {marker} in:\n{stdout}"
        );
        assert!(!stdout.contains(key), "{args:?} leaked full key:\n{stdout}");
    }
}

#[test]
fn pi_dry_run_uses_anyrouter_provider() {
    let key = "sk-ar-v1-fixture-key-0001";
    let dir = std::env::temp_dir().join(format!("anyr-pi-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.yaml");
    let (code, stdout, stderr) = {
        let out = anyr()
            .args([
                "pi",
                "--dry-run",
                "--yes",
                "--key",
                key,
                "--model",
                "z-ai/glm-4.7-flash",
                "--config",
                cfg.to_str().unwrap(),
            ])
            .env("ANYROUTER_HOME", temp_home())
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("pi dry-run");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("command: pi"), "{stdout}");
    assert!(stdout.contains("--provider"), "{stdout}");
    assert!(stdout.contains("anyrouter"), "{stdout}");
    assert!(stdout.contains("--model"), "{stdout}");
    assert!(stdout.contains("z-ai/glm-4.7-flash"), "{stdout}");
    assert!(
        !stdout.contains("anyrouter/z-ai/glm-4.7-flash"),
        "Pi --model is the catalog id, not anyrouter/<id>:\n{stdout}"
    );
    assert!(stdout.contains("PI_CODING_AGENT_DIR"), "{stdout}");
    assert!(stdout.contains("ANYROUTER_API_KEY"), "{stdout}");
    assert!(stdout.contains("anyrouter.dev/api/v1"), "{stdout}");
    assert!(!stdout.contains(key), "full key leaked:\n{stdout}");
    let models = std::fs::read_to_string(dir.join("pi").join("models.json")).unwrap();
    // Current Pi sends a bare `ANYROUTER_API_KEY` string as the key itself (401).
    assert!(
        models.contains(r#""apiKey": "!printf %s \"$ANYROUTER_API_KEY\"""#),
        "{models}"
    );
    assert!(
        !models.contains(key),
        "full key written to models.json:\n{models}"
    );
    assert!(models.contains("z-ai/glm-4.7-flash"), "{models}");
    let settings = std::fs::read_to_string(dir.join("pi").join("settings.json")).unwrap();
    assert!(
        settings.contains("\"defaultProvider\": \"anyrouter\""),
        "{settings}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn claude_dry_run_omitted_model_uses_anyrouter_auto() {
    let key = "sk-ar-v1-fixture-key-0001";
    let (code, stdout, stderr) = {
        let out = anyr()
            .args(["claude", "--dry-run", "--yes", "--key", key])
            .env("ANYROUTER_HOME", temp_home())
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("dry-run");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("ANTHROPIC_MODEL=anyrouter/auto"),
        "omitted --model must send the documented auto preset:\n{stdout}"
    );
    assert!(stdout.contains("ANYROUTER_MODEL_MODE=auto"), "{stdout}");
    assert!(
        !stdout.to_ascii_lowercase().contains("most used"),
        "{stdout}"
    );
}

#[test]
fn claude_dry_run_model_auto_1m_sets_min_context_in_extra_body() {
    let key = "sk-ar-v1-fixture-key-0001";
    let out = anyr()
        .args([
            "claude",
            "--dry-run",
            "--yes",
            "--key",
            key,
            "--model",
            "anyrouter/auto[1m]",
        ])
        .env("ANYROUTER_HOME", temp_home())
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("dry-run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("ANTHROPIC_MODEL=anyrouter/auto[1m]"),
        "Claude HUD needs the [1m] suffix on virtual auto:\n{stdout}"
    );
    // The floor is carried into CLAUDE_CODE_EXTRA_BODY under provider.min_context.
    assert!(stdout.contains("CLAUDE_CODE_EXTRA_BODY="), "{stdout}");
    assert!(
        stdout.contains("\"min_context\":1000000"),
        "expected min_context 1_000_000 in extra body:\n{stdout}"
    );
    assert!(
        stdout.contains("\"provider\""),
        "expected provider object in extra body:\n{stdout}"
    );
}

#[test]
fn claude_dry_run_model_auto_500k_sets_min_context_in_extra_body() {
    let key = "sk-ar-v1-fixture-key-0001";
    let out = anyr()
        .args([
            "claude",
            "--dry-run",
            "--yes",
            "--key",
            key,
            "--model",
            "anyrouter/auto[500k]",
        ])
        .env("ANYROUTER_HOME", temp_home())
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("dry-run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("ANTHROPIC_MODEL=anyrouter/auto[500k]"),
        "Claude HUD needs the [500k] suffix on virtual auto:\n{stdout}"
    );
    // The floor is carried into CLAUDE_CODE_EXTRA_BODY under provider.min_context.
    assert!(stdout.contains("CLAUDE_CODE_EXTRA_BODY="), "{stdout}");
    assert!(
        stdout.contains("\"min_context\":500000"),
        "expected min_context 500_000 in extra body:\n{stdout}"
    );
    assert!(
        stdout.contains("\"provider\""),
        "expected provider object in extra body:\n{stdout}"
    );
}

#[test]
fn claude_help_documents_anyrouter_auto_not_invented_skus() {
    let (code, stdout, stderr) = run(&["claude", "--help"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("anyrouter/auto"), "{stdout}");
    assert!(
        !stdout.to_ascii_lowercase().contains("most-used"),
        "help must not claim auto invents a usage SKU:\n{stdout}"
    );
    // Help documents the min-context floor suffixes.
    assert!(stdout.contains("[1m]"), "{stdout}");
    assert!(stdout.contains("[500k]"), "{stdout}");
}

#[test]
fn claude_dry_run_with_key_prints_base_and_redacts_secret() {
    let key = "sk-ar-v1-fixture-key-0001";
    let (code, stdout, stderr) = {
        let out = anyr()
            .args([
                "claude",
                "--dry-run",
                "--yes",
                "--key",
                key,
                "--model",
                "auto",
            ])
            .env("ANYROUTER_HOME", temp_home())
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("dry-run");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("ANTHROPIC_BASE_URL"), "{stdout}");
    assert!(
        stdout.contains("ANTHROPIC_MODEL=anyrouter/auto"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ANTHROPIC_DEFAULT_HAIKU_MODEL=anyrouter/free"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ANTHROPIC_DEFAULT_SONNET_MODEL=anyrouter/free"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ANTHROPIC_DEFAULT_OPUS_MODEL=anyrouter/free"),
        "{stdout}"
    );
    assert!(!stdout.contains(key), "full key leaked:\n{stdout}");
}

#[test]
fn claude_dry_run_pinned_model_collapses_aliases() {
    let key = "sk-ar-v1-fixture-key-0001";
    let (code, stdout, stderr) = {
        let out = anyr()
            .args([
                "claude",
                "--dry-run",
                "--yes",
                "--key",
                key,
                "--model",
                "stealth/ox-alpha",
            ])
            .env("ANYROUTER_HOME", temp_home())
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("dry-run");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("ANTHROPIC_MODEL=stealth/ox-alpha"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("ANTHROPIC_MODEL=stealth/ox-alpha[1m]"),
        "{stdout}"
    );
    // Unset alias slots follow the pinned model so nothing falls back to
    // haiku/sonnet/opus behind the user's back.
    for key_line in [
        "ANTHROPIC_DEFAULT_HAIKU_MODEL=stealth/ox-alpha",
        "ANTHROPIC_DEFAULT_SONNET_MODEL=stealth/ox-alpha",
        "ANTHROPIC_DEFAULT_OPUS_MODEL=stealth/ox-alpha",
        "CLAUDE_CODE_SUBAGENT_MODEL=stealth/ox-alpha",
    ] {
        assert!(stdout.contains(key_line), "missing {key_line}:\n{stdout}");
    }
}

#[test]
fn claude_dry_run_haiku_flag_beats_pinned_model() {
    let key = "sk-ar-v1-fixture-key-0001";
    let (code, stdout, stderr) = {
        let out = anyr()
            .args([
                "claude",
                "--dry-run",
                "--yes",
                "--key",
                key,
                "--model",
                "stealth/ox-alpha",
                "--haiku",
                "z-ai/glm-4.7-flash",
            ])
            .env("ANYROUTER_HOME", temp_home())
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("dry-run");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("ANTHROPIC_DEFAULT_HAIKU_MODEL=z-ai/glm-4.7-flash"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ANTHROPIC_DEFAULT_SONNET_MODEL=stealth/ox-alpha"),
        "{stdout}"
    );
}

#[test]
fn claude_yolo_flag_expands_to_dangerously_skip_permissions() {
    let key = "sk-ar-v1-fixture-key-0001";
    let (code, stdout, stderr) = {
        let out = anyr()
            .args(["claude", "--dry-run", "--yes", "--key", key, "--yolo"])
            .env("ANYROUTER_HOME", temp_home())
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("dry-run");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("\"--dangerously-skip-permissions\""),
        "--yolo should expand to --dangerously-skip-permissions in args:\n{stdout}"
    );
}

#[test]
fn claude_yolo_with_ox_alpha_1m_still_works() {
    let key = "sk-ar-v1-fixture-key-0001";
    let (code, stdout, stderr) = {
        let out = anyr()
            .args([
                "claude",
                "--dry-run",
                "--yes",
                "--key",
                key,
                "--model",
                "stealth/ox-alpha[1m]",
                "--yolo",
            ])
            .env("ANYROUTER_HOME", temp_home())
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("dry-run");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("ANTHROPIC_MODEL=stealth/ox-alpha"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("ANTHROPIC_MODEL=stealth/ox-alpha[1m]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\"--dangerously-skip-permissions\""),
        "{stdout}"
    );
}

#[test]
fn codex_yolo_flag_maps_to_codex_bypass_flag() {
    let key = "sk-ar-v1-fixture-key-0001";
    let (code, stdout, stderr) = {
        let out = anyr()
            .args(["codex", "--dry-run", "--yes", "--key", key, "--yolo"])
            .env("ANYROUTER_HOME", temp_home())
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("dry-run");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("\"--dangerously-bypass-approvals-and-sandbox\""),
        "--yolo should map to codex's own bypass flag:\n{stdout}"
    );
    assert!(
        !stdout.contains("--dangerously-skip-permissions"),
        "{stdout}"
    );
}

#[test]
fn claude_dry_run_fable_flag_beats_pinned_model() {
    let key = "sk-ar-v1-fixture-key-0001";
    let (code, stdout, stderr) = {
        let out = anyr()
            .args([
                "claude",
                "--dry-run",
                "--yes",
                "--key",
                key,
                "--model",
                "stealth/ox-alpha",
                "--fable",
                "anthropic/claude-fable-5",
            ])
            .env("ANYROUTER_HOME", temp_home())
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("dry-run");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    // Explicit --fable wins over the pinned session model...
    assert!(
        stdout.contains("ANTHROPIC_DEFAULT_FABLE_MODEL=anthropic/claude-fable-5"),
        "{stdout}"
    );
    // ...while every other unset slot still follows the pin.
    for key_line in [
        "ANTHROPIC_DEFAULT_SONNET_MODEL=stealth/ox-alpha",
        "ANTHROPIC_DEFAULT_OPUS_MODEL=stealth/ox-alpha",
    ] {
        assert!(stdout.contains(key_line), "missing {key_line}:\n{stdout}");
    }
}

#[test]
fn claude_dry_run_haiku_flag_overrides_alias() {
    let key = "sk-ar-v1-fixture-key-0001";
    let (code, stdout, stderr) = {
        let out = anyr()
            .args([
                "claude",
                "--dry-run",
                "--yes",
                "--key",
                key,
                // Keep the session model on auto so this test does not depend
                // on the developer's ~/.anyrouter config default_model.
                "--model",
                "auto",
                "--haiku",
                "z-ai/glm-4.7-flash",
            ])
            .env("ANYROUTER_HOME", temp_home())
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("dry-run");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("ANTHROPIC_DEFAULT_HAIKU_MODEL=z-ai/glm-4.7-flash"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ANTHROPIC_DEFAULT_SONNET_MODEL=anyrouter/free"),
        "{stdout}"
    );
}

#[test]
fn model_without_value_errors() {
    let (code, stdout, stderr) = run(&["claude", "--model"]);
    assert_ne!(code, 0);
    assert!(format!("{stdout}{stderr}").contains("requires a value"));
}

#[test]
fn launch_does_not_persist_env_or_flag_key_to_config() {
    // #202: an env-only (or --key) key must stay in the process env; launch
    // may write config.yaml (last_tool, default_model) but never the secret.
    let stub_path = exit_zero_stub();
    let stub = stub_path.to_str().unwrap();
    for variant in ["env", "flag"] {
        let home = temp_home();
        let mut args = vec!["claude", "--yes", "--model", "z-ai/glm-4.7-flash"];
        if variant == "flag" {
            args.extend(["--key", "sk-ar-v1-ephemeral-flag-key-0002"]);
        }
        let mut cmd = anyr();
        cmd.args(&args)
            .env("ANYROUTER_HOME", &home)
            .env("ANYROUTER_CLAUDE_PATH", stub);
        if variant == "env" {
            cmd.env("ANYROUTER_API_KEY", "sk-ar-v1-ephemeral-env-key-0001");
        } else {
            cmd.env_remove("ANYROUTER_API_KEY");
        }
        let out = cmd.output().expect("launch");
        assert_eq!(out.status.code().unwrap_or(1), 0, "{variant}");
        if let Ok(cfg) = std::fs::read_to_string(home.join("config.yaml")) {
            assert!(
                !cfg.contains("sk-ar-v1"),
                "{variant} leaked a key into config.yaml:\n{cfg}"
            );
            assert!(cfg.contains("last_tool: claude"), "{variant}:\n{cfg}");
            assert!(
                cfg.contains("default_model: z-ai/glm-4.7-flash"),
                "{variant}:\n{cfg}"
            );
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}

#[test]
fn launch_remembers_explicit_model_as_session_default() {
    // Launching with --model must persist it as default_model so a bare
    // `{bin} claude` next time starts with the same model. A trivially
    // successful stub stands in for claude: a script file (not `cmd`, which
    // rejects/hangs on claude-style flags) that exits 0 whatever args arrive.
    let stub_path = exit_zero_stub();
    let stub = stub_path.to_str().unwrap();
    let home = temp_home();
    let out = anyr()
        .args([
            "claude",
            "--yes",
            "--key",
            "sk-ar-v1-fixture-key-0001",
            "--model",
            "z-ai/glm-4.7-flash",
        ])
        .env("ANYROUTER_HOME", &home)
        .env("ANYROUTER_CLAUDE_PATH", stub)
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("launch");
    assert_eq!(out.status.code().unwrap_or(1), 0);

    // A Claude `[1m]` suffix must not be written into config (Pi/Codex 404 on it).
    let home_suffix = temp_home();
    let out = anyr()
        .args([
            "claude",
            "--yes",
            "--key",
            "sk-ar-v1-fixture-key-0001",
            "--model",
            "stealth/ox-alpha[1m]",
        ])
        .env("ANYROUTER_HOME", &home_suffix)
        .env("ANYROUTER_CLAUDE_PATH", stub)
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("launch suffix");
    assert_eq!(out.status.code().unwrap_or(1), 0);
    let cfg_suffix =
        std::fs::read_to_string(home_suffix.join("config.yaml")).expect("config written");
    assert!(
        cfg_suffix.contains("default_model: stealth/ox-alpha"),
        "{cfg_suffix}"
    );
    assert!(
        !cfg_suffix.contains("stealth/ox-alpha[1m]"),
        "catalog id only:\n{cfg_suffix}"
    );
    let _ = std::fs::remove_dir_all(&home_suffix);

    // Second launch with no --model: dry-run reveals which model was picked.
    let out = anyr()
        .args([
            "claude",
            "--yes",
            "--dry-run",
            "--key",
            "sk-ar-v1-fixture-key-0001",
        ])
        .env("ANYROUTER_HOME", &home)
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("relaunch");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ANTHROPIC_MODEL=z-ai/glm-4.7-flash"),
        "session default not remembered:\n{stdout}"
    );

    // And the persisted config records it too.
    let cfg = std::fs::read_to_string(home.join("config.yaml")).expect("config written");
    assert!(cfg.contains("default_model: z-ai/glm-4.7-flash"), "{cfg}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn upgrade_help_mentions_channel_stable_beta() {
    let (code, stdout, stderr) = run(&["upgrade", "--help"]);
    assert_eq!(code, 0, "stderr={stderr}");
    let combined = format!("{stdout}{stderr}");
    assert!(
        !combined.contains("Unknown command"),
        "upgrade treated as unknown:\n{combined}"
    );
    assert!(stdout.contains("channel"), "{stdout}");
    assert!(stdout.contains("stable"), "{stdout}");
    assert!(stdout.contains("beta"), "{stdout}");
    assert!(stdout.contains("--beta"), "{stdout}");
    assert!(stdout.contains("--stable"), "{stdout}");
    assert!(
        stdout.contains("Auto-update") || stdout.contains("auto-update"),
        "upgrade help should mention auto-update, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Run anyr to start using the new version."),
        "upgrade help should show the post-update hint, got:\n{stdout}"
    );
    // Nothing handles Ctrl+G; the old hint sent users to a key that did nothing.
    assert!(!stdout.contains("Ctrl+G"), "{stdout}");
    assert!(stdout.contains("--rollback"), "{stdout}");
}

#[test]
fn update_is_upgrade_alias() {
    let (code, stdout, stderr) = run(&["update", "--help"]);
    assert_eq!(code, 0, "stderr={stderr}");
    let combined = format!("{stdout}{stderr}");
    assert!(
        !combined.contains("Unknown command"),
        "update treated as unknown:\n{combined}"
    );
    assert!(stdout.contains("--beta"), "{stdout}");
    assert!(stdout.contains("--stable"), "{stdout}");
}

#[test]
fn update_beta_persists_channel_and_selects_prerelease() {
    let home = std::env::temp_dir().join(format!(
        "anyr-cli-update-beta-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&home).expect("home");
    let (code, stdout, stderr) = {
        let out = anyr()
            .args(["update", "--beta", "--check"])
            .env("ANYROUTER_HOME", &home)
            .env("ANYR_RELEASES_JSON", fixture_path())
            .env_remove("ANYR_CHANNEL")
            .output()
            .expect("update --beta --check");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("channel set to beta"), "{stdout}");
    assert!(stdout.contains("channel: beta"), "{stdout}");
    assert!(
        stdout.contains("latest:") && stdout.contains("0.2.0-beta.1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("current:") && stdout.contains("status:"),
        "{stdout}"
    );
    let cfg = std::fs::read_to_string(home.join("config.yaml")).expect("config");
    assert!(cfg.contains("channel: beta"), "{cfg}");
}

#[test]
fn update_stable_and_beta_conflict() {
    let (code, stdout, stderr) = run(&["update", "--beta", "--stable", "--check"]);
    assert_ne!(code, 0);
    let combined = format!("{stdout}{stderr}");
    assert!(combined.contains("either --beta or --stable"), "{combined}");
}

#[test]
fn upgrade_check_flag_is_known() {
    // Isolate from a real ~/.anyrouter whose channel would skew the check.
    let home = std::env::temp_dir().join(format!("anyr-cli-upgrade-home-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let fixture = std::env::temp_dir().join(format!(
        "anyr-cli-upgrade-check-{}.json",
        std::process::id()
    ));
    std::fs::write(
        &fixture,
        r#"[{"tag_name":"v0.1.0","prerelease":false,"draft":false,"assets":[{"name":"anyr-linux-x86_64","browser_download_url":"https://github.com/AnyRouterHQ/cli/releases/download/v0.1.0/anyr-linux-x86_64"}]}]"#,
    )
    .expect("write fixture");
    let out = anyr()
        .args(["upgrade", "--check"])
        .env("ANYR_RELEASES_JSON", &fixture)
        .env("ANYROUTER_HOME", &home)
        .env_remove("ANYR_CHANNEL")
        .output()
        .expect("upgrade --check");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}{stderr}");
    assert_eq!(out.status.code().unwrap_or(1), 0, "{combined}");
    assert!(
        !combined.contains("Unknown command"),
        "upgrade --check treated as unknown:\n{combined}"
    );
    assert!(
        stdout.contains("up to date") || stdout.contains("ahead"),
        "{stdout}"
    );
}

fn fixture_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/releases.json")
}

#[test]
fn upgrade_check_newer_stable_would_upgrade() {
    let (code, stdout, stderr) = {
        let out = anyr()
            .args(["upgrade", "--check", "--channel", "stable"])
            .env("ANYR_RELEASES_JSON", fixture_path())
            .env_remove("ANYR_CHANNEL")
            .output()
            .expect("upgrade --check stable");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("channel: stable"), "{stdout}");
    assert!(
        stdout.contains("latest:") && stdout.contains("0.1.99"),
        "{stdout}"
    );
    assert!(stdout.contains("update available"), "{stdout}");
    assert!(
        stdout.contains("->") && stdout.contains("0.1.99"),
        "check should show old -> new, got:\n{stdout}"
    );
    assert!(
        stdout.contains("github.com/AnyRouterHQ/cli/releases/download/"),
        "{stdout}"
    );
}

#[test]
fn upgrade_check_beta_selects_prerelease() {
    let (code, stdout, stderr) = {
        let out = anyr()
            .args(["upgrade", "--check", "--channel", "beta"])
            .env("ANYR_RELEASES_JSON", fixture_path())
            .env("ANYR_CHANNEL", "stable")
            .output()
            .expect("upgrade --check beta");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("channel: beta"), "{stdout}");
    assert!(
        stdout.contains("latest:") && stdout.contains("0.2.0-beta.1"),
        "{stdout}"
    );
    assert!(stdout.contains("update available"), "{stdout}");
}

fn empty_latest_fixture() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/releases-empty-latest.json")
}

#[test]
fn update_stable_empty_latest_is_actionable_not_http_403() {
    let (code, stdout, stderr) = {
        let out = anyr()
            .args(["update", "--check", "--channel", "stable"])
            .env("ANYR_RELEASES_JSON", empty_latest_fixture())
            .env_remove("ANYR_CHANNEL")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .output()
            .expect("update --check empty stable");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let combined = format!("{stdout}{stderr}");
    assert_ne!(code, 0, "empty stable should fail:\n{combined}");
    assert!(
        !combined.contains("GitHub Releases API HTTP 403"),
        "bare 403:\n{combined}"
    );
    assert!(
        combined.contains("update --beta") || combined.contains("anyr-linux-x86_64"),
        "{combined}"
    );
}

#[test]
fn update_beta_empty_latest_catalog_selects_prerelease() {
    let (code, stdout, stderr) = {
        let out = anyr()
            .args(["update", "--check", "--channel", "beta"])
            .env("ANYR_RELEASES_JSON", empty_latest_fixture())
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .output()
            .expect("update --check beta empty-latest catalog");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let combined = format!("{stdout}{stderr}");
    assert_eq!(code, 0, "{combined}");
    assert!(
        !combined.contains("GitHub Releases API HTTP 403"),
        "{combined}"
    );
    assert!(stdout.contains("channel: beta"), "{stdout}");
    assert!(
        stdout.contains("latest:") && stdout.contains("0.1.12-beta.98"),
        "{stdout}"
    );
}

#[test]
fn login_help_describes_device_and_paste() {
    let (code, stdout, stderr) = run(&["login", "--help"]);
    assert_eq!(code, 0, "stderr={stderr}");
    let combined = format!("{stdout}{stderr}");
    assert!(combined.contains("--device"), "{combined}");
    assert!(combined.contains("--paste"), "{combined}");
    assert!(combined.contains("device"), "{combined}");
    assert!(
        combined.contains("claude"),
        "login help must point at anyr claude:\n{combined}"
    );
    assert!(
        !combined.to_ascii_lowercase().contains("wizard"),
        "login must not mention a post-login wizard:\n{combined}"
    );
}

#[test]
fn claude_help_starts_immediately() {
    // WHY: `anyr claude` is the post-install path. Help must not send the
    // user through a settings launcher first.
    let (code, stdout, stderr) = run(&["claude", "--help"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let combined = format!("{stdout}{stderr}");
    assert!(combined.contains("--dry-run"), "{combined}");
    assert!(
        !combined.contains("opens the launcher"),
        "named launch must start, not open a picker:\n{combined}"
    );
    assert!(
        combined.to_ascii_lowercase().contains("sign in"),
        "first run signs in if needed:\n{combined}"
    );
}

#[test]
fn config_help_describes_tui() {
    let (code, stdout, stderr) = run(&["config", "--help"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("--pick") || combined.contains("Interactive"),
        "{combined}"
    );
    assert!(
        combined.contains("credits") || combined.contains("account"),
        "{combined}"
    );
    assert!(
        combined.contains("models --pick") || combined.contains("json"),
        "{combined}"
    );
}

#[test]
fn config_no_args_non_tty_prints_status_not_picker() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-config-secret-value
    default_model: auto
",
    )
    .unwrap();
    let out = anyr()
        .args([
            "config",
            "--config",
            path.to_str().unwrap(),
            "--base-url",
            "http://127.0.0.1:9",
        ])
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("config");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(stdout.contains("default"), "{stdout}");
    assert!(stdout.contains("key"), "{stdout}");
    assert!(stdout.contains("account"), "{stdout}");
    assert!(
        !stdout.contains("sk-ar-v1-config-secret-value"),
        "full key leaked:\n{stdout}"
    );
    assert!(
        !stderr.contains("Pick 1-"),
        "non-TTY must not open picker:\n{stderr}"
    );
    assert!(
        stdout.contains("config get --json") || stdout.contains("models --pick"),
        "{stdout}"
    );
    let path_out = anyr()
        .args(["config", "path", "--config", path.to_str().unwrap()])
        .output()
        .expect("config path");
    let printed = String::from_utf8_lossy(&path_out.stdout);
    assert!(
        printed.contains(path.to_str().unwrap()),
        "config path should still print the file: {printed}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn keys_and_account_help_exit_zero() {
    for cmd in ["keys", "account", "logout", "menu"] {
        let (code, stdout, stderr) = run(&[cmd, "--help"]);
        assert_eq!(code, 0, "{cmd} --help failed: {stdout}{stderr}");
    }
}

#[test]
fn account_use_switches_active_profile() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-account-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-one-secret-value
    default_model: auto
  work:
    api_key: sk-ar-v1-two-secret-value
    default_model: anthropic/claude-sonnet-4.6
",
    )
    .unwrap();
    let out = anyr()
        .args(["account", "use", "work", "--config", path.to_str().unwrap()])
        .output()
        .expect("account use");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(stdout.contains("work"), "{stdout}");
    let who = anyr()
        .args([
            "whoami",
            "--config",
            path.to_str().unwrap(),
            "--base-url",
            "http://127.0.0.1:1",
        ])
        .output()
        .expect("whoami");
    let who_out = String::from_utf8_lossy(&who.stdout);
    assert!(who_out.contains("work"), "{who_out}");
    assert!(
        !who_out.contains("sk-ar-v1-two-secret-value"),
        "full key leaked:\n{who_out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn models_use_persists_default_without_network_when_unknown() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-models-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-one
    default_model: auto
",
    )
    .unwrap();
    // Unknown id is rejected locally after a models fetch; without a live
    // network this should fail loudly rather than write a guess.
    let out = anyr()
        .args([
            "models",
            "use",
            "definitely-not-a-model",
            "--config",
            path.to_str().unwrap(),
            "--base-url",
            "http://127.0.0.1:9",
        ])
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("models use");
    assert_ne!(out.status.code().unwrap_or(1), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn models_dump_tui_pins_anyrouter_auto_without_usage_dump() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-models-dump-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-models-dump-secret-value-abcdef
    default_model: auto
",
    )
    .unwrap();
    let out = anyr()
        .args(["models", "--dump-tui", "--config", path.to_str().unwrap()])
        .output()
        .expect("models dump");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(
        !stdout.contains('\u{1b}'),
        "dump must be ANSI-free: {stdout}"
    );
    assert!(stdout.contains("anyrouter/auto"), "{stdout}");
    assert!(
        !stdout.contains("most used"),
        "picker must not dump a most-used ranking:\n{stdout}"
    );
    let auto_at = stdout.find("anyrouter/auto").expect("preset");
    for ranked in ["stealth/ox-alpha", "openai/gpt", "claude-sonnet"] {
        if let Some(at) = stdout.find(ranked) {
            assert!(
                auto_at < at,
                "anyrouter/auto must lead, not a usage dump:\n{stdout}"
            );
        }
    }
    assert!(
        !stdout.contains("models-dump-secret-value"),
        "dump must not leak full secret: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn models_use_anyrouter_auto_persists_without_network() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-models-auto-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-one
    default_model: stealth/ox-alpha
",
    )
    .unwrap();
    let out = anyr()
        .args([
            "models",
            "use",
            "anyrouter/auto",
            "--config",
            path.to_str().unwrap(),
            "--base-url",
            "http://127.0.0.1:9",
        ])
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("models use auto");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(stdout.contains("anyrouter/auto"), "{stdout}");
    let cfg = std::fs::read_to_string(&path).expect("config");
    assert!(
        !cfg.contains("default_model: stealth/ox-alpha"),
        "preset should clear the pinned catalog id:\n{cfg}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn logout_clears_key_and_does_not_print_it() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-logout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-secret-value
",
    )
    .unwrap();
    let out = anyr()
        .args(["logout", "--config", path.to_str().unwrap()])
        .output()
        .expect("logout");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code().unwrap_or(1), 0, "{combined}");
    assert!(!combined.contains("sk-ar-v1-secret-value"), "{combined}");
    let src = std::fs::read_to_string(&path).unwrap();
    assert!(!src.contains("sk-ar-v1-secret-value"), "{src}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn install_flag_is_known_on_launch() {
    let (code, stdout, stderr) = run(&[
        "claude",
        "--install",
        "--dry-run",
        "--key",
        "sk-ar-v1-fixture-key-0001",
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("ANTHROPIC_BASE_URL"), "{stdout}");
}

#[test]
fn upgrade_does_not_print_full_sk_ar_key() {
    let key = "sk-ar-v1-secret-value";
    let (code, stdout, stderr) = {
        let out = anyr()
            .args(["upgrade", "--check", "--dry-run"])
            .env("ANYR_RELEASES_JSON", fixture_path())
            .env("ANYROUTER_API_KEY", key)
            .output()
            .expect("upgrade dry-run");
        (
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "stderr={stderr}");
    let combined = format!("{stdout}{stderr}");
    assert!(!combined.contains(key), "full key leaked:\n{combined}");
}

#[test]
fn update_check_keeps_config_channel_without_switch_flags() {
    let home = std::env::temp_dir().join(format!("anyr-keep-ch-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&home);
    std::fs::write(
        home.join("config.yaml"),
        "active_profile: default\nchannel: beta\nprofiles:\n  default:\n    api_key: x\n",
    )
    .expect("write config");
    let out = anyr()
        .args(["update", "--check"])
        .env("ANYR_RELEASES_JSON", fixture_path())
        .env("ANYROUTER_HOME", &home)
        .env("ANYR_CHANNEL", "stable")
        .output()
        .expect("update --check keep channel");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(stdout.contains("channel: beta"), "{stdout}");
    assert!(!stdout.contains("channel set to"), "{stdout}");
    let cfg = std::fs::read_to_string(home.join("config.yaml")).expect("config");
    assert!(cfg.contains("channel: beta"), "{cfg}");
    assert!(!cfg.contains("channel: stable"), "{cfg}");
}

#[test]
fn upgrade_check_reads_channel_from_config() {
    let home = std::env::temp_dir().join(format!("anyr-ch-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&home);
    std::fs::write(
        home.join("config.yaml"),
        "active_profile: default\nchannel: beta\nprofiles:\n  default:\n    api_key: x\n",
    )
    .expect("write config");
    let out = anyr()
        .args(["upgrade", "--check"])
        .env("ANYR_RELEASES_JSON", fixture_path())
        .env("ANYROUTER_HOME", &home)
        .env_remove("ANYR_CHANNEL")
        .output()
        .expect("upgrade --check config channel");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "stderr={stderr}");
    assert!(stdout.contains("channel: beta"), "{stdout}");
    assert!(
        stdout.contains("latest:") && stdout.contains("0.2.0-beta.1"),
        "{stdout}"
    );
}

#[test]
fn upgrade_auto_with_fixture_would_update() {
    let home = std::env::temp_dir().join(format!("anyr-auto-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&home);
    let out = anyr()
        .args(["upgrade", "--auto"])
        .env("ANYR_RELEASES_JSON", fixture_path())
        .env("ANYROUTER_HOME", &home)
        .env("ANYR_UPDATE_INTERVAL_SECS", "0")
        .env_remove("ANYR_CHANNEL")
        .output()
        .expect("upgrade --auto");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "stderr={stderr}");
    assert!(
        !format!("{stdout}{stderr}").contains("Unknown flag"),
        "{stdout}{stderr}"
    );
    // The detached checker prints nothing; it records what it saw.
    assert!(stdout.trim().is_empty(), "{stdout}");
    let state = std::fs::read_to_string(home.join("update-state.json")).unwrap_or_default();
    assert!(
        state.contains("\"latest_seen\": \"0.1.99\""),
        "auto-update should see the newer GitHub release, got:\n{state}"
    );
}

#[test]
fn upgrade_auto_is_quiet_when_up_to_date() {
    let home = std::env::temp_dir().join(format!("anyr-auto-current-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&home);
    let (_, ver, _) = run(&["--version"]);
    let ver = ver.trim();
    let fixture = home.join("releases.json");
    std::fs::write(
        &fixture,
        format!(
            r#"[{{"tag_name":"v{ver}","prerelease":false,"assets":[{{"name":"anyr-linux-x86_64","browser_download_url":"https://github.com/AnyRouterHQ/cli/releases/download/v{ver}/anyr-linux-x86_64"}}]}}]"#
        ),
    )
    .expect("write fixture");
    let out = anyr()
        .args(["upgrade", "--auto"])
        .env("ANYR_RELEASES_JSON", &fixture)
        .env("ANYROUTER_HOME", &home)
        .env("ANYR_UPDATE_INTERVAL_SECS", "0")
        .output()
        .expect("upgrade --auto current");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "stderr={stderr}");
    assert!(
        !stdout.contains("would update") && !stdout.contains("update available"),
        "up-to-date auto-update should be silent, got:\n{stdout}"
    );
}

#[test]
fn update_prints_from_to_channel_and_success() {
    let home = std::env::temp_dir().join(format!(
        "anyr-cli-update-ux-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&home).expect("home");
    let out = anyr()
        .args(["update"])
        .env("ANYROUTER_HOME", &home)
        .env("ANYR_RELEASES_JSON", fixture_path())
        .env("ANYR_CHANNEL", "beta")
        .output()
        .expect("update");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "stderr={stderr}");
    assert!(
        stdout.contains("Updating") && stdout.contains("->") && stdout.contains("beta channel"),
        "expected from→to + channel, got:\n{stdout}"
    );
    assert!(stdout.contains("0.2.0-beta.1"), "{stdout}");
    assert!(
        stdout.contains("✔") && stdout.contains("Would update to v0.2.0-beta.1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Run anyr to start using the new version."),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[cfg(unix)]
#[test]
fn update_spinner_animates_on_tty() {
    let home = std::env::temp_dir().join(format!(
        "anyr-cli-update-pty-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&home).expect("home");
    let helper = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".agents/skills/verify-anyr/helpers/pty-anyr.py");
    let transcript = home.join("tty.txt");
    let out = Command::new("python3")
        .args([
            helper.to_str().unwrap(),
            "--timeout",
            "6",
            "--out",
            transcript.to_str().unwrap(),
            "--",
            env!("CARGO_BIN_EXE_anyr"),
            "update",
        ])
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("ANYROUTER_HOME", &home)
        .env("ANYR_RELEASES_JSON", fixture_path())
        .env("ANYR_CHANNEL", "beta")
        .env("ANYR_SPINNER_MS", "20")
        .env("ANYR_SPINNER_MIN_TICKS", "6")
        .env_remove("NO_COLOR")
        .output()
        .expect("pty update");
    let captured = std::fs::read_to_string(&transcript)
        .unwrap_or_else(|_| String::from_utf8_lossy(&out.stdout).into_owned());
    let combined = format!(
        "{}{}{}",
        captured,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        out.status.code().unwrap_or(1),
        0,
        "pty update failed:\n{combined}"
    );
    assert!(
        combined.contains("Updating") && combined.contains("beta channel"),
        "{combined}"
    );
    assert!(combined.contains("->"), "{combined}");
    assert!(
        combined.contains("Would update to") || combined.contains("Updated to"),
        "{combined}"
    );
    assert!(
        combined.contains("Run anyr to start using the new version."),
        "{combined}"
    );
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let seen: Vec<_> = frames.iter().filter(|g| combined.contains(**g)).collect();
    assert!(
        seen.len() >= 2,
        "TTY spinner must tick distinct frames, got {seen:?} in:\n{combined:?}"
    );
    assert!(
        combined.contains('\r'),
        "animation rewrites the line with CR, got {combined:?}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn menu_dump_tui_prints_plain_frame() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-menu-dump-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-menu-dump-secret-value-abcdef
    default_model: auto
agents:
  claude:
    default_model: stealth/ox-alpha
  codex:
    default_model: auto
",
    )
    .unwrap();
    let out = anyr()
        .args(["menu", "--dump-tui", "--config", path.to_str().unwrap()])
        .env("ANYR_AGENTS", "claude,codex")
        .output()
        .expect("menu dump");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(
        !stdout.contains('\u{1b}'),
        "dump must be ANSI-free: {stdout}"
    );
    assert!(stdout.contains("anyr"), "{stdout}");
    assert!(stdout.contains("What do you want to do?"), "{stdout}");
    assert!(stdout.contains("Launch claude"), "{stdout}");
    assert!(stdout.contains("Launch codex"), "{stdout}");
    assert!(stdout.contains("Config"), "{stdout}");
    assert!(stdout.contains("Models"), "{stdout}");
    assert!(stdout.contains("Quit"), "{stdout}");
    assert!(stdout.contains("1."), "{stdout}");
    assert!(
        stdout.contains("no fullscreen"),
        "HUD footer missing:\n{stdout}"
    );
    assert!(
        !stdout.contains("type a number"),
        "dump must not ask to type a number:\n{stdout}"
    );
    assert!(
        !stdout.contains("menu-dump-secret-value"),
        "dump must not leak full secret: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dry_run_uses_per_agent_model_not_sibling_binding() {
    let dir = temp_home();
    std::fs::write(
        dir.join("config.yaml"),
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-fixture-key-0001
    default_model: auto
agents:
  claude:
    default_model: stealth/ox-alpha
  grok:
    default_model: auto
",
    )
    .unwrap();
    let claude = anyr()
        .args(["claude", "--dry-run", "--yes"])
        .env("ANYROUTER_HOME", &dir)
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("claude dry-run");
    let stdout = String::from_utf8_lossy(&claude.stdout);
    let stderr = String::from_utf8_lossy(&claude.stderr);
    assert_eq!(claude.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("ANTHROPIC_MODEL=stealth/ox-alpha"),
        "claude should use its own binding:\n{stdout}"
    );
    let cfg = std::fs::read_to_string(dir.join("config.yaml")).expect("config");
    assert!(cfg.contains("stealth/ox-alpha"), "{cfg}");
    assert!(
        cfg.contains("grok:") && cfg.contains("default_model: auto"),
        "grok binding must stay auto:\n{cfg}"
    );
}

#[test]
fn menu_dump_tui_empty_agents_still_offers_launch_claude() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-menu-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-empty-agents-secret-abcdef
    default_model: auto
",
    )
    .unwrap();
    let out = anyr()
        .args(["menu", "--dump-tui", "--config", path.to_str().unwrap()])
        .env("ANYR_AGENTS", "none")
        .output()
        .expect("menu dump empty");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("Launch claude"),
        "empty PATH must still offer Launch claude:\n{stdout}"
    );
    assert!(stdout.contains("Config"), "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn config_dump_tui_prints_plain_frame() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-config-dump-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-config-dump-secret-value-abcdef
    default_model: auto
",
    )
    .unwrap();
    let out = anyr()
        .args(["config", "--dump-tui", "--config", path.to_str().unwrap()])
        .output()
        .expect("config dump");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(
        !stdout.contains('\u{1b}'),
        "dump must be ANSI-free: {stdout}"
    );
    // WHY: plain text from the top-left, no card frame (owner style rule).
    assert!(
        !stdout.contains('╭') && !stdout.contains('│'),
        "no frame: {stdout}"
    );
    assert!(stdout.starts_with("Config"), "starts top-left: {stdout}");
    // Grouped sections with current values.
    for section in ["ACCOUNT", "MODEL", "AGENT", "GENERAL"] {
        assert!(stdout.contains(section), "missing {section} in:\n{stdout}");
    }
    let dumped: Vec<&str> = stdout.lines().collect();
    let acct = dumped
        .iter()
        .position(|l| l.contains("ACCOUNT"))
        .expect("ACCOUNT");
    let model = dumped
        .iter()
        .position(|l| l.contains("MODEL"))
        .expect("MODEL");
    assert!(
        model > acct + 1,
        "expected padding between ACCOUNT and MODEL:\n{stdout}"
    );
    for row in [
        "account",
        "api key",
        "default",
        "coding agent",
        "auto-update",
        "update channel",
        "[general]",
        "claude",
        "codex",
    ] {
        assert!(stdout.contains(row), "missing row \"{row}\" in:\n{stdout}");
    }
    assert!(
        !stdout.contains("config-dump-secret-value"),
        "dump must not leak full secret: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn menu_help_mentions_dump_tui() {
    let (code, stdout, stderr) = run(&["menu", "--help"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("dump-tui") || combined.contains("Ratatui"),
        "{combined}"
    );
}

#[test]
fn relay_help_documents_subcommands_and_flags() {
    let (code, stdout, stderr) = run(&["relay", "--help"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let combined = format!("{stdout}{stderr}");
    for word in [
        "start",
        "pair",
        "--target",
        "--pool",
        "--no-pool",
        "--model",
        "--all",
        "https://anyrouter.dev/pool",
        "--max-concurrency",
        "fm serve",
    ] {
        assert!(
            combined.contains(word),
            "relay help missing {word}:\n{combined}"
        );
    }
    assert!(
        !combined.contains("not yet implemented") && !combined.contains("coming later"),
        "relay is implemented; help must not call it a stub:\n{combined}"
    );
}

#[test]
fn relay_unknown_subcommand_prints_usage_and_fails() {
    let (code, _stdout, stderr) = run(&["relay", "wat"]);
    assert_eq!(code, 2, "bad subcommand is a usage error");
    assert!(stderr.contains("Unknown relay subcommand: wat"), "{stderr}");
    assert!(stderr.contains("Usage:"), "{stderr}");
}

#[test]
fn relay_rejects_unknown_flag_before_running() {
    let (code, _stdout, stderr) = run(&["relay", "start", "--bogus"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("Unknown flag --bogus"), "{stderr}");
}

/// Run `anyr relay …` against an empty ANYROUTER_HOME with no key, so
/// nothing reads or writes the developer's real config.
fn relay_in(home: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    let out = anyr()
        .arg("relay")
        .args(args)
        .env("ANYROUTER_HOME", home)
        .env_remove("ANYROUTER_API_KEY")
        .env_remove("ANYROUTER_RELAY_TOKEN")
        .env("NO_COLOR", "1")
        .output()
        .expect("spawn anyr relay");
    (
        out.status.code().unwrap_or(1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn bare_relay_shows_status_and_the_next_command_not_help() {
    let home = temp_home();
    let (code, stdout, stderr) = relay_in(&home, &[]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("not running"), "{stdout}");
    // No credential: the one thing to do next is log in.
    assert!(
        stdout.contains("Next:") && stdout.contains("login"),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn relay_status_json_is_machine_readable_without_probing() {
    let home = temp_home();
    let started = std::time::Instant::now();
    let (code, stdout, _) = relay_in(&home, &["status", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("status --json is JSON");
    assert_eq!(v["running"], false);
    assert_eq!(v["pool"]["enabled"], true);
    assert_eq!(v["pool"]["source"], "default");
    assert!(v["next"].as_str().unwrap().contains("login"));
    // WHY: status reads the state file; it must not wait on local probes.
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn relay_pool_off_is_saved_and_survives_for_the_next_start() {
    let home = temp_home();
    let (code, stdout, _) = relay_in(&home, &["pool", "off"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("Saved"), "{stdout}");
    let (_, stdout, _) = relay_in(&home, &["pool", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["enabled"], false);
    assert_eq!(v["source"], "saved");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn relay_stop_and_logs_explain_themselves_when_nothing_runs() {
    let home = temp_home();
    let (code, stdout, _) = relay_in(&home, &["stop"]);
    assert_eq!(code, 0, "stop is idempotent");
    assert!(stdout.contains("not running"), "{stdout}");
    let (code, _, stderr) = relay_in(&home, &["logs"]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("hint:") && stderr.contains("--detach"),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn relay_usage_errors_exit_2_with_the_fix() {
    let home = temp_home();
    for (args, needle) in [
        (&["unpair"][..], "--yes"),
        (&["models", "enable", "gpt-5"], "Unknown on-device model"),
        (&["pool", "maybe"], "on or off"),
        (&["start", "--model", "gpt-5"], "relay models"),
        (&["start", "--pool", "--no-pool"], "not both"),
    ] {
        let (code, _, stderr) = relay_in(&home, args);
        assert_eq!(code, 2, "{args:?}: {stderr}");
        assert!(stderr.contains(needle), "{args:?}: {stderr}");
    }
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn relay_models_toggle_is_saved() {
    let home = temp_home();
    let (code, stdout, _) = relay_in(&home, &["models", "disable", "apple/foundation-model"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("Disabled apple/foundation-model"),
        "{stdout}"
    );
    let prefs = std::fs::read_to_string(home.join("relay/prefs.json")).unwrap();
    assert!(!prefs.contains("apple/foundation-model"), "{prefs}");
    assert!(
        prefs.contains("perplexity/pplx-decider-flash-v1.2"),
        "{prefs}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn relay_start_without_target_or_server_reports_detection_fallback() {
    // No --target and nothing listening on the probed ports: start resolves a
    // token first — with none configured it must fail loudly BEFORE printing
    // anything misleading about targets.
    let dir = temp_home();
    let out = anyr()
        .args(["relay", "start"])
        .env("ANYROUTER_HOME", &dir)
        .output()
        .expect("spawn anyr");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    // unwrap_or(1): a signal death (None) must count as failure, not pass.
    assert_ne!(
        out.status.code().unwrap_or(1),
        0,
        "unauthenticated start must fail"
    );
    assert!(
        !stderr.is_empty(),
        "expected an error explaining what to do next"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn account_use_agent_binds_without_changing_active_profile() {
    let home = temp_home();
    let path = home.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-default-key-aaaa
    default_model: auto
  work:
    api_key: sk-ar-v1-work-key-bbbb
    default_model: anthropic/claude-sonnet-4.6
",
    )
    .unwrap();
    let out = anyr()
        .args(["account", "use", "work", "--agent", "claude"])
        .env("ANYROUTER_HOME", &home)
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("account use --agent");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(stdout.contains("claude"), "{stdout}");
    assert!(stdout.contains("work"), "{stdout}");
    let cfg = std::fs::read_to_string(&path).unwrap();
    assert!(
        cfg.contains("active_profile: default"),
        "session default must stay default:\n{cfg}"
    );
    assert!(cfg.contains("agents:"), "{cfg}");
    assert!(cfg.contains("  claude:"), "{cfg}");
    assert!(cfg.contains("    profile: work"), "{cfg}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn models_use_agent_pins_without_inventing_ids() {
    let home = temp_home();
    let path = home.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
last_tool: claude
profiles:
  default:
    api_key: sk-ar-v1-default-key-aaaa
    default_model: auto
",
    )
    .unwrap();
    let out = anyr()
        .args(["models", "use", "stealth/ox-alpha", "--agent", "claude"])
        .env("ANYROUTER_HOME", &home)
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("models use --agent");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    let cfg = std::fs::read_to_string(&path).unwrap();
    assert!(cfg.contains("    default_model: stealth/ox-alpha"), "{cfg}");
    assert!(
        !cfg.contains("stealth/ox-alpha[1m]"),
        "catalog id only:\n{cfg}"
    );
    assert!(
        cfg.contains("agents:") && cfg.contains("  claude:"),
        "{cfg}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn launch_uses_per_agent_key_and_model_not_default() {
    let home = temp_home();
    let path = home.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-default-key-aaaa
    default_model: auto
  work:
    api_key: sk-ar-v1-work-key-bbbb
    default_model: anthropic/claude-sonnet-4.6
agents:
  claude:
    api_key: sk-ar-v1-claude-key-cccc
    default_model: stealth/ox-alpha
  grok:
    profile: work
    api_key: sk-ar-v1-grok-key-dddd
    default_model: grok-4
",
    )
    .unwrap();
    let claude = anyr()
        .args(["claude", "--dry-run", "--yes"])
        .env("ANYROUTER_HOME", &home)
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("claude dry-run");
    let grok = anyr()
        .args(["grok", "--dry-run", "--yes"])
        .env("ANYROUTER_HOME", &home)
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("grok dry-run");
    let claude_out = String::from_utf8_lossy(&claude.stdout);
    let grok_out = String::from_utf8_lossy(&grok.stdout);
    assert_eq!(
        claude.status.code().unwrap_or(1),
        0,
        "{claude_out}{}",
        String::from_utf8_lossy(&claude.stderr)
    );
    assert_eq!(
        grok.status.code().unwrap_or(1),
        0,
        "{grok_out}{}",
        String::from_utf8_lossy(&grok.stderr)
    );
    assert!(
        claude_out.contains("ANTHROPIC_MODEL=stealth/ox-alpha"),
        "{claude_out}"
    );
    assert!(
        !claude_out.contains("sk-ar-v1-claude-key-cccc"),
        "full claude key leaked:\n{claude_out}"
    );
    assert!(
        !claude_out.contains("sk-ar-v1-default-key-aaaa"),
        "must not fall back to default key:\n{claude_out}"
    );
    assert!(
        grok_out.contains("GROK_CODE_XAI_API_KEY") || grok_out.contains("GROK_"),
        "{grok_out}"
    );
    assert!(
        !grok_out.contains("sk-ar-v1-grok-key-dddd"),
        "full grok key leaked:\n{grok_out}"
    );
    assert!(
        !grok_out.contains("sk-ar-v1-default-key-aaaa"),
        "grok must not fall back to default key:\n{grok_out}"
    );
    assert!(
        claude_out.contains("...cccc"),
        "claude dry-run missing bound key suffix:\n{claude_out}"
    );
    assert!(
        grok_out.contains("...dddd"),
        "grok dry-run missing bound key suffix:\n{grok_out}"
    );
    assert!(
        !claude_out.contains("...aaaa"),
        "claude used the default profile key:\n{claude_out}"
    );
    assert!(
        !grok_out.contains("...aaaa"),
        "grok used the default profile key:\n{grok_out}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn menu_dump_shows_per_agent_bindings_inline() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-agent-dump-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
last_tool: claude
profiles:
  default:
    api_key: sk-ar-v1-menu-agent-secret-abcdef
    default_model: auto
agents:
  claude:
    default_model: stealth/ox-alpha
  grok:
    default_model: grok-4
",
    )
    .unwrap();
    let out = anyr()
        .args(["menu", "--dump-tui", "--config", path.to_str().unwrap()])
        .env("ANYR_AGENTS", "claude,grok")
        .output()
        .expect("menu dump per-agent");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(stdout.contains("Launch claude"), "{stdout}");
    assert!(stdout.contains("Launch grok"), "{stdout}");
    assert!(stdout.contains("What do you want to do?"), "{stdout}");
    assert!(
        !stdout.contains("menu-agent-secret"),
        "dump must not leak full secret: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn claude_model_1m_flag_still_launches() {
    let (code, stdout, stderr) = run(&[
        "claude",
        "--dry-run",
        "--yes",
        "--key",
        "sk-ar-v1-fixture-key-0001",
        "--model",
        "stealth/ox-alpha[1m]",
        "--yolo",
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("ANTHROPIC_MODEL=stealth/ox-alpha"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("ANTHROPIC_MODEL=stealth/ox-alpha[1m]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("--dangerously-skip-permissions"),
        "{stdout}"
    );
}

#[test]
fn menu_dump_shows_routing_toggles_under_configure() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-routing-dump-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
last_tool: claude
profiles:
  default:
    api_key: sk-ar-v1-routing-secret-abcdef
    default_model: auto
agents:
  claude:
    default_model: anyrouter/auto
    provider:
      sort: exacto
    require_params: tools
    min_context: 1000000
  grok:
    default_model: anyrouter/free
",
    )
    .unwrap();
    let out = anyr()
        .args(["menu", "--dump-tui", "--config", path.to_str().unwrap()])
        .env("ANYR_AGENTS", "claude,grok")
        .output()
        .expect("menu dump routing");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(stdout.contains("Launch claude"), "{stdout}");
    assert!(stdout.contains("Launch grok"), "{stdout}");
    assert!(stdout.contains("What do you want to do?"), "{stdout}");
    assert!(
        !stdout.to_ascii_lowercase().contains("most used"),
        "picker/catalog must not dump most-used:\n{stdout}"
    );
    assert!(
        !stdout.contains("routing-secret"),
        "dump must not leak full secret: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn claude_dry_run_sends_routing_extra_body_for_auto_and_free() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-routing-launch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-fixture-key-0001
    default_model: auto
agents:
  claude:
    default_model: anyrouter/auto
    provider:
      sort: exacto
    require_params: tools
    min_context: 1000000
",
    )
    .unwrap();
    for model in ["anyrouter/auto", "anyrouter/free"] {
        let out = anyr()
            .args([
                "claude",
                "--dry-run",
                "--yes",
                "--config",
                path.to_str().unwrap(),
                "--model",
                model,
            ])
            .output()
            .expect("claude dry-run routing");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
        assert!(stdout.contains("command:"), "{stdout}");
        let expected = if model == "anyrouter/auto" {
            "ANTHROPIC_MODEL=anyrouter/auto[1m]"
        } else {
            "ANTHROPIC_MODEL=anyrouter/free[1m]"
        };
        assert!(
            stdout.contains(expected),
            "yaml min_context 1M must re-apply the HUD suffix:\n{stdout}"
        );
        assert!(
            stdout.contains("CLAUDE_CODE_EXTRA_BODY="),
            "must send extra body:\n{stdout}"
        );
        assert!(stdout.contains("\"sort\":\"exacto\""), "{stdout}");
        assert!(
            stdout.contains("\"require_params\":[\"tools\"]"),
            "{stdout}"
        );
        assert!(stdout.contains("\"min_context\":1000000"), "{stdout}");
        assert!(!stdout.contains("sk-ar-v1-fixture-key-0001"), "{stdout}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn claude_dry_run_peels_auto_1m_into_extra_body_not_laguna() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-auto-1m-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-fixture-key-0001
    default_model: auto
",
    )
    .unwrap();
    let out = anyr()
        .args([
            "claude",
            "--dry-run",
            "--yes",
            "--config",
            path.to_str().unwrap(),
            "--model",
            "anyrouter/auto[1m]",
        ])
        .output()
        .expect("claude dry-run auto[1m]");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("ANTHROPIC_MODEL=anyrouter/auto[1m]"),
        "must keep virtual auto[1m] for the Claude HUD, not a concrete SKU:\n{stdout}"
    );
    assert!(!stdout.to_ascii_lowercase().contains("laguna"), "{stdout}");
    assert!(stdout.contains("CLAUDE_CODE_EXTRA_BODY="), "{stdout}");
    assert!(stdout.contains("\"min_context\":1000000"), "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn claude_dry_run_peels_virtual_preset_1m_into_extra_body() {
    let dir = std::env::temp_dir().join(format!("anyr-cli-virtual-1m-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-fixture-key-0001
    default_model: auto
",
    )
    .unwrap();
    for (flag, expected) in [
        ("anyrouter/free[1m]", "anyrouter/free[1m]"),
        ("anyrouter/byok[1m]", "anyrouter/byok[1m]"),
        ("anyrouter/hermes[500k]", "anyrouter/hermes[500k]"),
    ] {
        let out = anyr()
            .args([
                "claude",
                "--dry-run",
                "--yes",
                "--config",
                path.to_str().unwrap(),
                "--model",
                flag,
            ])
            .output()
            .expect("claude dry-run virtual[1m]");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code().unwrap_or(1),
            0,
            "{flag}\n{stdout}{stderr}"
        );
        assert!(
            stdout.contains(&format!("ANTHROPIC_MODEL={expected}")),
            "{flag} must keep virtual id:\n{stdout}"
        );
        assert!(
            stdout.contains("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=0"),
            "{flag}\n{stdout}"
        );
        assert!(
            stdout.contains("CLAUDE_CODE_EXTRA_BODY="),
            "{flag}\n{stdout}"
        );
        if flag.contains("[1m]") {
            assert!(
                stdout.contains("\"min_context\":1000000"),
                "{flag}\n{stdout}"
            );
        } else {
            assert!(
                stdout.contains("\"min_context\":500000"),
                "{flag}\n{stdout}"
            );
        }
        assert!(
            !stdout.to_ascii_lowercase().contains("laguna"),
            "{flag}\n{stdout}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unsigned_hud_dump_offers_launch_claude() {
    // WHY: right after install there is no key yet. Enter on the HUD
    // should still be "Launch claude" (login happens inside launch).
    let dir = temp_home();
    let path = dir.join("config.yaml");
    let out = anyr()
        .args(["menu", "--dump-tui", "--config", path.to_str().unwrap()])
        .env("ANYROUTER_HOME", &dir)
        .env("ANYR_AGENTS", "none")
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("unsigned hud");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(stdout.contains("Launch claude"), "{stdout}");
    assert!(stdout.contains("What do you want to do?"), "{stdout}");
    assert!(stdout.contains("Config"), "{stdout}");
    assert!(stdout.contains("Quit"), "{stdout}");
    assert!(!stdout.contains("Login"), "{stdout}");
    assert!(
        !stdout.contains("Install an agent"),
        "unsigned first-run must not hide Launch behind install:\n{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn models_dump_tui_pins_anyrouter_auto_without_most_used() {
    let (code, stdout, stderr) = run(&["models", "--dump-tui"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("anyrouter/auto"), "{stdout}");
    assert!(
        !stdout.to_ascii_lowercase().contains("most used"),
        "{stdout}"
    );
}

// --- PR #52 bug-fix coverage ---
#[test]
fn claude_yolo_extra_in_config_expands_like_flag() {
    // WHY: tools.claude.yolo must survive serialize and launch like --yolo.
    let dir = temp_home();
    std::fs::write(
        dir.join("config.yaml"),
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-fixture-key-0001
    default_model: auto
tools:
  claude:
    yolo: true
",
    )
    .unwrap();
    let out = anyr()
        .args(["claude", "--dry-run", "--yes"])
        .env("ANYROUTER_HOME", &dir)
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("yolo extra dry-run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("--dangerously-skip-permissions"),
        "config yolo must expand:\n{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn config_dump_tui_claude_tab_shows_agent_rows() {
    let dir = temp_home();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-tab-dump-secret-abcdef
    default_model: auto
",
    )
    .unwrap();
    let out = anyr()
        .args(["config", "--dump-tui", "--config", path.to_str().unwrap()])
        .env("ANYR_TUI_TAB", "claude")
        .env("ANYROUTER_HOME", &dir)
        .output()
        .expect("config claude tab");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(stdout.contains("[claude]"), "{stdout}");
    for row in ["command", "install", "gateway discovery", "haiku"] {
        assert!(stdout.contains(row), "missing {row} in:\n{stdout}");
    }
    assert!(
        !stdout.contains("tab-dump-secret"),
        "dump must not leak secret: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn config_dump_tui_grok_tab_omits_claude_aliases() {
    let dir = temp_home();
    let path = dir.join("config.yaml");
    std::fs::write(
        &path,
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-grok-tab-secret-abcdef
    default_model: auto
",
    )
    .unwrap();
    let out = anyr()
        .args(["config", "--dump-tui", "--config", path.to_str().unwrap()])
        .env("ANYR_TUI_TAB", "grok")
        .env("ANYROUTER_HOME", &dir)
        .output()
        .expect("config grok tab");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    assert!(stdout.contains("[grok]"), "{stdout}");
    assert!(
        stdout.contains("GROK_MODELS_BASE_URL") || stdout.contains("base URL env"),
        "{stdout}"
    );
    assert!(!stdout.contains("haiku"), "{stdout}");
    assert!(!stdout.contains("gateway discovery"), "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cursor_is_honest_stub_not_silent_success() {
    let (code, stdout, stderr) = run(&["cursor"]);
    assert_ne!(code, 0, "{stdout}{stderr}");
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("not yet") || combined.contains("not a launch"),
        "{combined}"
    );
}

#[test]
fn launch_forwards_unknown_flag_to_the_agent() {
    // WHY: contract change. A flag anyr does not own on a launcher used to be
    // a usage error (exit 2); it now goes to the agent, which reports it.
    let out = anyr()
        .args([
            "claude",
            "--dry-run",
            "--yes",
            "--key",
            "sk-ar-v1-fixture-key-0001",
            "--bogus",
        ])
        .env("ANYROUTER_HOME", temp_home())
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("dry-run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("\"--bogus\"]"), "{stdout}");
}
#[test]
fn whoami_and_keys_fail_loud_without_config() {
    let dir = temp_home();
    // `whoami` is a status question: "not signed in" is its answer, on stdout,
    // still with the auth exit code.
    let out = anyr()
        .arg("whoami")
        .env("ANYROUTER_HOME", &dir)
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("missing key");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(4), "{stdout}");
    assert!(stdout.contains("Not signed in"), "{stdout}");
    let out = anyr()
        .args(["keys", "list"])
        .env("ANYROUTER_HOME", &dir)
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("missing key");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code().unwrap_or(1), 0, "{stderr}");
    assert!(
        stderr.contains("not signed in") && stderr.contains("hint: run `anyr login`"),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn yes_equals_true_skips_confirm_parse() {
    let (code, stdout, stderr) = run(&["claude", "--help"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!stdout.contains("Skip the launcher"), "{stdout}");
    assert!(!stdout.contains("--no-check"), "{stdout}");
    assert!(
        stdout.contains("--yes") && stdout.contains("--ok"),
        "{stdout}"
    );
}

#[test]
fn menu_non_tty_prints_status_before_actions() {
    // WHY: a piped `anyr menu` can't be driven, but the status line is the
    // preflight payoff — account · model · agent · credits. It must lead so
    // scripts/CI see state, not just a bare action list.
    let dir = temp_home();
    std::fs::write(
        dir.join("config.yaml"),
        "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-menu-status-secret-abcdef
    default_model: auto
",
    )
    .unwrap();
    let out = anyr()
        .arg("menu")
        .env("ANYROUTER_HOME", &dir)
        .output()
        .expect("menu non-tty");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code().unwrap_or(1), 0, "{stdout}{stderr}");
    // First line is the status, not an action: it names the binary and the
    // signed-in dot, and never leaks the full secret.
    let first = stdout.lines().next().unwrap_or("");
    assert!(first.contains("anyr"), "status must name anyr: {first}");
    assert!(
        first.contains("auto") || first.contains("claude"),
        "status must show model/agent: {first}"
    );
    assert!(
        !stdout.contains("menu-status-secret"),
        "leaked secret: {stdout}"
    );
    // Actions follow the status line.
    assert!(
        stdout.contains("Config") && stdout.contains("Quit"),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Releases where stable is older than the running build and the beta
/// channel holds the running build plus an older beta. Version-independent.
fn ahead_of_stable_fixture(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let (_, ver, _) = run(&["--version"]);
    let ver = ver.split_whitespace().next().unwrap_or("").to_string();
    let home = std::env::temp_dir().join(format!("anyr-ahead-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("home");
    let asset = r#""assets":[{"name":"anyr-linux-x86_64","browser_download_url":"https://github.com/AnyRouterHQ/cli/releases/download/vX/anyr-linux-x86_64"},{"name":"anyr-linux-arm64","browser_download_url":"https://github.com/AnyRouterHQ/cli/releases/download/vX/anyr-linux-arm64"},{"name":"anyr-darwin-arm64","browser_download_url":"https://github.com/AnyRouterHQ/cli/releases/download/vX/anyr-darwin-arm64"},{"name":"anyr-darwin-x86_64","browser_download_url":"https://github.com/AnyRouterHQ/cli/releases/download/vX/anyr-darwin-x86_64"},{"name":"anyr-windows-x86_64.exe","browser_download_url":"https://github.com/AnyRouterHQ/cli/releases/download/vX/anyr-windows-x86_64.exe"}]"#;
    let fixture = home.join("releases.json");
    std::fs::write(
        &fixture,
        format!(
            r#"[{{"tag_name":"v0.0.1","prerelease":false,{asset}}},{{"tag_name":"v{ver}","prerelease":true,{asset}}},{{"tag_name":"v0.0.2-beta.1","prerelease":true,{asset}}}]"#
        ),
    )
    .expect("fixture");
    (home, fixture)
}

fn update_out(args: &[&str], home: &std::path::Path, fixture: &std::path::Path) -> String {
    let out = anyr()
        .args(args)
        .env("ANYROUTER_HOME", home)
        .env("ANYR_RELEASES_JSON", fixture)
        .env_remove("ANYR_CHANNEL")
        .output()
        .expect("update");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{args:?}\n{stdout}{stderr}");
    stdout
}

#[test]
fn switch_to_older_stable_is_ahead_not_update_and_agrees_with_channel_flag() {
    // WHY: `update --stable --check` reported "update available" for an older
    // stable while `upgrade --channel stable --check` said "up to date".
    // Both must agree, and neither may offer a silent downgrade.
    let (home, fixture) = ahead_of_stable_fixture("check");
    let switch = update_out(&["update", "--stable", "--check"], &home, &fixture);
    let channel = update_out(
        &["upgrade", "--channel", "stable", "--check"],
        &home,
        &fixture,
    );
    for out in [&switch, &channel] {
        assert!(!out.contains("update available"), "{out}");
        assert!(out.contains("ahead") && out.contains("0.0.1"), "{out}");
        assert!(out.contains("--stable --force"), "{out}");
    }
    let dry = update_out(&["update", "--stable", "--dry-run"], &home, &fixture);
    assert!(!dry.contains("Would update"), "downgraded silently:\n{dry}");
    assert!(dry.contains("Keeping"), "{dry}");
    let forced = update_out(
        &["update", "--stable", "--force", "--dry-run"],
        &home,
        &fixture,
    );
    assert!(forced.contains("Would update to v0.0.1"), "{forced}");
}

#[test]
fn beta_dry_run_on_latest_beta_never_picks_older_build() {
    // WHY: the fallback list skipped only the exact installed version, so
    // `update --beta --dry-run` offered an older beta while --check said current.
    let (home, fixture) = ahead_of_stable_fixture("beta");
    let dry = update_out(&["update", "--beta", "--dry-run"], &home, &fixture);
    assert!(!dry.contains("0.0.2-beta.1"), "{dry}");
    assert!(dry.contains("Already up to date"), "{dry}");
    let check = update_out(&["update", "--beta", "--check"], &home, &fixture);
    assert!(check.contains("up to date"), "{check}");
}

#[test]
fn dry_run_has_no_bare_env_header() {
    // WHY: dry-run printed a stray `env:` line with nothing under it.
    let (home, fixture) = ahead_of_stable_fixture("env");
    let out = anyr()
        .args(["update", "--beta", "--check", "--dry-run"])
        .env("ANYROUTER_HOME", &home)
        .env("ANYR_RELEASES_JSON", &fixture)
        .output()
        .expect("dry-run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.lines().any(|l| l.trim() == "env:"),
        "stray env header:\n{stdout}"
    );
}

#[test]
fn beta_stable_flags_persist_channel_but_channel_flag_does_not() {
    // WHY: --beta/--stable switch the track auto-update follows; --channel
    // is a one-off peek and must not rewrite config.
    let (home, fixture) = ahead_of_stable_fixture("persist");
    update_out(&["update", "--beta", "--check"], &home, &fixture);
    let cfg = std::fs::read_to_string(home.join("config.yaml")).expect("config");
    assert!(cfg.contains("channel: beta"), "{cfg}");
    update_out(
        &["update", "--channel", "stable", "--check"],
        &home,
        &fixture,
    );
    let cfg = std::fs::read_to_string(home.join("config.yaml")).expect("config");
    assert!(
        cfg.contains("channel: beta"),
        "--channel rewrote config:\n{cfg}"
    );
}

#[test]
fn auto_update_never_downgrades_to_older_stable() {
    // WHY: the background updater must not replace a newer beta build with
    // an older stable after a channel switch.
    let (home, fixture) = ahead_of_stable_fixture("auto");
    let out = anyr()
        .args(["upgrade", "--auto"])
        .env("ANYROUTER_HOME", &home)
        .env("ANYR_RELEASES_JSON", &fixture)
        .env("ANYR_UPDATE_INTERVAL_SECS", "0")
        .env("ANYR_CHANNEL", "stable")
        .output()
        .expect("auto");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    assert!(!stdout.contains("would update"), "{stdout}");
}

// WHY: scripts branch on exit code. Exit 2 means "you called it wrong" and
// must be the same for every kind of usage mistake; exit 1 is for runtime
// failures only.
#[test]
fn usage_errors_all_exit_2() {
    for args in [
        &["nonsense"][..],
        &["auth", "bogus"],
        &["completion", "bogus"],
        &["api", "bogus"],
        &["model", "bogus"],
        &["usage", "--zzz"],
        &["help", "nonsense"],
        &["help", "auth", "bogus"],
    ] {
        let (code, _, stderr) = run(args);
        assert_eq!(code, 2, "{args:?} stderr={stderr}");
    }
}

// WHY: `help <typo>` used to print root help and exit 0, hiding the typo.
#[test]
fn help_unknown_topic_errors_with_suggestion() {
    let (code, stdout, stderr) = run(&["help", "clade"]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty(), "no help on stdout: {stdout}");
    assert!(stderr.contains("unknown help topic \"clade\""), "{stderr}");
    assert!(
        stderr.contains("did you mean `anyr help claude`"),
        "{stderr}"
    );
}

#[test]
fn help_known_topics_still_exit_0() {
    for args in [
        &["help"][..],
        &["help", "commands"],
        &["help", "auth", "login"],
        &["help", "claude"],
    ] {
        let (code, _, stderr) = run(args);
        assert_eq!(code, 0, "{args:?} stderr={stderr}");
    }
}

#[test]
fn help_topic_prints_that_topics_help_not_root_help() {
    // `help <topic>` used to fall through to root help, so users asking for
    // `help claude` never saw the launch flags.
    let (_, root, _) = run(&["help"]);
    for (args, needle) in [
        (&["help", "claude"][..], "--yolo"),
        (&["help", "login"], "--device"),
        (&["help", "auth", "login"], "--key -"),
        (&["help", "auth", "token"], "--masked"),
        (&["help", "keys"], "keys revoke"),
    ] {
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, 0, "{args:?}: {stderr}");
        assert!(
            stdout.contains(needle),
            "{args:?} lacks {needle}:\n{stdout}"
        );
        assert_ne!(stdout, root, "{args:?} printed root help");
    }
    let (_, auth, _) = run(&["help", "auth"]);
    assert_ne!(auth, root, "help auth printed root help");
}

#[test]
fn missing_subcommand_argument_is_a_usage_error_exit_2() {
    // Scripts tell "you called it wrong" (2) from "it ran and failed" (1).
    let dir = temp_home();
    let stub_path = exit_zero_stub();
    let seed = anyr()
        .args(["claude", "--yes", "--key", "sk-ar-v1-fixture-key-0002"])
        .args(["--model", "z-ai/glm-4.7-flash"])
        .env("ANYROUTER_HOME", &dir)
        .env("ANYROUTER_CLAUDE_PATH", stub_path.to_str().unwrap())
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("seed config");
    assert!(
        dir.join("config.yaml").exists(),
        "seed failed: {}",
        String::from_utf8_lossy(&seed.stderr)
    );
    for args in [
        &["account", "use"][..],
        &["account", "remove"],
        &["account", "rename", "a"],
        &["auth", "switch"],
        &["keys", "revoke"],
        &["relay", "bogus"],
    ] {
        let out = anyr()
            .args(args)
            .env("ANYROUTER_HOME", &dir)
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .expect("spawn anyr");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains("Usage:"), "{args:?}: {stderr}");
        assert!(!stderr.contains('\u{0}'), "marker leaked: {stderr:?}");
    }
}

#[test]
fn dash_key_with_empty_stdin_fails_on_non_login_commands() {
    // `--key -` must mean "read stdin", never the literal key "-", including
    // under `auth <sub>`.
    for args in [
        &["usage", "--key", "-"][..],
        &["auth", "status", "--key", "-"][..],
    ] {
        let home = std::env::temp_dir().join(format!("anyr-key-{}", std::process::id()));
        let out = anyr()
            .args(args)
            .env("ANYROUTER_HOME", &home)
            .env("ANYR_AUTO_UPDATE", "0")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("spawn anyr");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains("No key on stdin"), "{args:?}: {stderr}");
    }
}

#[test]
fn launch_help_lists_claude_only_flags_for_claude_alone() {
    // WHY: `codex --help` used to advertise --haiku/--hub, which only Claude
    // Code honours, so users tried flags that silently did nothing. --yolo is
    // portable, so each help names what it maps to.
    let (_, claude, _) = run(&["claude", "--help"]);
    for flag in ["--haiku", "--hub", "passes --dangerously-skip-permissions"] {
        assert!(claude.contains(flag), "claude help lacks {flag}:\n{claude}");
    }
    for (agent, yolo) in [
        ("codex", "passes --dangerously-bypass-approvals-and-sandbox"),
        ("grok", "passes --always-approve"),
        ("opencode", "passes --auto"),
        ("pi", "No effect: Pi has no flag to skip permission prompts"),
        ("pool", "passes --mode always-allow (from docs, unverified)"),
        ("qwen", "passes --approval-mode yolo"),
        ("goose", "sets GOOSE_MODE=auto"),
        ("aider", "passes --yes-always"),
        ("crush", "passes -y"),
        ("kilo", "passes --auto"),
        ("fx", "sets FX_PERMISSION_MODE=full-access"),
    ] {
        let (code, out, _) = run(&[agent, "--help"]);
        assert_eq!(code, 0);
        assert!(out.contains("--model"), "{agent} help lost --model:\n{out}");
        assert!(out.contains(yolo), "{agent} help lacks `{yolo}`:\n{out}");
        assert!(
            out.contains("Any other flag goes to the agent"),
            "{agent}:\n{out}"
        );
        for flag in ["--haiku", "--sonnet", "--hub"] {
            assert!(!out.contains(flag), "{agent} help shows {flag}:\n{out}");
        }
    }
}

#[test]
fn usage_403_points_at_auth_login() {
    // WHY: a restricted key got a bare "HTTP 403" with no way forward, while
    // `keys` and `api` already name the fix.
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = s.write_all(
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            );
        }
    });
    let base = format!("http://127.0.0.1:{port}");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_anyr"))
        .args([
            "usage",
            "--key",
            "sk-ar-v1-testkey0000000000",
            "--base-url",
            &base,
        ])
        .env("ANYROUTER_HOME", temp_home())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(4), "{err}");
    assert!(
        err.contains("lacks scope `credits`") && err.contains("login"),
        "{err}"
    );
}

#[test]
fn usage_errors_print_clean_message_without_marker() {
    // WHY: the usage marker was doubled for `models <bad>` and leaked into
    // stderr; bare `relay` dumped usage as an error instead of help.
    let (code, _, err) = run(&["models", "nonexistent"]);
    assert_eq!(code, 2);
    assert!(err.starts_with("Unknown models command"), "{err:?}");
    let (code, _, err) = run(&["relay", "bogus"]);
    assert_eq!(code, 2);
    assert!(
        err.starts_with("Unknown relay subcommand: bogus"),
        "{err:?}"
    );
    let (code, out, _) = run(&["relay"]);
    assert_eq!(code, 0);
    assert!(out.contains("anyr relay start"), "{out}");
}

/// One-shot `/v1/me` server: returns (base_url, join handle).
fn me_server_with_status(
    status: &'static str,
    body: &'static str,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (base, handle)
}

fn status_config(home: &std::path::Path) -> std::path::PathBuf {
    let path = home.join("config.yaml");
    std::fs::write(
        &path,
        "active_profile: default\nprofiles:\n  default:\n    api_key: sk-ar-v1-abcdefghijklmnopqrstuvwxyz\n    default_model: poolside/laguna-s-2.1\n    claude_haiku: anyrouter/free\n    default_tool: claude\n",
    )
    .unwrap();
    path
}

#[test]
fn auth_status_shows_user_and_only_the_default_model() {
    let home = temp_home();
    let cfg = status_config(&home);
    // Status asks /v1/me and /v1/key in parallel, so answer by path.
    let base = common::serve_routes(vec![(
        "/api/v1/me",
        200,
        r#"{"name":"Duyet Le","email":"me@duyet.net"}"#.into(),
    )]);
    let out = anyr()
        .args([
            "auth",
            "status",
            "--config",
            cfg.to_str().unwrap(),
            "--base-url",
            &base,
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.starts_with("account   Duyet Le · me@duyet.net"),
        "{stdout:?}"
    );
    assert!(stdout.contains("poolside/laguna-s-2.1"), "{stdout}");
    assert!(
        !stdout.contains("claude_") && !stdout.contains("default_tool"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("abcdefghijklmnopqrstuvwxyz"),
        "full key leaked: {stdout}"
    );
}

#[test]
fn auth_status_json_has_user_and_masked_key() {
    let home = temp_home();
    let cfg = status_config(&home);
    // Status asks /v1/me and /v1/key in parallel, so answer by path.
    let base = common::serve_routes(vec![(
        "/api/v1/me",
        200,
        r#"{"name":"Duyet Le","email":"me@duyet.net"}"#.into(),
    )]);
    let out = anyr()
        .args([
            "status",
            "--json",
            "--config",
            cfg.to_str().unwrap(),
            "--base-url",
            &base,
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["account"], "default");
    assert_eq!(v["user"]["name"], "Duyet Le");
    assert_eq!(v["user"]["email"], "me@duyet.net");
    assert!(v["key_masked"].as_str().unwrap().contains('…'), "{v}");
    assert_eq!(v["default_model"], "poolside/laguna-s-2.1");
    assert!(v["config_path"].is_string());
    assert!(
        v.get("claude_haiku").is_none() && v.get("default_tool").is_none(),
        "{v}"
    );
}

#[test]
fn whoami_keeps_the_exit_tag_out_of_identity_error_on_401() {
    // WHY: scope_error tags 401/403 with an internal exit marker; it must not
    // reach `identity_error` in JSON or the human text line.
    let home = temp_home();
    let cfg = status_config(&home);
    for json in [true, false] {
        let (base, server) = me_server_with_status("401 Unauthorized", r#"{"error":"bad key"}"#);
        let mut args = vec!["whoami"];
        if json {
            args.push("--json");
        }
        args.extend(["--config", cfg.to_str().unwrap(), "--base-url", &base]);
        let out = anyr().args(args).stdin(Stdio::null()).output().unwrap();
        server.join().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stdout.contains('\u{1}'), "{stdout:?}");
        assert!(!stderr.contains('\u{1}'), "{stderr:?}");
        if json {
            let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
            let msg = v["identity_error"].as_str().unwrap();
            assert!(!msg.is_empty() && !msg.contains('\u{1}'), "{v}");
        }
    }
}

#[test]
fn auth_status_json_reports_identity_error_when_the_server_never_answers() {
    let home = temp_home();
    let cfg = status_config(&home);
    // WHY: an offline or hung gateway must not stall `status`; it answers
    // with identity_error and still exits 0 because a key is configured.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let started = std::time::Instant::now();
    let out = anyr()
        .args([
            "status",
            "--json",
            "--config",
            cfg.to_str().unwrap(),
            "--base-url",
            &base,
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "status waited {:?}",
        started.elapsed()
    );
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["identity_error"].as_str().is_some_and(|e| !e.is_empty()),
        "{v}"
    );
    assert_eq!(v["default_model"], "poolside/laguna-s-2.1");
    drop(listener);
}
