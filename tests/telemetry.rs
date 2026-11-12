//! The anonymous install ping, end to end through the built binary against a
//! local capture server. Nothing here talks to PostHog.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

fn capture_server() -> (String, Receiver<serde_json::Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/i/v0/e/", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let mut len = 0usize;
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            );
            if let Ok(v) = serde_json::from_slice(&body) {
                let _ = tx.send(v);
            }
        }
    });
    (url, rx)
}

/// `anyr help` with telemetry allowed (no CI, no ANYR_NO_UPDATE) unless the
/// caller adds an opt-out.
fn anyr(home: &Path, url: &str, extra: &[(&str, &str)]) {
    let out = anyr_args(home, url, &["help"], extra);
    assert!(out.status.success(), "{out:?}");
}

/// `anyr <args>` with telemetry allowed, output captured.
fn anyr_args(
    home: &Path,
    url: &str,
    args: &[&str],
    extra: &[(&str, &str)],
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.args(args)
        .env("NO_COLOR", "1")
        .env("HOME", home)
        .env("ANYROUTER_HOME", home.join("anyr"))
        .env("ANYR_TELEMETRY_URL", url)
        .env_remove("CI")
        .env_remove("ANYR_NO_UPDATE")
        .env_remove("DO_NOT_TRACK")
        .env_remove("ANYR_NO_TELEMETRY")
        .env_remove("ANYR_TELEMETRY")
        .stdin(Stdio::null());
    for (k, v) in extra {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

fn state(home: &Path) -> std::path::PathBuf {
    home.join("anyr").join("telemetry.json")
}

#[test]
fn first_run_sends_one_install_event_and_a_second_run_sends_nothing() {
    let home = common::unique_temp_dir("anyr-telemetry-on");
    let (url, rx) = capture_server();
    anyr(&home, &url, &[]);
    let body = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("no install event reached the server");
    assert_eq!(body["event"], "cli_install");
    assert_eq!(body["api_key"], anyr_cli::telemetry::POSTHOG_KEY);
    let id = body["distinct_id"].as_str().unwrap();
    assert_eq!(id.len(), 32);
    assert!(id.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
    let props = &body["properties"];
    assert_eq!(props["$process_person_profile"], false);
    assert_eq!(props["version"], anyr_cli::VERSION);
    assert_eq!(props["os"], std::env::consts::OS);
    assert_eq!(props["arch"], std::env::consts::ARCH);

    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state(&home)).unwrap()).unwrap();
    assert_eq!(saved["install_id"], id);

    anyr(&home, &url, &[]);
    assert!(
        rx.recv_timeout(Duration::from_millis(800)).is_err(),
        "the same version must not ping twice"
    );
}

#[test]
fn a_new_version_sends_an_update_event_with_the_same_id() {
    let home = common::unique_temp_dir("anyr-telemetry-update");
    std::fs::create_dir_all(home.join("anyr")).unwrap();
    std::fs::write(
        state(&home),
        r#"{"install_id":"0123456789abcdef0123456789abcdef","last_version":"0.0.1"}"#,
    )
    .unwrap();
    let (url, rx) = capture_server();
    anyr(&home, &url, &[]);
    let body = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("no update event");
    assert_eq!(body["event"], "cli_update");
    assert_eq!(body["distinct_id"], "0123456789abcdef0123456789abcdef");
    assert_eq!(body["properties"]["from_version"], "0.0.1");
    // The stamp must land in the existing file, or every later run would
    // send cli_update again.
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state(&home)).unwrap()).unwrap();
    assert_eq!(saved["last_version"], anyr_cli::VERSION);
    assert_eq!(saved["install_id"], "0123456789abcdef0123456789abcdef");
}

#[test]
fn daily_ping_fires_once_per_utc_day_and_reuses_the_install_id() {
    let day = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86400)
        .unwrap_or(0);
    let home = common::unique_temp_dir("anyr-telemetry-daily");
    std::fs::create_dir_all(home.join("anyr")).unwrap();
    let id = "0123456789abcdef0123456789abcdef";
    std::fs::write(
        state(&home),
        format!(
            r#"{{"install_id":"{id}","last_version":"{}","last_daily":{}}}"#,
            anyr_cli::VERSION,
            day - 1
        ),
    )
    .unwrap();
    let (url, rx) = capture_server();
    anyr(&home, &url, &[]);
    let body = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("no daily event reached the server");
    assert_eq!(body["event"], "cli_daily");
    assert_eq!(body["distinct_id"], id, "daily reuses the install id");
    assert_eq!(body["properties"]["version"], anyr_cli::VERSION);

    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state(&home)).unwrap()).unwrap();
    assert_eq!(
        saved["last_daily"].as_u64(),
        Some(day),
        "daily stamp lands on today"
    );

    anyr(&home, &url, &[]);
    assert!(
        rx.recv_timeout(Duration::from_millis(800)).is_err(),
        "the daily ping must not fire twice in one UTC day"
    );
}

