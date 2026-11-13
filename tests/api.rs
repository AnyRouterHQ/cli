//! `anyr api` and `anyr completion` end to end, against a one-shot local
//! HTTP server so nothing touches anyrouter.dev.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::mpsc;

fn anyr() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("NO_COLOR", "1")
        .env(
            "ANYROUTER_HOME",
            std::env::temp_dir().join("anyr-api-test-empty"),
        );
    cmd
}

/// Serve one request with `status`/`body`; returns base URL and a receiver
/// for "METHOD PATH AUTH BODY" of the request seen.
fn serve_once(status: u16, body: &'static str) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/api", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let mut auth = String::new();
        let mut len = 0usize;
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
        let summary = format!(
            "{} {} {} {}",
            parts.next().unwrap_or(""),
            parts.next().unwrap_or(""),
            auth,
            String::from_utf8_lossy(&req_body)
        );
        let resp = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        // Record the request before answering: once the client has its
        // reply it may exit, and the test may read `seen` at once.
        tx.send(summary).unwrap();
        stream.write_all(resp.as_bytes()).unwrap();
    });
    (base, rx)
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = anyr().args(args).output().expect("spawn anyr");
    (
        out.status.code().unwrap_or(1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn keys_list_pipes_tsv_with_bearer_key() {
    let (base, rx) = serve_once(
        200,
        r#"{"data":[{"hash":"h1","name":"ci","disabled":false}]}"#,
    );
    let (code, out, err) = run(&[
        "api",
        "keys",
        "list",
        "--base-url",
        &base,
        "--key",
        "sk-ar-test",
    ]);
    assert_eq!(code, 0, "{err}");
    // WHY: piped output must be machine-friendly (no header, tab separated).
    assert!(out.starts_with("h1\tci\t-\tfalse"), "{out:?}");
    let seen = rx.recv().unwrap();
    assert!(
        seen.starts_with("GET /api/v1/keys Bearer sk-ar-test"),
        "{seen}"
    );
}

#[test]
fn keys_create_prints_secret_only_on_stdout() {
    let (base, rx) = serve_once(201, r#"{"key":"sk-ar-v1-new","data":{"hash":"h2"}}"#);
    let (code, out, err) = run(&[
        "api",
        "keys",
        "create",
        "ci-bot",
        "--base-url",
        &base,
        "--key",
        "sk-ar-test",
    ]);
    assert_eq!(code, 0, "{err}");
    // WHY: `KEY=$(anyr api keys create x)` must capture just the secret.
    assert_eq!(out.trim(), "sk-ar-v1-new");
    assert!(err.contains("shown once"), "{err}");
    let seen = rx.recv().unwrap();
    assert!(
        seen.contains("POST /api/v1/keys") && seen.contains(r#""name":"ci-bot""#),
        "{seen}"
    );
}

#[test]
fn raw_patch_sends_typed_fields() {
    let (base, rx) = serve_once(200, r#"{"ok":true}"#);
    let (code, out, _) = run(&[
        "api",
        "PATCH",
        "/keys/h1",
        "disabled=true",
        "limit:=25",
        "--base-url",
        &base,
        "--key",
        "sk-ar-test",
    ]);
    assert_eq!(code, 0);
    assert!(out.contains("\"ok\":true"), "{out}");
    let seen = rx.recv().unwrap();
    assert!(seen.starts_with("PATCH /api/v1/keys/h1"), "{seen}");
    assert!(
        seen.contains(r#""disabled":true"#) && seen.contains(r#""limit":25"#),
        "{seen}"
    );
}

#[test]
fn unauthorized_exits_4_with_login_hint() {
    let (base, _rx) = serve_once(401, r#"{"error":{"message":"invalid key"}}"#);
    let (code, _, err) = run(&["api", "credits", "--base-url", &base, "--key", "sk-ar-bad"]);
    // WHY: curated verbs and raw calls share one exit table, so a script
    // branches on auth failure (4) the same way for both.
    assert_eq!(code, 4, "{err}");
    assert!(
        err.contains("invalid key") && err.contains("login"),
        "{err}"
    );

    let (base, _rx) = serve_once(401, r#"{"error":"nope"}"#);
    let (code, _, _) = run(&["api", "/credits", "--base-url", &base, "--key", "sk-ar-bad"]);
    // WHY: scripts branch on auth failure without parsing text.
    assert_eq!(code, 4);
}

#[test]
fn presets_without_management_key_explains_how_to_get_one() {
    let (code, _, err) = run(&["api", "presets", "list", "--key", "sk-ar-test"]);
    // A missing management key is an auth failure (exit 4), not a generic one.
    assert_eq!(code, 4, "{err}");
    assert!(err.contains("ANYROUTER_MANAGEMENT_KEY"), "{err}");
}

#[test]
fn revoke_refuses_without_yes_when_not_interactive() {
    let (code, _, err) = run(&["api", "keys", "revoke", "h1", "--key", "sk-ar-test"]);
    assert_eq!(code, 6, "confirm_required");
    assert!(err.contains("--yes"), "{err}");
}

#[test]
fn unknown_resource_lists_choices() {
    let (code, _, err) = run(&["api", "kees"]);
    // WHY: usage errors share exit 2 with top-level typos.
    assert_eq!(code, 2);
    assert!(err.contains("keys") && err.contains("credits"), "{err}");
}

#[test]
fn completion_scripts_and_engine() {
    for shell in ["bash", "zsh", "fish", "powershell"] {
        let (code, out, _) = run(&["completion", shell]);
        assert_eq!(code, 0, "{shell}");
        assert!(out.contains("__complete"), "{shell}: {out}");
    }
    let (code, _, _) = run(&["completion", "tcsh"]);
    assert_ne!(code, 0);

    let (code, out, err) = run(&["__complete", "claude", "--"]);
    assert_eq!(code, 0);
    assert!(err.is_empty(), "completion must never write stderr: {err}");
    assert!(out.lines().any(|l| l == "--yolo"), "{out}");

    // A dangling value flag must not error during completion.
    let (code, out, _) = run(&["__complete", "grok", "--effort", "h"]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "high");
}

#[test]
fn typo_suggests_command_and_exits_2() {
    let (code, _, err) = run(&["cluade"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("did you mean") && err.contains("claude"),
        "{err}"
    );
}

#[test]
fn api_help_is_examples_first() {
    let (code, out, _) = run(&["api"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("Examples") && out.contains("api keys create"),
        "{out}"
    );
}

const MODELS_BODY: &str = r#"{"data":[{"id":"anthropic/claude-sonnet-4.6","owned_by":"anthropic","context_length":200000},{"id":"openai/gpt-5.4-mini","owned_by":"openai","context_length":128000}]}"#;

#[test]
fn model_alias_list_and_ls_pipe_tsv_like_api() {
    // WHY: `anyr model list` failed with "Unknown command model"; the
    // singular form and list/ls verbs must reach the same listing, and
    // piped output must be bare TSV rows like `anyr api`.
    for argv in [
        ["model", "list"],
        ["model", "ls"],
        ["models", "list"],
        ["models", "ls"],
    ] {
        let (base, rx) = serve_once(200, MODELS_BODY);
        let (code, out, err) = run(&[argv[0], argv[1], "--base-url", &base]);
        assert_eq!(code, 0, "{argv:?}: {err}");
        assert!(rx.recv().unwrap().contains("/models"), "{argv:?}");
        assert_eq!(
            out,
            "anthropic/claude-sonnet-4.6\tanthropic\t200000\tfalse\tchat\nopenai/gpt-5.4-mini\topenai\t128000\tfalse\tchat\n",
            "{argv:?}"
        );
    }
}

#[test]
fn model_list_json_matches_models_json() {
    let (base, _rx) = serve_once(200, MODELS_BODY);
    let (code, out, err) = run(&["model", "list", "--json", "--base-url", &base]);
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("json");
    assert_eq!(v["models"][0]["id"], "anthropic/claude-sonnet-4.6");
}

#[test]
fn model_unknown_verb_is_usage_error_without_network() {
    // WHY: a typo like `model lsit` used to silently list; it is a usage
    // error (exit 2) and must not hit the network.
    let (code, out, err) = run(&["model", "lsit", "--base-url", "http://127.0.0.1:9/api"]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("Unknown models command"), "{err}");
}

#[test]
fn model_help_and_completion_resolve_to_models() {
    let (code, out, _) = run(&["model", "--help"]);
    assert_eq!(code, 0);
    assert!(out.contains("models — list catalog"), "{out}");
    let (_, out, _) = run(&["__complete", "model", ""]);
    for verb in ["list", "ls", "use"] {
        assert!(out.lines().any(|l| l.starts_with(verb)), "{verb}: {out}");
    }
    let (code, _, err) = run(&["modle"]);
    assert_eq!(code, 2);
    assert!(err.contains("did you mean"), "{err}");
}

#[test]
fn model_use_under_alias_persists_default() {
    // WHY: `model` must be a full alias, not just a listing shortcut.
    let home = std::env::temp_dir().join(format!("anyr-model-use-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.yaml"),
        "active_profile: default\nprofiles:\n  default:\n    api_key: sk-ar-v1-test\n    default_model: openai/gpt-5.4-mini\n",
    )
    .unwrap();
    let out = anyr()
        .env("ANYROUTER_HOME", &home)
        .args(["model", "use", "anyrouter/auto"])
        .output()
        .expect("model use");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // `anyrouter/auto` is the built-in default, so it is stored by dropping
    // the previous pin.
    assert!(String::from_utf8_lossy(&out.stdout).contains("anyrouter/auto"));
    let cfg = std::fs::read_to_string(home.join("config.yaml")).expect("config");
    assert!(!cfg.contains("openai/gpt-5.4-mini"), "{cfg}");
}

#[test]
fn html_error_body_is_summarised_not_dumped() {
    let html: &'static str = Box::leak(
        format!(
            "<!doctype html>\n<html><body>\n<h1>Not   Found</h1>{}</body></html>",
            "<p>filler</p>".repeat(200)
        )
        .into_boxed_str(),
    );
    let (base, _rx) = serve_once(404, html);
    let (code, _, err) = run(&[
        "api",
        "keys",
        "list",
        "--base-url",
        &base,
        "--key",
        "sk-ar-test",
    ]);
    assert_eq!(code, 3, "404 is not found: {err}");
    // WHY: a proxy/CDN HTML page must not flood the terminal or hide the status.
    assert!(
        err.contains("HTTP 404: Not Found: <!doctype html> <html>"),
        "{err}"
    );
    assert!(err.len() < 600, "{} bytes: {err}", err.len());
}

#[test]
fn env_key_alone_drives_auth_token_and_status_without_config() {
    // WHY: CI/headless use sets ANYROUTER_API_KEY and has no config file.
    let (code, out, err) = {
        let o = anyr()
            .args(["auth", "token", "--masked"])
            .env("ANYROUTER_API_KEY", "sk-ar-v1-abcdefghijklmnop")
            .output()
            .unwrap();
        (
            o.status.code().unwrap_or(1),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("sk-ar-v1-abcd"), "{out}");
    assert!(!out.contains("abcdefghijklmnop"), "{out}");

    let o = anyr()
        .args(["status", "--json", "--base-url", "http://127.0.0.1:1"])
        .env("ANYROUTER_API_KEY", "sk-ar-v1-abcdefghijklmnop")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(
        out.contains("\"base_url\": \"http://127.0.0.1:1\""),
        "{out}"
    );
}

#[test]
fn keys_list_accepts_env_key_and_key_flag_without_config() {
    for use_flag in [false, true] {
        let (base, rx) = serve_once(200, r#"{"data":[]}"#);
        let mut cmd = anyr();
        cmd.args(["keys", "list", "--base-url", &base]);
        if use_flag {
            cmd.args(["--key", "sk-ar-v1-flagkey"]);
        } else {
            cmd.env("ANYROUTER_API_KEY", "sk-ar-v1-flagkey");
        }
        let o = cmd.output().unwrap();
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(!err.contains("No AnyRouter config"), "{err}");
        assert!(!err.contains("unknown flag"), "{err}");
        let seen = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(seen.contains("Bearer sk-ar-v1-flagkey"), "{seen}");
    }
}

#[test]
fn config_get_json_reports_env_key_source() {
    let o = anyr()
        .args(["config", "get", "--json"])
        .env("ANYROUTER_API_KEY", "sk-ar-v1-abcdefghijklmnop")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    let v: serde_json::Value = serde_json::from_str(&out).expect(&out);
    // WHY: scripts must see the key that will actually be used, masked.
    assert_eq!(v["api_key_source"], "env");
    assert!(v["api_key"].as_str().unwrap().starts_with("sk-ar-v1-abcd"));
    assert!(!out.contains("abcdefghijklmnop"), "{out}");

    let o = anyr()
        .args(["config", "get", "--json"])
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["api_key_source"], "none");
    assert!(v["api_key"].is_null());
}

#[test]
fn restricted_key_403_names_the_missing_scope_and_exits_4() {
    // WHY: a restricted sk-ar key used to get a bare "HTTP 403"; the user
    // must learn which scope is missing and how to fix it.
    let (base, _rx) = serve_once(403, r#"{"error":{"message":"not permitted"}}"#);
    let (code, _, err) = run(&["usage", "--base-url", &base, "--key", "sk-ar-restricted"]);
    assert_eq!(code, 4, "{err}");
    assert!(err.lines().any(|l| l.starts_with("error:")), "{err:?}");
    assert!(err.contains("lacks scope `credits`"), "{err}");
}

#[test]
fn command_failures_share_one_error_prefix_on_stderr() {
    // WHY: `usage` used to print a bare sentence while `api` printed `error:`.
    let (base, _rx) = serve_once(500, "{}");
    let (code, out, err) = run(&["usage", "--base-url", &base, "--key", "sk-ar-test"]);
    assert_eq!(code, 1);
    assert!(out.is_empty(), "{out:?}");
    assert!(err.lines().any(|l| l.starts_with("error:")), "{err:?}");
}

const CHAT_SSE: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\ndata: [DONE]\n\n";

#[test]
fn chat_one_shot_streams_the_reply_to_stdout() {
    // WHY: `anyr chat` was a stub; a prompt must hit /chat/completions with
    // stream=true and print only the model text on stdout.
    let (base, rx) = serve_once(200, CHAT_SSE);
    let (code, out, err) = run(&[
        "chat",
        "say",
        "hi",
        "--model",
        "z-ai/glm-4.7-flash",
        "--system",
        "be brief",
        "--base-url",
        &base,
        "--key",
        "sk-ar-test",
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "Hello\n");
    let seen = rx.recv().unwrap();
    assert!(
        seen.starts_with("POST /api/v1/chat/completions Bearer sk-ar-test"),
        "{seen}"
    );
    let body: serde_json::Value =
        serde_json::from_str(seen.splitn(5, ' ').nth(4).unwrap()).unwrap();
    assert_eq!(body["stream"], true);
    assert_eq!(body["model"], "z-ai/glm-4.7-flash");
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["content"], "say hi");
}

#[test]
fn chat_server_error_prints_the_message_and_fails() {
    let (base, _rx) = serve_once(402, r#"{"error":{"message":"no credits"}}"#);
    let (code, out, err) = run(&["chat", "hi", "--base-url", &base, "--key", "sk-ar-test"]);
    assert_eq!(code, 8, "HTTP 402 is payment_required");
    assert!(out.is_empty(), "{out:?}");
    assert!(err.contains("error: HTTP 402: no credits"), "{err}");
}
