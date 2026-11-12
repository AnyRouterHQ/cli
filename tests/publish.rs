//! `anyr publish`, `anyr apps` and `anyr api --data/-H` end to end against a
//! local HTTP server, so nothing touches anyrouter.dev.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;

const SCHEMA: &str = r#"{"app":"demo","version":"1","json_schema":{"type":"object","required":["title"],"additionalProperties":false,"properties":{"title":{"type":"string","minLength":2},"steps":{"type":"array","items":{"type":"integer"}}}}}"#;

fn cli(base: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("NO_COLOR", "1")
        .env(
            "ANYROUTER_HOME",
            std::env::temp_dir().join("anyr-publish-test-empty"),
        )
        .args(args)
        .args(["--base-url", base, "--key", "sk-ar-test"]);
    cmd
}

/// Answers `replies` in order, one connection each. Each request is reported
/// as "METHOD PATH\nheaders\nbody".
fn serve(replies: Vec<(u16, &'static str)>) -> (String, mpsc::Receiver<String>) {
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
            let mut headers = String::new();
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
                headers.push_str(&h.to_ascii_lowercase());
            }
            let mut b = vec![0u8; len];
            reader.read_exact(&mut b).unwrap();
            let mut p = line.split_whitespace();
            let summary = format!(
                "{} {}\n{}\n{}",
                p.next().unwrap_or(""),
                p.next().unwrap_or(""),
                headers,
                String::from_utf8_lossy(&b)
            );
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).unwrap();
            tx.send(summary).unwrap();
        }
    });
    (base, rx)
}