#[test]
fn daily_child_posts_the_payload_it_reads_from_stdin_not_argv() {
    let (url, rx) = capture_server();
    let payload = serde_json::json!({
        "api_key": anyr_cli::telemetry::POSTHOG_KEY,
        "event": "cli_daily",
        "distinct_id": "0123456789abcdef0123456789abcdef",
        "properties": {"$lib": "anyr-cli"},
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .arg("__telemetry-daily")
        .env("ANYR_TELEMETRY_URL", &url)
        .env_remove("CI")
        .env_remove("DO_NOT_TRACK")
        .env_remove("ANYR_NO_TELEMETRY")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn anyr");
    let mut stdin = child.stdin.take().expect("stdin piped");
    stdin.write_all(payload.to_string().as_bytes()).unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success());
    let body = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the child never posted the stdin payload");
    assert_eq!(body, payload);
}

#[test]
fn every_opt_out_sends_nothing_and_writes_nothing() {
    type OptOut<'a> = (&'a str, &'a [(&'a str, &'a str)], Option<&'a str>);
    let cases: &[OptOut] = &[
        ("do-not-track", &[("DO_NOT_TRACK", "1")], None),
        ("anyr-no-telemetry", &[("ANYR_NO_TELEMETRY", "1")], None),
        ("anyr-telemetry-0", &[("ANYR_TELEMETRY", "0")], None),
        ("ci", &[("CI", "true")], None),
        ("no-update-switch", &[("ANYR_NO_UPDATE", "1")], None),
        ("config-flag", &[], Some("telemetry: false\n")),
    ];
    for (name, env, config) in cases {
        let home = common::unique_temp_dir(&format!("anyr-telemetry-off-{name}"));
        if let Some(body) = config {
            std::fs::create_dir_all(home.join("anyr")).unwrap();
            std::fs::write(home.join("anyr").join("config.yaml"), body).unwrap();
        }
        let (url, rx) = capture_server();
        anyr(&home, &url, env);
        assert!(
            rx.recv_timeout(Duration::from_millis(800)).is_err(),
            "{name}: a ping was sent"
        );
        assert!(!state(&home).exists(), "{name}: wrote an install id");
        assert!(!usage(&home).exists(), "{name}: wrote counters");
    }
}

fn usage(home: &Path) -> std::path::PathBuf {
    home.join("anyr").join("usage.json")
}

fn today() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86400)
        .unwrap()
}

#[test]
fn counters_hold_registry_names_only_and_ride_the_next_daily_ping() {
    // WHY: the counters are the one place argv could leak into a payload;
    // a secret typed as a command or flag must come out as `other`/`usage`.
    let home = common::unique_temp_dir("anyr-telemetry-counters");
    let (url, rx) = capture_server();
    let secret = "sk-ar-v1-countersecret0123456789";
    let _ = anyr_args(&home, &url, &["--version"], &[]);
    assert!(!usage(&home).exists(), "--version is not counted");
    assert!(anyr_args(&home, &url, &["help"], &[]).status.success());
    rx.recv_timeout(Duration::from_secs(5)).expect("install");
    let bad = anyr_args(&home, &url, &[secret, "--key", secret], &[]);
    assert_eq!(bad.status.code(), Some(2));
    let bad = anyr_args(
        &home,
        &url,
        &["models", "--no-such-flag", "/home/me/x"],
        &[],
    );
    assert_eq!(bad.status.code(), Some(2));

    let raw = std::fs::read_to_string(usage(&home)).unwrap();
    assert!(
        !raw.contains("countersecret") && !raw.contains("/home/"),
        "{raw}"
    );
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let day = &v[today().to_string()];
    assert_eq!(day["commands"]["help"], 1, "{raw}");
    assert_eq!(day["commands"]["other"], 1, "{raw}");
    assert_eq!(day["commands"]["models"], 1, "{raw}");
    assert_eq!(day["exit"]["ok"], 1, "{raw}");
    assert_eq!(day["exit"]["usage"], 2, "{raw}");
    assert_eq!(day["errors"]["usage"], 2, "{raw}");
    assert!(day["slowest"].is_string(), "{raw}");

    // Move those counts to yesterday and make the daily ping due.
    let yesterday = today() - 1;
    std::fs::write(usage(&home), format!(r#"{{"{yesterday}":{day}}}"#)).unwrap();
    let mut st: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state(&home)).unwrap()).unwrap();
    st["last_daily"] = yesterday.into();
    std::fs::write(state(&home), st.to_string()).unwrap();
    anyr(&home, &url, &[]);
    let body = rx.recv_timeout(Duration::from_secs(5)).expect("daily");
    assert_eq!(body["event"], "cli_daily");
    let props = &body["properties"];
    assert_eq!(props["commands"]["help"], 1);
    assert_eq!(props["exit"]["usage"], 2);
    assert_eq!(props["slowest"], day["slowest"]);
    let left: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(usage(&home)).unwrap()).unwrap();
    assert!(
        left.get(yesterday.to_string()).is_none(),
        "sent days are cleared: {left}"
    );
    assert_eq!(left[today().to_string()]["commands"]["help"], 1);
}

