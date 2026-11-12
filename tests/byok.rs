//! `anyr byok` end to end against a local scripted HTTP server, so nothing
//! touches anyrouter.dev. Every test sets stdin explicitly.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const SECRET: &str = "sk-proj-SECRETprovider0123456789";
const MGMT: &str = "ak_test_management";

/// One request as the server saw it.
#[derive(Debug)]
struct Seen {
    method: String,
    path: String,
    auth: String,
    body: String,
}

/// Serve `replies` in order, one per connection.
fn serve(replies: Vec<(u16, String)>) -> (String, mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/api", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for (status, body) in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let (mut auth, mut len) = (String::new(), 0usize);
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                let lower = h.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
                if lower.starts_with("authorization:") {
                    auth = h["authorization:".len()..].trim().to_string();
                }
            }
            let mut req_body = vec![0u8; len];
            reader.read_exact(&mut req_body).unwrap();
            let mut parts = line.split_whitespace();
            let seen = Seen {
                method: parts.next().unwrap_or("").into(),
                path: parts.next().unwrap_or("").into(),
                auth,
                body: String::from_utf8_lossy(&req_body).into_owned(),
            };
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).unwrap();
            tx.send(seen).unwrap();
        }
    });
    (base, rx)
}

fn anyr(base: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.env("ANYR_NO_UPDATE", "1")
        .env("NO_COLOR", "1")
        .env("ANYROUTER_MANAGEMENT_KEY", MGMT)
        .env_remove("ANYR_BYOK_KEY")
        .env(
            "ANYROUTER_HOME",
            std::env::temp_dir().join("anyr-byok-test-empty"),
        )
        .env("ANYR_TEST_BASE", base);
    cmd
}

