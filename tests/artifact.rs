//! `anyr artifact` end to end against a scripted local HTTP server, so
//! nothing touches anyrouter.dev.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<String>>>;

/// Answer requests in order with `(status, body)`; `seen` collects
/// "METHOD PATH AUTH BODY" per request.
fn serve(script: Vec<(u16, String)>) -> (String, Seen) {
    serve_hook(script, |_| {})
}

/// Like `serve`, but runs `before(i)` once request `i` has arrived and
/// before its response is sent, so a test can change the world mid-publish.
fn serve_hook(
    script: Vec<(u16, String)>,
    before: impl Fn(usize) + Send + 'static,
) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/api", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        for (i, (status, body)) in script.into_iter().enumerate() {
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
            let mut req = vec![0u8; len];
            reader.read_exact(&mut req).unwrap();
            let mut parts = line.split_whitespace();
            log.lock().unwrap().push(format!(
                "{} {} {} {}",
                parts.next().unwrap_or(""),
                parts.next().unwrap_or(""),
                auth,
                String::from_utf8_lossy(&req)
            ));
            before(i);
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    (base, seen)
}

fn anyr(home: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("NO_COLOR", "1")
        .env_remove("ANYROUTER_API_KEY")
        .env("ANYROUTER_HOME", home)
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn refs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("anyr-artifact-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const OK: &str =
    r#"{"id":"abc123","version":1,"url":"https://anyrouter.dev/a/abc123","visibility":"private"}"#;

fn sha(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn file_publish_posts_inline_json_and_prints_only_the_url() {
    let dir = tmp("file");
    std::fs::write(dir.join("report.html"), "<h1>Hi</h1>").unwrap();
    let (base, seen) = serve(vec![(201, OK.into())]);
    let (code, out, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            dir.join("report.html").to_str().unwrap(),
            "--title",
            "Sales",
            "--visibility",
            "link",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    // WHY: `URL=$(anyr artifact publish x.html)` must capture just the URL.
    assert_eq!(out.trim(), "https://anyrouter.dev/a/abc123");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].starts_with("POST /api/v1/artifacts Bearer sk-ar-test "),
        "{}",
        seen[0]
    );
    let body: serde_json::Value =
        serde_json::from_str(seen[0].splitn(5, ' ').nth(4).unwrap()).unwrap();
    assert_eq!(body["title"], "Sales");
    // `link` is the old name; the CLI sends the server's name.
    assert_eq!(body["visibility"], "unlisted");
    assert_eq!(body["files"]["report.html"]["content"], "<h1>Hi</h1>");
    assert_eq!(body["files"]["report.html"]["encoding"], "utf8");
}

#[test]
fn json_flag_prints_the_server_response() {
    let dir = tmp("json");
    std::fs::write(dir.join("a.md"), "# a").unwrap();
    let (base, _) = serve(vec![(201, OK.into())]);
    let (code, out, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            dir.join("a.md").to_str().unwrap(),
            "--json",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["id"], "abc123");
}

#[test]
fn dir_publish_uploads_only_missing_blobs_and_skips_dotfiles() {
    let dir = tmp("dir");
    let site = dir.join("site");
    std::fs::create_dir_all(site.join("css")).unwrap();
    std::fs::create_dir_all(site.join("node_modules/x")).unwrap();
    std::fs::create_dir_all(site.join(".git")).unwrap();
    std::fs::write(site.join("index.html"), "<link href=css/a.css>").unwrap();
    std::fs::write(site.join("css/a.css"), "body{}").unwrap();
    std::fs::write(site.join(".env"), "SECRET=1").unwrap();
    std::fs::write(site.join("node_modules/x/i.js"), "x").unwrap();
    std::fs::write(site.join(".git/HEAD"), "ref").unwrap();
    let css = sha("body{}");
    let started = format!(
        r#"{{"upload_id":"up1","missing":[{{"path":"css/a.css","sha256":"{css}","put_url":"x"}}]}}"#
    );
    let (base, seen) = serve(vec![(200, started), (200, "{}".into()), (201, OK.into())]);
    let (code, out, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            site.to_str().unwrap(),
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "https://anyrouter.dev/a/abc123");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3, "{seen:?}");
    let manifest: serde_json::Value =
        serde_json::from_str(seen[0].splitn(5, ' ').nth(4).unwrap()).unwrap();
    let paths: Vec<&str> = manifest["manifest"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["path"].as_str().unwrap())
        .collect();
    // WHY: dotfiles and node_modules must never leave the machine.
    assert_eq!(paths, ["css/a.css", "index.html"]);
    assert_eq!(
        manifest["manifest"][1]["sha256"],
        sha("<link href=css/a.css>")
    );
    assert!(
        seen[1].starts_with(&format!(
            "PUT /api/v1/artifacts/uploads/up1/files/{css} Bearer sk-ar-test body{{}}"
        )),
        "{}",
        seen[1]
    );
    assert!(
        seen[2].starts_with("POST /api/v1/artifacts/uploads/up1/commit "),
        "{}",
        seen[2]
    );
}

#[test]
fn republish_reads_current_version_then_sends_base_version() {
    let dir = tmp("republish");
    std::fs::write(dir.join("a.html"), "<p>2</p>").unwrap();
    let (base, seen) = serve(vec![
        (200, r#"{"id":"abc123","current_version":3}"#.into()),
        (
            201,
            r#"{"id":"abc123","version":4,"url":"https://anyrouter.dev/a/abc123"}"#.into(),
        ),
    ]);
    let (code, out, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            dir.join("a.html").to_str().unwrap(),
            "--id",
            "abc123",
            "--label",
            "fix typo",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "https://anyrouter.dev/a/abc123");
    let seen = seen.lock().unwrap();
    assert!(
        seen[0].starts_with("GET /api/v1/artifacts/abc123 "),
        "{}",
        seen[0]
    );
    assert!(
        seen[1].starts_with("POST /api/v1/artifacts/abc123/versions "),
        "{}",
        seen[1]
    );
    let body: serde_json::Value =
        serde_json::from_str(seen[1].splitn(5, ' ').nth(4).unwrap()).unwrap();
    assert_eq!(body["base_version"], 3);
    assert_eq!(body["label"], "fix typo");
}

#[test]
fn version_conflict_prints_current_version_and_exits_2() {
    let dir = tmp("conflict");
    std::fs::write(dir.join("a.html"), "x").unwrap();
    let (base, _) = serve(vec![
        (200, r#"{"current_version":3}"#.into()),
        (409, r#"{"error":{"code":"version_conflict","message":"someone published first","current_version":4}}"#.into()),
    ]);
    let (code, out, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            dir.join("a.html").to_str().unwrap(),
            "--id",
            "abc123",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 2, "{err}");
    assert!(out.trim().is_empty(), "stdout must stay empty: {out}");
    assert!(err.contains("current version is 4"), "{err}");
}

#[test]
fn public_needs_yes_without_a_terminal_and_sends_nothing() {
    let dir = tmp("public");
    std::fs::write(dir.join("a.html"), "x").unwrap();
    let (base, seen) = serve(vec![(201, OK.into())]);
    let args = |extra: &[&'static str]| {
        let mut a = vec![
            "artifact".to_string(),
            "publish".into(),
            dir.join("a.html").to_str().unwrap().into(),
            "--visibility".into(),
            "public".into(),
            "--base-url".into(),
            base.clone(),
            "--key".into(),
            "sk-ar-test".into(),
        ];
        a.extend(extra.iter().map(|s| s.to_string()));
        a
    };
    let a = args(&[]);
    let (code, _, err) = anyr(&dir, &refs(&a));
    assert_eq!(code, 6, "{err}");
    assert!(err.contains("--yes"), "{err}");
    assert!(seen.lock().unwrap().is_empty());
    let a = args(&["--yes"]);
    let (code, _, err) = anyr(&dir, &refs(&a));
    assert_eq!(code, 0, "{err}");
    assert!(seen.lock().unwrap()[0].contains(r#""visibility":"public""#));
}

#[test]
fn missing_key_exits_4_without_a_request() {
    let dir = tmp("nokey");
    std::fs::write(dir.join("a.html"), "x").unwrap();
    let (base, seen) = serve(vec![(201, OK.into())]);
    let (code, out, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            dir.join("a.html").to_str().unwrap(),
            "--base-url",
            &base,
        ],
    );
    assert_eq!(code, 4, "{err}");
    assert!(out.is_empty());
    assert!(err.contains("ANYROUTER_API_KEY"), "{err}");
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn server_401_exits_4_and_404_exits_3() {
    let dir = tmp("status");
    let (base, _) = serve(vec![(401, r#"{"error":{"message":"bad key"}}"#.into())]);
    let (code, _, err) = anyr(
        &dir,
        &["artifact", "ls", "--base-url", &base, "--key", "sk-ar-bad"],
    );
    assert_eq!(code, 4, "{err}");
    assert!(err.contains("bad key"), "{err}");
    let (base, _) = serve(vec![(404, r#"{"error":{"message":"not found"}}"#.into())]);
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "rm",
            "nope",
            "--yes",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 3, "{err}");
}

#[test]
fn rm_confirms_unless_yes_and_sends_delete() {
    let dir = tmp("rm");
    let (base, seen) = serve(vec![(200, "{}".into())]);
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "rm",
            "abc123",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 6, "{err}");
    assert!(err.contains("--yes"), "{err}");
    assert!(seen.lock().unwrap().is_empty());
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "rm",
            "abc123",
            "--yes",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(seen.lock().unwrap()[0].starts_with("DELETE /api/v1/artifacts/abc123 "));
}

#[test]
fn visibility_patches_and_ls_lists() {
    let dir = tmp("vis");
    let (base, seen) = serve(vec![(200, "{}".into())]);
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "visibility",
            "abc123",
            "unlisted",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let s = seen.lock().unwrap()[0].clone();
    assert!(s.starts_with("PATCH /api/v1/artifacts/abc123 "), "{s}");
    assert!(s.ends_with(r#"{"visibility":"unlisted"}"#), "{s}");
    let (base, _) = serve(vec![(
        200,
        r#"{"data":[{"id":"abc123","title":"Sales","visibility":"link","current_version":2,"url":"https://anyrouter.dev/a/abc123"}]}"#.into(),
    )]);
    let (code, out, err) = anyr(
        &dir,
        &["artifact", "ls", "--base-url", &base, "--key", "sk-ar-test"],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out.trim(),
        "abc123\tSales\tlink\t2\thttps://anyrouter.dev/a/abc123"
    );
}

#[test]
fn bad_id_and_bad_visibility_are_usage_errors() {
    let dir = tmp("bad");
    let (code, _, err) = anyr(
        &dir,
        &["artifact", "rm", "../keys", "--yes", "--key", "sk-ar-test"],
    );
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "visibility",
            "abc",
            "world",
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 2, "{err}");
}

#[cfg(unix)]
#[test]
fn dir_publish_skips_symlinks_so_nothing_outside_the_folder_leaves() {
    // WHY: a symlink to ~/.ssh or /etc must never be followed into an upload.
    let dir = tmp("symlink");
    let site = dir.join("site");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("index.html"), "hi").unwrap();
    std::fs::write(dir.join("secret.txt"), "TOP SECRET").unwrap();
    std::os::unix::fs::symlink(dir.join("secret.txt"), site.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(&dir, site.join("up")).unwrap();
    let (base, seen) = serve(vec![
        (200, r#"{"upload_id":"u","missing":[]}"#.into()),
        (201, OK.into()),
    ]);
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            site.to_str().unwrap(),
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let seen = seen.lock().unwrap();
    assert!(
        !seen[0].contains("link.txt") && !seen[0].contains("secret"),
        "{}",
        seen[0]
    );
    assert!(seen[0].contains("index.html"), "{}", seen[0]);
}

// APFS rejects non-UTF-8 file names, so the fixture cannot be created there.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn dir_publish_rejects_non_utf8_file_names_locally() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tmp("nonutf8");
    let site = dir.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let bad = std::ffi::OsStr::from_bytes(b"bad\xff.txt");
    std::fs::write(site.join(bad), "x").unwrap();
    let (base, seen) = serve(vec![]);
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            site.to_str().unwrap(),
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("not valid UTF-8"), "{err}");
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn label_is_sent_on_a_first_publish() {
    let dir = tmp("label");
    let f = dir.join("a.html");
    std::fs::write(&f, "<p>x</p>").unwrap();
    let (base, seen) = serve(vec![(201, OK.into())]);
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            f.to_str().unwrap(),
            "--label",
            "v1 draft",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(seen.lock().unwrap()[0].contains(r#""label":"v1 draft""#));
}

#[test]
fn ls_rejects_a_bad_limit_before_any_request() {
    let dir = tmp("limit");
    let (base, seen) = serve(vec![]);
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "ls",
            "--limit",
            "5&x=1",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 2, "{err}");
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn server_413_is_reported_and_the_key_is_redacted() {
    // WHY: an error body that echoes the bearer token must not reach the terminal.
    let dir = tmp("413");
    let f = dir.join("a.html");
    std::fs::write(&f, "<p>x</p>").unwrap();
    let (base, _seen) = serve(vec![(
        413,
        r#"{"error":{"message":"too large for key sk-ar-test"}}"#.into(),
    )]);
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            f.to_str().unwrap(),
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("HTTP 413: too large"), "{err}");
    assert!(
        !err.contains("sk-ar-test\n") && err.contains("[redacted]"),
        "{err}"
    );
}

#[test]
fn private_publish_says_the_link_is_not_shareable_yet() {
    // WHY: the default is private, and a private link shows other people a
    // sign-in page. Without this line a user shares a link nobody can open.
    let dir = tmp("private-hint");
    std::fs::write(dir.join("p.html"), "<h1>Hi</h1>").unwrap();
    let (base, _seen) = serve(vec![(201, OK.into())]);
    let p = dir.join("p.html");
    let (code, out, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            p.to_str().unwrap(),
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "https://anyrouter.dev/a/abc123");
    assert!(err.contains("only you can open this link"), "{err}");
    assert!(err.contains("artifact visibility abc123 unlisted"), "{err}");

    let unlisted = r#"{"id":"abc123","version":1,"url":"https://anyrouter.dev/a/abc123","visibility":"unlisted"}"#;
    let (base, _seen) = serve(vec![(201, unlisted.into())]);
    let (code, _out, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            p.to_str().unwrap(),
            "--visibility",
            "unlisted",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(!err.contains("only you can open"), "{err}");
}

fn publish_body(dir: &Path, extra: &[&str]) -> serde_json::Value {
    std::fs::write(dir.join("p.html"), "<h1>Hi</h1>").unwrap();
    let (base, seen) = serve(vec![(201, OK.into())]);
    let file = dir.join("p.html");
    let mut args = vec!["artifact", "publish", file.to_str().unwrap()];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["--base-url", &base, "--key", "sk-ar-test"]);
    let (code, _, err) = anyr(dir, &args);
    assert_eq!(code, 0, "{err}");
    let s = seen.lock().unwrap()[0].clone();
    serde_json::from_str(s.splitn(5, ' ').nth(4).unwrap()).unwrap()
}

#[test]
fn show_author_is_sent_only_when_a_flag_asks_for_it() {
    let dir = tmp("author-pub");
    // WHY: an older server rejects unknown keys, so no flag means no field.
    assert!(publish_body(&dir, &[]).get("show_author").is_none());
    assert_eq!(publish_body(&dir, &["--show-author"])["show_author"], true);
    assert_eq!(
        publish_body(&dir, &["--no-show-author"])["show_author"],
        false
    );
}

#[test]
fn visibility_can_change_only_the_author_setting() {
    let dir = tmp("author-vis");
    let (base, seen) = serve(vec![(200, "{}".into()), (200, "{}".into())]);
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "visibility",
            "abc123",
            "--show-author",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("shows your name"), "{err}");
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "visibility",
            "abc123",
            "unlisted",
            "--no-show-author",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let seen = seen.lock().unwrap();
    assert!(seen[0].ends_with(r#"{"show_author":true}"#), "{}", seen[0]);
    assert!(
        seen[1].ends_with(r#"{"show_author":false,"visibility":"unlisted"}"#)
            || seen[1].ends_with(r#"{"visibility":"unlisted","show_author":false}"#),
        "{}",
        seen[1]
    );
}

#[test]
fn owner_data_is_sent_only_when_a_flag_asks_for_it() {
    let dir = tmp("ownerdata-pub");
    // WHY: an older server rejects unknown keys, so no flag means no field.
    assert!(publish_body(&dir, &[]).get("capabilities").is_none());
    assert_eq!(
        publish_body(&dir, &["--owner-data"])["capabilities"]["owner_data"],
        true
    );
    assert_eq!(
        publish_body(&dir, &["--no-owner-data"])["capabilities"]["owner_data"],
        false
    );
}

#[test]
fn owner_data_on_a_shared_page_warns_that_everyone_with_the_link_sees_usage() {
    let dir = tmp("ownerdata-warn");
    std::fs::write(dir.join("p.html"), "<h1>Hi</h1>").unwrap();
    let unlisted = r#"{"id":"abc123","version":1,"url":"https://anyrouter.dev/a/abc123","visibility":"unlisted","capabilities":{"owner_data":true}}"#;
    let (base, _seen) = serve(vec![(201, unlisted.into())]);
    let p = dir.join("p.html");
    let (code, out, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            p.to_str().unwrap(),
            "--visibility",
            "unlisted",
            "--owner-data",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "https://anyrouter.dev/a/abc123");
    assert!(
        err.contains("anyone with the link sees your usage"),
        "{err}"
    );

    // Private with owner data on: only the owner can open it, no warning.
    let (base, _seen) = serve(vec![(201, OK.into())]);
    let (code, _out, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            p.to_str().unwrap(),
            "--owner-data",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(!err.contains("anyone with the link"), "{err}");
}

#[test]
fn owner_data_inference_key_403_points_at_the_dashboard_and_exits_4() {
    let dir = tmp("ownerdata-403");
    std::fs::write(dir.join("p.html"), "<h1>Hi</h1>").unwrap();
    let (base, _seen) = serve(vec![
        (
            403,
            r#"{"error":{"code":"owner_data_requires_owner_session","message":"Enabling capabilities.owner_data needs your signed-in session"}}"#.into(),
        ),
    ]);
    let p = dir.join("p.html");
    let (code, _out, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            p.to_str().unwrap(),
            "--owner-data",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 4, "{err}");
    assert!(err.contains("owner_data_requires_owner_session"), "{err}");
    assert!(err.contains("artifact settings"), "{err}");
}

#[test]
fn visibility_can_toggle_owner_data_without_a_visibility_word() {
    let dir = tmp("ownerdata-vis");
    let (base, seen) = serve(vec![(200, "{}".into()), (200, "{}".into())]);
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "visibility",
            "abc123",
            "--owner-data",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("shows your usage"), "{err}");
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "visibility",
            "abc123",
            "--no-owner-data",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let seen = seen.lock().unwrap();
    assert!(
        seen[0].ends_with(r#"{"capabilities":{"owner_data":true}}"#),
        "{}",
        seen[0]
    );
    assert!(
        seen[1].ends_with(r#"{"capabilities":{"owner_data":false}}"#),
        "{}",
        seen[1]
    );
}

#[test]
fn both_owner_data_flags_together_are_a_usage_error() {
    let dir = tmp("ownerdata-bad");
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "visibility",
            "abc123",
            "--owner-data",
            "--no-owner-data",
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 2, "{err}");
}

#[test]
fn both_author_flags_or_neither_value_are_usage_errors() {
    let dir = tmp("author-bad");
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "visibility",
            "abc123",
            "--show-author",
            "--no-show-author",
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = anyr(
        &dir,
        &["artifact", "visibility", "abc123", "--key", "sk-ar-test"],
    );
    assert_eq!(code, 2, "{err}");
}

const PAGE_ROW: &str =
    r#"{"id":"ID","title":"T","visibility":"private","current_version":1,"url":"u"}"#;

fn page(ids: &[&str], next: Option<&str>) -> String {
    let rows: Vec<String> = ids.iter().map(|i| PAGE_ROW.replace("ID", i)).collect();
    let next = next.map_or("null".to_string(), |n| format!("\"{n}\""));
    format!(r#"{{"data":[{}],"next_cursor":{next}}}"#, rows.join(","))
}

#[test]
fn ls_all_follows_next_cursor_and_merges_pages() {
    let dir = tmp("lsall");
    let (base, seen) = serve(vec![
        (200, page(&["a1", "a2"], Some("c 1"))),
        (200, page(&["b1"], Some("c2"))),
        (200, page(&["c1"], None)),
    ]);
    let (code, out, err) = anyr(
        &dir,
        &[
            "artifact",
            "ls",
            "--all",
            "--limit",
            "2",
            "--json",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let ids: Vec<&str> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    // WHY: --all must not silently stop at the first page.
    assert_eq!(ids, ["a1", "a2", "b1", "c1"]);
    assert!(v["next_cursor"].is_null());
    let seen = seen.lock().unwrap();
    assert!(
        seen[0].starts_with("GET /api/v1/artifacts?limit=2 "),
        "{}",
        seen[0]
    );
    assert!(
        seen[1].starts_with("GET /api/v1/artifacts?limit=2&cursor=c%201 "),
        "{}",
        seen[1]
    );
    assert!(
        seen[2].starts_with("GET /api/v1/artifacts?limit=2&cursor=c2 "),
        "{}",
        seen[2]
    );
}

#[test]
fn ls_cursor_fetches_one_page_and_a_single_page_hints_the_next_cursor() {
    let dir = tmp("lscursor");
    let (base, seen) = serve(vec![(200, page(&["b1"], Some("c3")))]);
    let (code, out, err) = anyr(
        &dir,
        &[
            "artifact",
            "ls",
            "--cursor",
            "c2",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "b1\tT\tprivate\t1\tu");
    assert!(err.contains("--cursor c3"), "{err}");
    assert!(seen.lock().unwrap()[0].starts_with("GET /api/v1/artifacts?cursor=c2 "));
}

#[test]
fn ls_all_stops_on_a_repeated_cursor() {
    let dir = tmp("lsloop");
    let (base, seen) = serve(vec![
        (200, page(&["a1"], Some("same"))),
        (200, page(&["a2"], Some("same"))),
        (200, page(&["a3"], Some("same"))),
    ]);
    let (code, out, err) = anyr(
        &dir,
        &[
            "artifact",
            "ls",
            "--all",
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 1, "{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("kept returning cursors"), "{err}");
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[test]
fn ls_rejects_bad_paging_flags_before_any_request() {
    let dir = tmp("lsbad");
    let (base, seen) = serve(vec![]);
    for extra in [
        &["--limit", "101"][..],
        &["--limit", "0"],
        &["--all", "--cursor", "x"],
    ] {
        let mut args = vec!["artifact", "ls", "--base-url", &base, "--key", "sk-ar-test"];
        args.extend_from_slice(extra);
        let (code, _, err) = anyr(&dir, &args);
        assert_eq!(code, 2, "{extra:?}: {err}");
    }
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn file_changed_after_hashing_fails_clearly_and_commits_nothing() {
    let dir = tmp("changed");
    let site = dir.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let file = site.join("a.css");
    std::fs::write(&file, "body{}").unwrap();
    let started = format!(
        r#"{{"upload_id":"up1","missing":[{{"path":"a.css","sha256":"{}","put_url":"x"}}]}}"#,
        sha("body{}")
    );
    // The file is edited while the server answers the manifest request.
    let edited = file.clone();
    let (base, seen) = serve_hook(vec![(200, started)], move |i| {
        if i == 0 {
            std::fs::write(&edited, "body{color:red}").unwrap();
        }
    });
    let (code, out, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            site.to_str().unwrap(),
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    // WHY: uploading changed bytes under the old hash corrupts the artifact.
    assert_eq!(code, 1, "{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("a.css changed while publishing"), "{err}");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "no PUT and no commit: {seen:?}");
}

#[test]
fn rejected_blob_after_a_mid_upload_edit_reports_the_change() {
    let dir = tmp("rejected");
    let site = dir.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let file = site.join("a.css");
    std::fs::write(&file, "body{}").unwrap();
    let started = format!(
        r#"{{"upload_id":"up1","missing":[{{"path":"a.css","sha256":"{}","put_url":"x"}}]}}"#,
        sha("body{}")
    );
    // The edit lands while the server handles the PUT, then it rejects the blob.
    let edited = file.clone();
    let (base, _) = serve_hook(
        vec![
            (200, started),
            (400, r#"{"error":{"message":"sha256 mismatch"}}"#.into()),
        ],
        move |i| {
            if i == 1 {
                std::fs::write(&edited, "body{color:red}").unwrap();
            }
        },
    );
    let (code, _, err) = anyr(
        &dir,
        &[
            "artifact",
            "publish",
            site.to_str().unwrap(),
            "--base-url",
            &base,
            "--key",
            "sk-ar-test",
        ],
    );
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("a.css changed while publishing"), "{err}");
}