#[cfg(unix)]
#[test]
fn a_launcher_counts_its_agent_and_the_agents_own_exit_code() {
    use std::os::unix::fs::PermissionsExt;
    let home = common::unique_temp_dir("anyr-telemetry-agent");
    let (url, _rx) = capture_server();
    let agent = home.join("fake-claude");
    std::fs::write(&agent, "#!/bin/sh\nsleep 1.2\nexit 3\n").unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = anyr_args(
        &home,
        &url,
        &[
            "claude",
            "--yes",
            "--no-mcp",
            "--key",
            "sk-ar-v1-fixture-key-0001",
            "--command-path",
            agent.to_str().unwrap(),
        ],
        &[("ANYR_NO_CATALOG", "1")],
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(usage(&home)).unwrap()).unwrap();
    let day = &v[today().to_string()];
    assert_eq!(day["commands"]["claude"], 1, "{day}");
    assert_eq!(day["agents"]["claude"], 1, "{day}");
    assert_eq!(day["exit"]["agent"], 1, "{day}");
    assert!(day["errors"].as_object().unwrap().is_empty(), "{day}");
    // anyr's own time ends at the spawn, not when the agent session ends.
    let slowest = day["slowest"].as_str().unwrap();
    assert!(matches!(slowest, "<100ms" | "<1s"), "{day}");
}

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn telemetry_off_as_the_first_command_sends_nothing_and_later_runs_stay_quiet() {
    // WHY: a user who opts out before anything else must never be counted,
    // not even by the install ping of the opt-out command itself.
    let home = common::unique_temp_dir("anyr-telemetry-cmd-off");
    let (url, rx) = capture_server();
    let out = anyr_args(&home, &url, &["telemetry", "off", "--json"], &[]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(json(&out)["enabled"], false);
    let config = std::fs::read_to_string(home.join("anyr").join("config.yaml")).unwrap();
    assert!(config.contains("telemetry: false"), "{config}");
    anyr(&home, &url, &[]);
    assert!(
        rx.recv_timeout(Duration::from_millis(800)).is_err(),
        "a ping was sent after telemetry off"
    );
    assert!(!state(&home).exists(), "wrote an install id");

    let out = anyr_args(&home, &url, &["telemetry", "status", "--json"], &[]);
    let v = json(&out);
    assert_eq!(v["enabled"], false);
    assert_eq!(v["reason"], "telemetry: false in the config");
    assert!(v["next_daily"].is_null());

    let out = anyr_args(&home, &url, &["telemetry", "on", "--json"], &[]);
    assert_eq!(json(&out)["enabled"], true);
    anyr(&home, &url, &[]);
    let body = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("telemetry on did not turn the ping back on");
    assert_eq!(body["event"], "cli_install");
}

#[test]
fn status_names_the_switch_that_turned_it_off() {
    let home = common::unique_temp_dir("anyr-telemetry-why");
    let (url, _rx) = capture_server();
    for (k, v, why) in [
        ("ANYR_TELEMETRY", "0", "ANYR_TELEMETRY=0"),
        ("ANYR_NO_TELEMETRY", "1", "ANYR_NO_TELEMETRY=1"),
        ("DO_NOT_TRACK", "1", "DO_NOT_TRACK=1"),
        ("CI", "true", "CI"),
    ] {
        let out = anyr_args(&home, &url, &["telemetry", "--json"], &[(k, v)]);
        let v = json(&out);
        assert_eq!(v["enabled"], false, "{k}");
        assert_eq!(v["reason"], why, "{k}");
    }
    let v = json(&anyr_args(&home, &url, &["telemetry", "--json"], &[]));
    assert_eq!(v["enabled"], true);
    assert_eq!(v["reason"], "default");
    assert_eq!(v["endpoint"], url.as_str());
}

#[test]
fn status_shows_the_exact_daily_payload_that_is_then_sent() {
    // WHY: the status output is the user's audit of what leaves the machine;
    // it must be the very bytes the daily ping posts, not a description.
    let day = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86400)
        .unwrap();
    let home = common::unique_temp_dir("anyr-telemetry-next");
    std::fs::create_dir_all(home.join("anyr")).unwrap();
    let id = "0123456789abcdef0123456789abcdef";
    std::fs::write(
        state(&home),
        format!(
            r#"{{"install_id":"{id}","last_version":"{}","last_daily":{}}}"#,
            anyr_cli::VERSION,
            day - 1
        ),
    )
    .unwrap();
    let (url, rx) = capture_server();
    let v = json(&anyr_args(
        &home,
        &url,
        &["telemetry", "status", "--json"],
        &[],
    ));
    assert_eq!(v["install_id"], id);
    let next = v["next_daily"].clone();
    assert_eq!(next["event"], "cli_daily");
    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "status itself must not send"
    );
    anyr(&home, &url, &[]);
    let body = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("no daily ping");
    assert_eq!(body, next);

    let v = json(&anyr_args(
        &home,
        &url,
        &["telemetry", "status", "--dry-run", "--json"],
        &[],
    ));
    let events: Vec<&str> = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert_eq!(
        events,
        ["cli_install", "cli_update", "cli_daily", "cli_crash"]
    );
}

