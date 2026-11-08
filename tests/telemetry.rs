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
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.arg("help")
        .env("NO_COLOR", "1")
        .env("HOME", home)
        .env("ANYROUTER_HOME", home.join("anyr"))
        .env("ANYR_TELEMETRY_URL", url)
        .env_remove("CI")
        .env_remove("ANYR_NO_UPDATE")
        .env_remove("DO_NOT_TRACK")
        .env_remove("ANYR_NO_TELEMETRY")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());
    for (k, v) in extra {
        cmd.env(k, v);
    }
    assert!(cmd.status().unwrap().success());
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
    }
}