/// Run with `stdin` piped in (or /dev/null when `None`).
fn run(mut cmd: Command, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let base = cmd
        .get_envs()
        .find(|(k, _)| *k == "ANYR_TEST_BASE")
        .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
        .unwrap_or_default();
    cmd.args(args)
        .args(["--base-url", &base])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = cmd.spawn().expect("spawn anyr");
    if let Some(input) = stdin {
        // The child may exit before reading (usage errors); ignore EPIPE.
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn assert_no_secret(out: &str, err: &str) {
    assert!(!out.contains(SECRET), "key leaked to stdout:\n{out}");
    assert!(!err.contains(SECRET), "key leaked to stderr:\n{err}");
}

fn created() -> (u16, String) {
    (
        201,
        r#"{"id":"byok_new1","provider_id":"openai","key_preview":"sk-p…6789"}"#.into(),
    )
}

#[test]
fn add_from_stdin_sends_source_cli_kind_byok_and_never_prints_key() {
    // WHY: the key is a third-party credential; it may only travel in the
    // request body, and the server must learn the request came from the CLI.
    let (base, rx) = serve(vec![created()]);
    let (code, out, err) = run(anyr(&base), &["byok", "add", "openai"], Some(SECRET));
    assert_eq!(code, 0, "{out}{err}");
    let seen = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        (seen.method.as_str(), seen.path.as_str()),
        ("POST", "/api/v1/auth/byok")
    );
    assert_eq!(seen.auth, format!("Bearer {MGMT}"));
    let body: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(body["api_key"], SECRET);
    assert_eq!(body["provider_id"], "openai");
    assert_eq!(body["source"], "cli");
    assert_eq!(body["kind"], "byok");
    assert!(out.contains("BYOK"), "{out}");
    assert_no_secret(&out, &err);
}

#[test]
fn server_error_echoing_the_key_is_redacted() {
    // WHY: a provider-validation error may quote the submitted key back.
    let (base, _rx) = serve(vec![(
        400,
        format!(r#"{{"message":"provider rejected key {SECRET}"}}"#),
    )]);
    let (code, out, err) = run(anyr(&base), &["byok", "add", "openai"], Some(SECRET));
    assert_ne!(code, 0);
    assert!(err.contains("HTTP 400"), "{err}");
    assert_no_secret(&out, &err);
}

#[test]
fn donate_with_yes_shows_consent_then_creates_and_donates() {
    // WHY: the pool terms must be shown before anything is sent, and the
    // donation must carry the consent version the server checks.
    let (base, rx) = serve(vec![
        created(),
        (201, r#"{"id":"don_1","status":"active"}"#.into()),
    ]);
    let mut cmd = anyr(&base);
    cmd.env("ANYR_BYOK_KEY", SECRET);
    let (code, out, err) = run(cmd, &["byok", "--donate", "openai", "--yes"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert!(err.contains("By donating this key"), "{err}");
    let create = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let body: serde_json::Value = serde_json::from_str(&create.body).unwrap();
    assert_eq!(body["kind"], "donated");
    assert_eq!(body["source"], "cli");
    let donate = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(donate.path, "/api/v1/byok/donated");
    let body: serde_json::Value = serde_json::from_str(&donate.body).unwrap();
    assert_eq!(body["byok_key_id"], "byok_new1");
    assert_eq!(body["consent"], "v5");
    assert!(!donate.body.contains(SECRET), "donate body resent the key");
    assert!(out.contains("DONATED"), "{out}");
    assert_no_secret(&out, &err);
}

#[test]
fn donate_without_terminal_or_yes_exits_6_and_sends_nothing() {
    // WHY: donating shares a paid credential with strangers; a script must
    // opt in explicitly with --yes, and nothing may be read or sent first.
    let (base, rx) = serve(vec![created()]);
    let (code, out, err) = run(
        anyr(&base),
        &["byok", "add", "openai", "--donate"],
        Some(SECRET),
    );
    assert_eq!(code, 6, "{out}{err}");
    assert!(
        err.contains("byok add openai --donate") && err.contains("--yes"),
        "{err}"
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "a request was sent without consent"
    );
    assert_no_secret(&out, &err);
}

#[test]
fn donate_rejected_names_the_private_key_left_behind() {
    // WHY: today the donate route rejects non-session credentials; the user
    // must learn the key exists as a private BYOK key, not believe it donated.
    let (base, _rx) = serve(vec![
        created(),
        (
            403,
            r#"{"message":"Management API key not allowed on this route"}"#.into(),
        ),
    ]);
    let mut cmd = anyr(&base);
    cmd.env("ANYR_BYOK_KEY", SECRET);
    let (code, out, err) = run(cmd, &["byok", "add", "openai", "--donate", "--yes"], None);
    assert_ne!(code, 0, "{out}{err}");
    assert!(!out.contains("Donated"), "{out}");
    assert!(
        err.contains("byok_new1") && err.contains("private"),
        "{err}"
    );
    assert!(!err.contains("login"), "misleading login hint:\n{err}");
    assert_no_secret(&out, &err);
}

#[test]
fn key_as_argument_is_refused_without_echo() {
    let (base, rx) = serve(vec![created()]);
    let (code, out, err) = run(anyr(&base), &["byok", "add", "openai", SECRET], None);
    assert_eq!(code, 2, "{out}{err}");
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    assert_no_secret(&out, &err);
}

const KEYS: &str = r#"{"object":"list","data":[{"id":"byok_a","provider_id":"openai","label":null,"key_preview":"sk-…aaaa"},{"id":"byok_b","provider_id":"anthropic","label":"work","key_preview":"sk-ant…bbbb"}]}"#;

#[test]
fn list_badges_donated_keys_from_the_donations_join() {
    let (base, rx) = serve(vec![
        (200, KEYS.into()),
        (
            200,
            r#"{"data":[{"byok_key_id":"byok_b","status":"active"}]}"#.into(),
        ),
    ]);
    let (code, out, err) = run(anyr(&base), &["byok", "list"], None);
    assert_eq!(code, 0, "{err}");
    let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(first.path.starts_with("/api/v1/auth/byok"), "{first:?}");
    assert!(first.path.contains("source=cli"), "{first:?}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(lines[0].starts_with("BYOK\topenai\tbyok_a"), "{out}");
    assert!(lines[1].starts_with("DONATED\tanthropic\tbyok_b"), "{out}");
}

#[test]
fn list_json_uses_lowercase_kind_and_server_kind_wins() {
    let keys = r#"{"data":[{"id":"byok_a","provider_id":"openai","kind":"donated"}]}"#;
    let (base, rx) = serve(vec![(200, keys.into())]);
    let (code, out, err) = run(anyr(&base), &["byok", "--json"], None);
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v[0]["kind"], "donated");
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "donations fetched although the server already sent kind"
    );
}

#[test]
fn list_marks_unknown_when_donations_unreadable() {
    // WHY: never call a key private (BYOK) when we could not check.
    let (base, _rx) = serve(vec![
        (200, KEYS.into()),
        (403, r#"{"message":"no"}"#.into()),
    ]);
    let (code, out, err) = run(anyr(&base), &["byok", "list"], None);
    assert_eq!(code, 0, "{err}");
    assert!(out.lines().all(|l| l.starts_with("?\t")), "{out}");
    assert!(err.contains("could not read donated keys"), "{err}");
}

#[test]
fn missing_management_key_explains_scopes() {
    let (base, _rx) = serve(vec![]);
    let mut cmd = anyr(&base);
    cmd.env_remove("ANYROUTER_MANAGEMENT_KEY");
    let (code, _out, err) = run(cmd, &["byok", "list"], None);
    assert_ne!(code, 0);
    assert!(
        err.contains("read:byok") && err.contains("write:byok"),
        "{err}"
    );
}

#[test]
fn rm_with_yes_deletes_and_without_yes_off_a_tty_sends_nothing() {
    // WHY: deleting a provider key is destructive; scripts must opt in.
    let (base, rx) = serve(vec![(200, r#"{"deleted":true}"#.into())]);
    let (code, _, err) = run(anyr(&base), &["byok", "rm", "openai"], None);
    assert_eq!(code, 6, "{err}");
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    let (code, out, err) = run(anyr(&base), &["byok", "rm", "openai", "--yes"], None);
    assert_eq!(code, 0, "{out}{err}");
    let seen = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        (seen.method.as_str(), seen.path.as_str()),
        ("DELETE", "/api/v1/auth/byok/openai")
    );
    assert_eq!(seen.auth, format!("Bearer {MGMT}"));
}

#[test]
fn rm_of_a_missing_key_exits_1() {
    let (base, _rx) = serve(vec![(200, r#"{"deleted":false}"#.into())]);
    let (code, _, err) = run(anyr(&base), &["byok", "rm", "openai", "--yes"], None);
    assert_eq!(code, 1, "{err}");
}

#[test]
fn test_saved_key_posts_provider_only_and_reports_failure() {
    let (base, rx) = serve(vec![
        (
            200,
            r#"{"ok":true,"backend":"openai-byok","latency_ms":300}"#.into(),
        ),
        (
            200,
            r#"{"ok":false,"error_message":"invalid api key"}"#.into(),
        ),
    ]);
    let (code, out, err) = run(anyr(&base), &["byok", "test", "openai"], None);
    assert_eq!(code, 0, "{out}{err}");
    let seen = rx.recv().unwrap();
    assert_eq!(seen.path, "/api/v1/auth/byok/test-inference");
    let body: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(body, serde_json::json!({"provider_id": "openai"}));
    let (code, _, err) = run(anyr(&base), &["byok", "test", "openai"], None);
    assert_eq!(code, 1);
    assert!(
        err.starts_with("error:") && err.contains("invalid api key"),
        "{err}"
    );
}

#[test]
fn test_key_stdin_probes_unsaved_key_and_never_echoes_it() {
    let (base, rx) = serve(vec![(
        200,
        format!(r#"{{"validation_state":"invalid","diagnostics":{{"message":"bad {SECRET}"}}}}"#),
    )]);
    let (code, out, err) = run(
        anyr(&base),
        &["byok", "test", "openai", "--key-stdin"],
        Some(SECRET),
    );
    assert_eq!(code, 1, "{out}{err}");
    let seen = rx.recv().unwrap();
    assert_eq!(seen.path, "/api/v1/byok/test-key");
    let body: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(body["api_key"], SECRET);
    assert_no_secret(&out, &err);
}

#[test]
fn providers_lists_the_public_catalog_with_badges() {
    let (base, rx) = serve(vec![
        (
            200,
            r#"{"data":[{"id":"openai","name":"OpenAI"},{"id":"groq","name":"Groq"}]}"#.into(),
        ),
        (
            200,
            r#"{"data":[{"provider_id":"openai","kind":"byok"}]}"#.into(),
        ),
    ]);
    let (code, out, err) = run(anyr(&base), &["byok", "providers"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(rx.recv().unwrap().path, "/api/v1/byok/providers");
    assert!(out.contains("openai") && out.contains("ready"), "{out}");
    assert!(out.contains("groq"), "{out}");
}

#[test]
fn rm_if_exists_of_a_missing_key_is_success() {
    // WHY: a cleanup script must be safe to rerun; "already gone" is done.
    for reply in [
        (200, r#"{"deleted":false}"#.to_string()),
        (404, r#"{"error":{"message":"no key"}}"#.to_string()),
    ] {
        let (base, _rx) = serve(vec![reply.clone()]);
        let (code, out, err) = run(
            anyr(&base),
            &["byok", "rm", "openai", "--yes", "--if-exists", "--json"],
            None,
        );
        assert_eq!(code, 0, "{reply:?}: {out}{err}");
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["deleted"], false, "{reply:?}");
    }
}