#[test]
fn reset_rotates_the_id_while_on_and_deletes_it_while_off() {
    let home = common::unique_temp_dir("anyr-telemetry-reset");
    let (url, rx) = capture_server();
    anyr(&home, &url, &[]);
    let first = rx.recv_timeout(Duration::from_secs(5)).expect("install");
    let old = first["distinct_id"].as_str().unwrap().to_string();
    let v = json(&anyr_args(
        &home,
        &url,
        &["telemetry", "reset", "--json"],
        &[],
    ));
    let new = v["install_id"].as_str().unwrap().to_string();
    assert_ne!(new, old);
    assert_eq!(new.len(), 32);
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state(&home)).unwrap()).unwrap();
    assert_eq!(saved["install_id"], new.as_str());

    let v = json(&anyr_args(
        &home,
        &url,
        &["telemetry", "reset", "--json"],
        &[("DO_NOT_TRACK", "1")],
    ));
    assert!(v["install_id"].is_null());
    assert!(
        !state(&home).exists(),
        "reset while off must not keep an id"
    );
}

#[test]
fn the_notice_never_prints_off_a_terminal() {
    // WHY: agents and scripts parse stderr; the one-time notice is for a
    // person, and printing it off a TTY would also burn its only showing.
    let home = common::unique_temp_dir("anyr-telemetry-notice-pipe");
    let (url, _rx) = capture_server();
    let out = anyr_args(&home, &url, &["help"], &[]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("telemetry off"), "{err}");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state(&home)).unwrap()).unwrap();
    assert_eq!(saved["notice_shown"], false);
}