fn run_with_stdin(mut cmd: Command, input: &str) -> (i32, String, String) {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A usage error exits before reading stdin; the write then hits a closed pipe.
    if let Err(err) = child.stdin.take().unwrap().write_all(input.as_bytes()) {
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe, "{err}");
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn run(mut cmd: Command) -> (i32, String, String) {
    let out = cmd.stdin(Stdio::null()).output().unwrap();
    (
        out.status.code().unwrap_or(1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn idem(seen: &str) -> String {
    seen.lines()
        .find_map(|l| l.strip_prefix("idempotency-key:"))
        .unwrap_or("")
        .trim()
        .to_string()
}

const CREATED: &str = r#"{"id":"k3f9a2","url":"https://anyrouter.dev/apps/demo/s/k3f9a2","status":"accepted","visibility":"unlisted"}"#;

#[test]
fn publish_from_stdin_validates_then_posts_and_prints_the_url() {
    let (base, rx) = serve(vec![(200, SCHEMA), (201, CREATED)]);
    let cmd = cli(&base, &["publish", "demo", "-"]);
    let (code, out, err) = run_with_stdin(cmd, r#"{"title":"Lock","steps":[1,2]}"#);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "https://anyrouter.dev/apps/demo/s/k3f9a2");
    assert!(rx
        .recv()
        .unwrap()
        .starts_with("GET /api/v1/apps/demo/schema"));
    let post = rx.recv().unwrap();
    assert!(
        post.starts_with("POST /api/v1/apps/demo/submissions"),
        "{post}"
    );
    assert!(post.contains("authorization: bearer sk-ar-test"), "{post}");
    assert!(
        post.ends_with(r#"{"payload":{"steps":[1,2],"title":"Lock"}}"#),
        "{post}"
    );
}

#[test]
fn idempotency_key_is_the_same_for_the_same_document_in_any_key_order() {
    let mut keys = Vec::new();
    for doc in [
        r#"{"title":"Lock","steps":[1]}"#,
        r#"{"steps":[1],"title":"Lock"}"#,
        r#"{"title":"Other"}"#,
    ] {
        let (base, rx) = serve(vec![(200, SCHEMA), (201, CREATED)]);
        let cmd = cli(&base, &["publish", "demo", "-"]);
        let (code, _, err) = run_with_stdin(cmd, doc);
        assert_eq!(code, 0, "{err}");
        rx.recv().unwrap();
        keys.push(idem(&rx.recv().unwrap()));
    }
    // WHY: a retry or rerun must be deduplicated by the server, not duplicated.
    assert!(keys[0].starts_with("anyr-") && !keys[0].is_empty());
    assert_eq!(keys[0], keys[1]);
    assert_ne!(keys[0], keys[2]);
}

#[test]
fn invalid_document_exits_2_with_field_errors_and_never_uploads() {
    let (base, rx) = serve(vec![(200, SCHEMA)]);
    let cmd = cli(&base, &["publish", "demo", "-"]);
    let (code, out, err) = run_with_stdin(cmd, r#"{"title":"x","steps":["a"],"extra":1}"#);
    assert_eq!(code, 2, "{err}");
    assert!(out.is_empty());
    assert!(
        err.contains("$.title: needs at least 2 characters"),
        "{err}"
    );
    assert!(
        err.contains("$.steps[0]: expected integer, got string"),
        "{err}"
    );
    assert!(err.contains("$.extra: unknown field"), "{err}");
    rx.recv().unwrap();
    assert!(rx.try_recv().is_err(), "a POST was sent");
}

#[test]
fn non_json_input_is_a_usage_error() {
    let (base, _rx) = serve(vec![]);
    let cmd = cli(&base, &["publish", "demo", "-"]);
    let (code, _, err) = run_with_stdin(cmd, "not json");
    assert_eq!(code, 2);
    assert!(err.contains("not valid JSON"), "{err}");
}

#[test]
fn dry_run_asks_the_server_to_validate_and_does_not_upload() {
    let dir = std::env::temp_dir().join("anyr-publish-dry");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("doc.json");
    std::fs::write(&file, r#"{"title":"Lock"}"#).unwrap();
    let (base, rx) = serve(vec![(200, SCHEMA), (200, r#"{"valid":true}"#)]);
    let cmd = cli(
        &base,
        &["publish", "demo", file.to_str().unwrap(), "--dry-run"],
    );
    let (code, out, err) = run(cmd);
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("ok: valid against demo v1"), "{out}");
    rx.recv().unwrap();
    let check = rx.recv().unwrap();
    assert!(
        check.starts_with("POST /api/v1/apps/demo/submissions?validate_only=1"),
        "{check}"
    );
    assert!(
        check.ends_with(r#"{"payload":{"title":"Lock"}}"#),
        "{check}"
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn dry_run_prints_each_server_issue_with_its_path_and_fix() {
    let refused = r#"{"error":{"code":"invalid_payload","message":"Payload failed the demo checks (1 issue).","metadata":{"issues":[{"path":"steps[1]","code":"too_small","message":"step too small","fix":"Use at least 2."}]}}}"#;
    let (base, _rx) = serve(vec![(200, SCHEMA), (422, refused)]);
    let cmd = cli(&base, &["publish", "demo", "-", "--dry-run"]);
    let (code, out, err) = run_with_stdin(cmd, r#"{"title":"Lock","steps":[1,2]}"#);
    assert_ne!(code, 0, "{out}");
    assert!(err.contains("steps[1]: step too small"), "{err}");
    assert!(err.contains("fix: Use at least 2."), "{err}");
}

#[test]
fn unknown_app_exits_3() {
    let (base, _rx) = serve(vec![(404, r#"{"error":{"message":"unknown app"}}"#)]);
    let cmd = cli(&base, &["publish", "nope", "-"]);
    let (code, _, err) = run_with_stdin(cmd, "{}");
    assert_eq!(code, 3, "{err}");
    assert!(err.contains("unknown app"), "{err}");
}

#[test]
fn public_needs_yes_when_not_interactive_and_sends_visibility_with_it() {
    let (base, _rx) = serve(vec![(200, SCHEMA)]);
    let cmd = cli(&base, &["publish", "demo", "-", "--visibility", "public"]);
    let (code, _, err) = run_with_stdin(cmd, r#"{"title":"Lock"}"#);
    assert_eq!(code, 6, "{err}");
    assert!(err.contains("--yes"), "{err}");

    let (base, rx) = serve(vec![
        (200, SCHEMA),
        (
            201,
            r#"{"id":"a","url":"/apps/demo/s/a","status":"pending_review","visibility":"public"}"#,
        ),
    ]);
    let cmd = cli(
        &base,
        &["publish", "demo", "-", "--visibility", "public", "--yes"],
    );
    let (code, out, err) = run_with_stdin(cmd, r#"{"title":"Lock"}"#);
    assert_eq!(code, 0, "{err}");
    // relative URL from the server is made absolute against the site origin
    assert!(
        out.trim().ends_with("/apps/demo/s/a") && out.starts_with("http://"),
        "{out}"
    );
    assert!(err.contains("pending review"), "{err}");
    rx.recv().unwrap();
    assert!(rx.recv().unwrap().contains(r#""visibility":"public""#));
}

#[test]
fn bad_visibility_is_a_usage_error() {
    let (base, _rx) = serve(vec![]);
    let cmd = cli(&base, &["publish", "demo", "-", "--visibility", "everyone"]);
    let (code, _, err) = run_with_stdin(cmd, "{}");
    assert_eq!(code, 2);
    assert!(err.contains("visibility must be one of"), "{err}");
}

#[test]
fn apps_verbs_hit_the_generic_routes() {
    let (base, rx) = serve(vec![(200, SCHEMA)]);
    let cmd = cli(&base, &["apps", "schema", "demo"]);
    let (code, out, err) = run(cmd);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("\"json_schema\""), "{out}");
    assert!(rx
        .recv()
        .unwrap()
        .starts_with("GET /api/v1/apps/demo/schema"));

    let (base, rx) = serve(vec![(
        200,
        r#"{"data":[{"id":"k1","visibility":"unlisted","status":"accepted","title":"Lock","url":"u"}]}"#,
    )]);
    let cmd = cli(&base, &["apps", "ls", "demo"]);
    let (code, out, err) = run(cmd);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "k1\tunlisted\taccepted\tLock\tu");
    assert!(rx
        .recv()
        .unwrap()
        .starts_with("GET /api/v1/apps/demo/submissions?mine=1"));

    let (base, rx) = serve(vec![(200, r#"{"id":"k1","visibility":"private"}"#)]);
    let cmd = cli(&base, &["apps", "visibility", "demo", "k1", "private"]);
    let (code, _, err) = run(cmd);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("demo/k1 is now private: only you can open it"),
        "{err}"
    );
    let seen = rx.recv().unwrap();
    assert!(
        seen.starts_with("PATCH /api/v1/apps/demo/submissions/k1"),
        "{seen}"
    );
    assert!(seen.ends_with(r#"{"visibility":"private"}"#), "{seen}");

    let (base, rx) = serve(vec![(204, "")]);
    let cmd = cli(&base, &["apps", "rm", "demo", "k1", "--yes"]);
    let (code, _, err) = run(cmd);
    assert_eq!(code, 0, "{err}");
    assert!(rx
        .recv()
        .unwrap()
        .starts_with("DELETE /api/v1/apps/demo/submissions/k1"));
}

#[test]
fn apps_visibility_public_says_whether_it_waits_for_review_or_is_listed() {
    let (base, _rx) = serve(vec![(
        200,
        r#"{"id":"k1","visibility":"public","status":"pending_review"}"#,
    )]);
    let cmd = cli(&base, &["apps", "visibility", "demo", "k1", "public"]);
    let (code, _, err) = run(cmd);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("demo/k1 is now public: pending review before it is listed publicly"),
        "{err}"
    );

    // Already approved: going public again lists it with no second review.
    let (base, _rx) = serve(vec![(
        200,
        r#"{"id":"k1","visibility":"public","status":"accepted"}"#,
    )]);
    let cmd = cli(&base, &["apps", "visibility", "demo", "k1", "public"]);
    let (code, _, err) = run(cmd);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("demo/k1 is now public: listed publicly"),
        "{err}"
    );

    let (base, _rx) = serve(vec![(
        200,
        r#"{"id":"k1","visibility":"unlisted","status":"accepted"}"#,
    )]);
    let cmd = cli(&base, &["apps", "visibility", "demo", "k1", "unlisted"]);
    let (_, _, err) = run(cmd);
    assert!(
        err.contains("demo/k1 is now unlisted: anyone with the link can open it; not listed"),
        "{err}"
    );
}

#[test]
fn apps_rm_without_yes_refuses_when_not_interactive() {
    let (base, rx) = serve(vec![]);
    let cmd = cli(&base, &["apps", "rm", "demo", "k1"]);
    let (code, _, err) = run(cmd);
    assert_eq!(code, 6);
    assert!(err.contains("--yes"), "{err}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn api_data_reads_stdin_and_at_file_and_sends_headers() {
    let (base, rx) = serve(vec![(201, "{}")]);
    let cmd = cli(
        &base,
        &[
            "api",
            "POST",
            "/apps/demo/submissions",
            "--data",
            "-",
            "-H",
            "X-Trace: 7",
            "--idempotency-key",
            "abc",
        ],
    );
    let (code, _, err) = run_with_stdin(cmd, r#"{"payload":{"title":"Lock"}}"#);
    assert_eq!(code, 0, "{err}");
    let seen = rx.recv().unwrap();
    assert!(seen.contains("x-trace: 7"), "{seen}");
    assert!(seen.contains("idempotency-key: abc"), "{seen}");
    assert!(seen.ends_with(r#"{"payload":{"title":"Lock"}}"#), "{seen}");

    let file = std::env::temp_dir().join("anyr-api-data.json");
    std::fs::write(&file, r#"{"a":1}"#).unwrap();
    let (base, rx) = serve(vec![(200, "{}")]);
    let at = format!("@{}", file.display());
    let cmd = cli(&base, &["api", "POST", "/x", "--data", &at]);
    let (code, _, err) = run(cmd);
    assert_eq!(code, 0, "{err}");
    assert!(rx.recv().unwrap().ends_with(r#"{"a":1}"#));

    let (base, _rx) = serve(vec![]);
    let cmd = cli(
        &base,
        &["api", "POST", "/x", "--data", "@/nonexistent/file.json"],
    );
    let (code, _, err) = run(cmd);
    assert_ne!(code, 0);
    assert!(err.contains("cannot read /nonexistent/file.json"), "{err}");
}

#[test]
fn no_app_is_named_in_the_rust_source_outside_help_examples() {
    // WHY: apps are server-side data; a new app must need no CLI change.
    fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, hits);
            } else if p.extension().is_some_and(|x| x == "rs") {
                for (i, l) in std::fs::read_to_string(&p).unwrap().lines().enumerate() {
                    let is_example = l.contains("{bin}") || l.contains("$ <agent");
                    if l.to_ascii_lowercase().contains("explain") && !is_example {
                        hits.push(format!("{}:{}: {l}", p.display(), i + 1));
                    }
                }
            }
        }
    }
    let mut hits = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut hits,
    );
    assert!(hits.is_empty(), "{hits:#?}");
}

const UPLOAD_SCHEMA: &str = r#"{"app":"demo","version":"1","json_schema":{"type":"object","required":["title"]},"upload":{"max_bytes":64,"json_schema":{"type":"object","required":["format","content"],"additionalProperties":false,"properties":{"format":{"type":"string","enum":["kit-html","kit-svg"]},"content":{"type":"string","minLength":1},"content_light":{"type":"string","minLength":1}}}}}"#;

fn page_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("anyr-publish-page-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn html_page_uploads_with_its_light_sibling_in_the_apps_format() {
    // WHY: an agent's page (fig.html + fig-light.html) is not JSON; before this,
    // publish refused it as "not valid JSON" and the only route was hand-built jq.
    let dir = page_dir("sibling");
    let page = dir.join("fig.html");
    std::fs::write(&page, "<p>dark</p>").unwrap();
    std::fs::write(dir.join("fig-light.html"), "<p>light</p>").unwrap();
    let (base, rx) = serve(vec![(200, UPLOAD_SCHEMA), (201, CREATED)]);
    let (code, out, err) = run(cli(&base, &["publish", "demo", page.to_str().unwrap()]));
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "https://anyrouter.dev/apps/demo/s/k3f9a2");
    assert!(
        err.contains("fig-light.html"),
        "names the light build it sent: {err}"
    );
    rx.recv().unwrap();
    let post = rx.recv().unwrap();
    assert!(
        post.ends_with(
            r#"{"payload":{"content":"<p>dark</p>","content_light":"<p>light</p>","format":"kit-html"}}"#
        ),
        "{post}"
    );
    assert!(!idem(&post).is_empty(), "uploads stay idempotent: {post}");
}

#[test]
fn svg_picks_the_svg_format_and_light_flag_names_the_light_file() {
    let dir = page_dir("svg");
    let page = dir.join("fig.svg");
    let other = dir.join("paper.svg");
    std::fs::write(&page, "<svg/>").unwrap();
    std::fs::write(&other, "<svg id=l/>").unwrap();
    let (base, rx) = serve(vec![(200, UPLOAD_SCHEMA), (200, r#"{"valid":true}"#)]);
    let cmd = cli(
        &base,
        &[
            "publish",
            "demo",
            page.to_str().unwrap(),
            "--light",
            other.to_str().unwrap(),
            "--dry-run",
        ],
    );
    let (code, out, err) = run(cmd);
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("ok: valid against demo v1"), "{out}");
    rx.recv().unwrap();
    let check = rx.recv().unwrap();
    assert!(check.starts_with("POST /api/v1/apps/demo/submissions?validate_only=1"));
    assert!(
        check.ends_with(
            r#"{"payload":{"content":"<svg/>","content_light":"<svg id=l/>","format":"kit-svg"}}"#
        ),
        "{check}"
    );
}

#[test]
fn a_page_over_the_apps_upload_limit_is_refused_before_upload() {
    let dir = page_dir("big");
    let page = dir.join("fig.html");
    std::fs::write(&page, "x".repeat(65)).unwrap();
    let (base, rx) = serve(vec![(200, UPLOAD_SCHEMA)]);
    let (code, _, err) = run(cli(&base, &["publish", "demo", page.to_str().unwrap()]));
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("over the 0.1 KiB upload limit"), "{err}");
    rx.recv().unwrap();
    assert!(rx.try_recv().is_err(), "nothing uploaded");
}

#[test]
fn a_page_for_an_app_without_uploads_says_it_takes_json() {
    let dir = page_dir("noupload");
    let page = dir.join("fig.html");
    std::fs::write(&page, "<p>x</p>").unwrap();
    let (base, _rx) = serve(vec![(200, SCHEMA)]);
    let (code, _, err) = run(cli(&base, &["publish", "demo", page.to_str().unwrap()]));
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("takes JSON documents, not .html pages"),
        "{err}"
    );
}

#[test]
fn light_without_a_page_is_a_usage_error() {
    let (base, _rx) = serve(vec![]);
    let cmd = cli(&base, &["publish", "demo", "-", "--light", "x.html"]);
    let (code, _, err) = run_with_stdin(cmd, r#"{"title":"Lock"}"#);
    assert_eq!(code, 2);
    assert!(
        err.contains("--light goes with an .html or .svg page"),
        "{err}"
    );
}

#[test]
fn apps_rm_if_exists_treats_404_as_done_and_deletes_once_otherwise() {
    let (base, rx) = serve(vec![(404, r#"{"error":{"message":"gone"}}"#)]);
    let cmd = cli(
        &base,
        &["apps", "rm", "demo", "k1", "--yes", "--if-exists", "--json"],
    );
    let (code, out, err) = run(cmd);
    assert_eq!(code, 0, "{out}{err}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["deleted"], false);
    assert!(rx.recv().unwrap().starts_with("DELETE "));
    let (base, rx) = serve(vec![(200, "{}")]);
    let cmd = cli(&base, &["apps", "rm", "demo", "k1", "--yes", "--if-exists"]);
    let (code, _, err) = run(cmd);
    assert_eq!(code, 0, "{err}");
    assert!(rx.recv().unwrap().starts_with("DELETE "));
    assert!(rx.try_recv().is_err(), "one DELETE only");
}
