//! `anyr fx` writes the model list into ~/.fx/settings.json, and fx's `/model`
//! picker lists only what is there. A launch with a cold catalog cache must
//! wait for the catalog instead of writing a one-model list, and a catalog
//! that cannot be loaded must not shrink the list an earlier launch wrote.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

const KEY: &str = "sk-ar-v1-fxlisttest0123456789";

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("anyr-fxlist-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A gateway whose `/v1/models` answers with `status` and `body` after
/// `delay`. Every other path answers 401 at once.
fn gateway(delay: Duration, status: u16, body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/api", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let body = body.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    return;
                }
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                }
                let (status, body) = if line.contains("/v1/models") {
                    std::thread::sleep(delay);
                    (status, body)
                } else {
                    (401, r#"{"error":{"message":"x"}}"#.to_string())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    base
}

fn catalog_body(not_routable: &[&str]) -> String {
    let rows: Vec<_> = [
        ("a/chat-one", &["chat", "function-calling"][..], 128_000),
        (
            "b/chat-two",
            &["chat", "function-calling", "vision"],
            200_000,
        ),
        ("c/chat-three", &["chat", "function-calling"], 64_000),
        ("d/chat-four", &["chat", "function-calling"], 32_000),
        ("e/embedder", &["embedding"], 512),
    ]
    .iter()
    .map(|(id, caps, ctx)| {
        let mut row = serde_json::json!({ "id": id, "capabilities": caps, "context_length": ctx });
        if not_routable.contains(id) {
            row[anyr_cli::http::ROUTABLE_FIELD] = false.into();
        }
        row
    })
    .collect();
    serde_json::json!({ "data": rows }).to_string()
}

struct Run {
    out: Output,
    settings: PathBuf,
}

impl Run {
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.out.stderr).into_owned()
    }

    fn listed(&self) -> Vec<String> {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&self.settings).unwrap()).unwrap();
        v["providers"]["anyrouter"]["model_metadata"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }
}

/// `anyr fx` against `base` with a fresh ANYROUTER_HOME (so no catalog cache),
/// on `home`, with a fake fx that exits at once.
fn launch_fx(dir: &Path, home: &Path, base: &str) -> Run {
    use std::os::unix::fs::PermissionsExt;
    let fake = dir.join("fake-fx");
    std::fs::write(&fake, "#!/bin/sh\necho 0.0.14\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .env("ANYR_NO_UPDATE", "1")
        .env("NO_COLOR", "1")
        .env("HOME", home)
        .env("ANYROUTER_HOME", dir.join("anyr"))
        .env("ANYROUTER_API_KEY", KEY)
        .env_remove("XDG_CONFIG_HOME")
        .args([
            "fx",
            "--yes",
            "--no-mcp",
            "--base-url",
            base,
            "--command-path",
        ])
        .arg(&fake)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    Run {
        out,
        settings: home.join(".fx").join("settings.json"),
    }
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

#[cfg(unix)]
#[test]
fn a_cold_cache_waits_for_a_slow_catalog_and_writes_the_full_list() {
    let dir = temp_dir("cold");
    let base = gateway(Duration::from_secs(2), 200, catalog_body(&[]));
    let run = launch_fx(&dir, &dir.join("home"), &base);
    assert!(run.out.status.success(), "{}", run.stderr());
    let listed = run.listed();
    for id in ["a/chat-one", "b/chat-two", "c/chat-three", "d/chat-four"] {
        assert!(listed.contains(&id.to_string()), "{id} missing: {listed:?}");
    }
    assert!(!listed.contains(&"e/embedder".to_string()), "{listed:?}");
    assert!(
        run.stderr().contains("loading the model list"),
        "the wait is announced: {}",
        run.stderr()
    );
    assert!(!run.stderr().contains(KEY));
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_failed_fetch_keeps_the_list_an_earlier_launch_wrote() {
    let dir = temp_dir("keep");
    let home = dir.join("home");
    let up = gateway(Duration::ZERO, 200, catalog_body(&[]));
    let first = launch_fx(&dir, &home, &up);
    assert!(first.out.status.success(), "{}", first.stderr());
    let before = sorted(first.listed());
    assert!(before.len() >= 4, "{before:?}");

    // A new ANYROUTER_HOME has no catalog cache; the gateway now errors.
    let down = gateway(Duration::ZERO, 500, "{}".into());
    let again = launch_fx(&temp_dir("keep-2"), &home, &down);
    assert!(again.out.status.success(), "{}", again.stderr());
    assert_eq!(sorted(again.listed()), before, "the list shrank");
    assert!(
        again.stderr().contains("list from the last launch"),
        "{}",
        again.stderr()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_model_the_gateway_says_is_not_routable_is_not_offered() {
    let dir = temp_dir("route");
    let base = gateway(Duration::ZERO, 200, catalog_body(&["c/chat-three"]));
    let run = launch_fx(&dir, &dir.join("home"), &base);
    assert!(run.out.status.success(), "{}", run.stderr());
    let listed = run.listed();
    assert!(!listed.contains(&"c/chat-three".to_string()), "{listed:?}");
    assert!(listed.contains(&"a/chat-one".to_string()), "{listed:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