fn pty(home: &Path, url: &str, args: &[&str]) -> Option<String> {
    let helper = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".agents/skills/verify-anyr/helpers/pty-anyr.py");
    if !helper.exists() || cfg!(windows) {
        return None; // PTY helper is dev-machine tooling.
    }
    let transcript = home.join(format!("tty-{}.txt", args.join("-")));
    let out = Command::new("python3")
        .args([
            helper.to_str().unwrap(),
            "--timeout",
            "8",
            "--out",
            transcript.to_str().unwrap(),
            "--",
            env!("CARGO_BIN_EXE_anyr"),
        ])
        .args(args)
        .env("NO_COLOR", "1")
        .env("HOME", home)
        .env("ANYROUTER_HOME", home.join("anyr"))
        .env("ANYR_TELEMETRY_URL", url)
        .env_remove("CI")
        .env_remove("ANYR_NO_UPDATE")
        .env_remove("DO_NOT_TRACK")
        .env_remove("ANYR_NO_TELEMETRY")
        .env_remove("ANYR_TELEMETRY")
        .output()
        .ok()?;
    Some(
        std::fs::read_to_string(&transcript)
            .unwrap_or_else(|_| String::from_utf8_lossy(&out.stdout).into_owned()),
    )
}

#[test]
fn the_notice_prints_once_on_the_first_terminal_run_and_never_under_json() {
    let home = common::unique_temp_dir("anyr-telemetry-notice-tty");
    let (url, _rx) = capture_server();
    let Some(json_run) = pty(&home, &url, &["config", "path", "--json"]) else {
        return;
    };
    assert!(!json_run.contains("telemetry off"), "{json_run}");
    let first = pty(&home, &url, &["config", "path"]).unwrap();
    assert!(first.contains("telemetry off"), "{first}");
    let second = pty(&home, &url, &["config", "path"]).unwrap();
    assert!(!second.contains("telemetry off"), "{second}");
}

fn crash_files(home: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(home.join("anyr").join("crash"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

#[test]
fn a_panic_leaves_a_redacted_crash_file_and_the_next_run_posts_it_once() {
    // WHY: the crash path is the one place free text (the panic message) is
    // stored and sent; the secret and path in it must never leave, and one
    // crash must be one upload.
    let home = common::unique_temp_dir("anyr-telemetry-crash");
    let (url, rx) = capture_server();
    anyr(&home, &url, &[]);
    rx.recv_timeout(Duration::from_secs(5)).expect("install");
    let boom = "boom sk-ar-v1-panicsecret0123456789 at /home/alice/secret.txt";
    let out = anyr_args(
        &home,
        &url,
        &["config", "path"],
        &[("ANYR_TEST_PANIC", boom)],
    );
    assert!(!out.status.success(), "{out:?}");
    let files = crash_files(&home);
    assert_eq!(files.len(), 1, "{files:?}");
    let raw = std::fs::read_to_string(&files[0]).unwrap();
    assert_eq!(anyr_cli::report::leaks(&raw), None, "{raw}");
    assert!(
        !raw.contains("panicsecret") && !raw.contains("alice"),
        "{raw}"
    );
    let rec: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(rec["command"], "config");
    let location = rec["location"].as_str().unwrap();
    assert!(
        location.starts_with("anyr-cli/src/commands.rs:"),
        "{location}"
    );
    assert!(
        rec["breadcrumbs"]
            .as_array()
            .unwrap()
            .contains(&"config.read".into()),
        "{raw}"
    );
    assert_eq!(rec["sent"], false);
    let fp = rec["fingerprint"].as_str().unwrap().to_string();
    assert_eq!(
        files[0].file_name().unwrap().to_string_lossy(),
        format!("{fp}.json")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    anyr(&home, &url, &[]);
    let body = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("no cli_crash posted");
    assert_eq!(body["event"], "cli_crash");
    assert_eq!(body["properties"]["fingerprint"], fp.as_str());
    assert_eq!(body["properties"]["message"], rec["message"]);
    let sent: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
    assert_eq!(sent["sent"], true);
    anyr(&home, &url, &[]);
    assert!(
        rx.recv_timeout(Duration::from_millis(800)).is_err(),
        "a crash already sent went out again"
    );

    // The same bug again bumps the count in the same file.
    let _ = anyr_args(
        &home,
        &url,
        &["config", "path"],
        &[("ANYR_TEST_PANIC", boom)],
    );
    assert_eq!(crash_files(&home).len(), 1);
    let again: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
    assert_eq!(again["count"], 2);
}

#[test]
fn a_panic_while_telemetry_is_off_writes_nothing() {
    let home = common::unique_temp_dir("anyr-telemetry-crash-off");
    let (url, _rx) = capture_server();
    let out = anyr_args(
        &home,
        &url,
        &["config", "path"],
        &[("ANYR_TEST_PANIC", "boom"), ("DO_NOT_TRACK", "1")],
    );
    assert!(!out.status.success());
    assert!(crash_files(&home).is_empty());
}
