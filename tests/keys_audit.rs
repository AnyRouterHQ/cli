//! Device login naming, the re-login revoke offer, `keys audit` and
//! `keys revoke <last4>` against a local scripted server. Nothing touches
//! anyrouter.dev and nothing is revoked without a yes.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;

const OLD_KEY: &str = "sk-ar-v1-oldoldoldoldoldoldoldoldoldoldaaaa";
const NEW_KEY: &str = "sk-ar-v1-newnewnewnewnewnewnewnewnewnewbbbb";

#[derive(Debug)]
struct Seen {
    method: String,
    path: String,
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
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut req_body = vec![0u8; len];
            reader.read_exact(&mut req_body).unwrap();
            let mut parts = line.split_whitespace();
            let seen = Seen {
                method: parts.next().unwrap_or("").into(),
                path: parts.next().unwrap_or("").into(),
                body: String::from_utf8_lossy(&req_body).into_owned(),
            };
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            // Record the request before answering: once the client has its
            // reply it may exit, and the test may read `seen` at once.
            tx.send(seen).unwrap();
            stream.write_all(resp.as_bytes()).unwrap();
        }
    });
    (base, rx)
}

fn home_with_key(tag: &str, key: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("anyr-keys-audit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.yaml"),
        format!("active_profile: default\nprofiles:\n  default:\n    api_key: {key}\n"),
    )
    .unwrap();
    home
}

fn run(home: &PathBuf, base: &str, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .env("ANYR_NO_UPDATE", "1")
        .env("NO_COLOR", "1")
        .env("SSH_CONNECTION", "test")
        .env_remove("ANYROUTER_API_KEY")
        .env("ANYROUTER_HOME", home)
        .args(args)
        .args(["--base-url", base])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn row(name: &str, tail: &str, origin: Option<&str>, created: &str, used: Option<&str>) -> String {
    serde_json::json!({
        "id": format!("key_{tail}"),
        "hash": format!("hash{tail}{tail}"),
        "name": name,
        "label": format!("sk-ar-v1-...{tail}"),
        "origin": origin,
        "created_at": created,
        "last_used_at": used,
        "real_usage_monthly_requests": 0,
        "disabled": false,
    })
    .to_string()
}

#[test]
fn device_relogin_names_the_key_and_only_hints_at_revoking_the_old_one() {
    let keys = format!(
        r#"{{"data":[{},{}]}}"#,
        row(
            "studio anyr 2026-10-27",
            "bbbb",
            Some("cli:cli"),
            "2026-10-27T10:00:00Z",
            None
        ),
        row(
            "studio anyr 2026-10-01",
            "aaaa",
            Some("cli:cli"),
            "2026-10-01T10:00:00Z",
            Some("2026-10-26T10:00:00Z")
        ),
    );
    let (base, seen) = serve(vec![
        (
            200,
            r#"{"device_code":"dc","user_code":"ABCD-EFGH","verification_uri":"http://x/cli/device","interval":0,"expires_in":600}"#.into(),
        ),
        (200, format!(r#"{{"key":"{NEW_KEY}","user_id":"u1"}}"#)),
        (200, r#"{"data":{"total_credits":1}}"#.into()),
        (200, keys),
    ]);
    let home = home_with_key("relogin", OLD_KEY);
    let (code, out, err) = run(&home, &base, &["login", "--device"]);
    assert_eq!(code, 0, "{out}\n{err}");

    let requests: Vec<Seen> = seen.try_iter().collect();
    let start: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    let label = start["key_label"].as_str().unwrap();
    let (host, date) = label.rsplit_once(" anyr ").expect(label);
    assert!(!host.is_empty() && date.len() == 10, "{label}");

    // Scripted run: the old device key is named with a revoke command, and
    // nothing is deleted.
    assert!(err.contains("keys revoke aaaa"), "{err}");
    assert!(
        requests.iter().all(|r| r.method != "DELETE"),
        "{requests:?}"
    );
    let all = format!("{out}{err}");
    assert!(!all.contains(NEW_KEY) && !all.contains(OLD_KEY), "{all}");
    let cfg = std::fs::read_to_string(home.join("config.yaml")).unwrap();
    assert!(cfg.contains(NEW_KEY), "new key saved");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn keys_audit_lists_suggestions_but_never_the_key_in_use() {
    // The current key was never used and is old: only "current" protects it.
    let keys = format!(
        r#"{{"data":[{},{},{}]}}"#,
        row("this-machine", "bbbb", None, "2026-01-01T00:00:00Z", None),
        row("ci-old", "aaaa", None, "2026-01-01T00:00:00Z", None),
        row(
            "busy",
            "cccc",
            None,
            "2026-01-01T00:00:00Z",
            Some("2099-01-01T00:00:00Z")
        ),
    );
    let (base, _seen) = serve(vec![(200, keys.clone()), (200, keys)]);
    let home = home_with_key("audit", NEW_KEY);
    let (code, out, err) = run(&home, &base, &["keys", "audit"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("ci-old") && out.contains("…aaaa"), "{out}");
    assert!(out.contains("never used"), "{out}");
    assert!(!out.contains("this-machine"), "{out}");
    assert!(!out.contains("busy"), "{out}");
    assert!(!out.contains("sk-ar-v1-"), "{out}");

    let (code, out, _) = run(&home, &base, &["keys", "audit", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["reason"], "never_used");
    assert_eq!(v[0]["last4"], "aaaa");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn keys_use_is_gone_and_points_at_login() {
    let home = home_with_key("use", NEW_KEY);
    let (code, _, err) = run(&home, "http://127.0.0.1:9/api", &["keys", "use"]);
    assert_ne!(code, 0);
    assert!(err.contains("login"), "{err}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn keys_revoke_by_last4_deletes_that_hash_after_yes() {
    let keys = format!(
        r#"{{"data":[{}]}}"#,
        row("ci-old", "aaaa", None, "2026-01-01T00:00:00Z", None)
    );
    let (base, seen) = serve(vec![(200, keys), (200, "{}".into())]);
    let home = home_with_key("revoke", NEW_KEY);
    let (code, out, err) = run(&home, &base, &["keys", "revoke", "aaaa", "--yes"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("…aaaa"), "{out}");
    let requests: Vec<Seen> = seen.try_iter().collect();
    assert_eq!(requests[1].method, "DELETE");
    assert!(
        requests[1].path.ends_with("/v1/keys/hashaaaaaaaa"),
        "{requests:?}"
    );
    let _ = std::fs::remove_dir_all(&home);
}
