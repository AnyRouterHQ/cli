//! Live end-to-end tests against production (https://anyrouter.dev).
//!
//! They need `ANYROUTER_E2E_API_KEY` (an sk-ar-v1 inference key). Without it
//! each test prints why it skipped and passes, unless `ANYR_E2E_REQUIRED=1`
//! (set in CI for main and release PRs), where a missing key is a failure so
//! the job can never go green without having run.
//!
//! Run locally: `ANYROUTER_E2E_API_KEY=… cargo test --test e2e_live -- --test-threads=1`

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

fn key() -> Option<String> {
    let key = std::env::var("ANYROUTER_E2E_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty());
    if key.is_none() {
        assert!(
            std::env::var("ANYR_E2E_REQUIRED").as_deref() != Ok("1"),
            "ANYR_E2E_REQUIRED=1 but ANYROUTER_E2E_API_KEY is not set"
        );
        eprintln!("skipped: ANYROUTER_E2E_API_KEY is not set");
    }
    key
}

fn temp_home(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("anyr-live-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `anyr` against prod with the key in the environment (never argv).
fn anyr(home: &Path, key: &str, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .env("ANYR_NO_UPDATE", "1")
        .env("NO_COLOR", "1")
        .env("ANYROUTER_HOME", home)
        .env("ANYROUTER_API_KEY", key)
        .env_remove("ANYROUTER_MANAGEMENT_KEY")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawn anyr");
    let scrub = |b: &[u8]| String::from_utf8_lossy(b).replace(key, "<key>");
    (
        out.status.code().unwrap_or(-1),
        scrub(&out.stdout),
        scrub(&out.stderr),
    )
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}):\n{text}"))
}

#[test]
fn live_whoami_and_usage_accept_the_key() {
    let Some(key) = key() else { return };
    let home = temp_home("whoami");
    let (code, out, err) = anyr(&home, &key, &["usage", "--json"]);
    assert_eq!(code, 0, "usage failed: {out}{err}");
    assert!(json(&out).is_object(), "{out}");
}

#[test]
fn live_models_lists_the_catalog() {
    let Some(key) = key() else { return };
    let home = temp_home("models");
    let (code, out, err) = anyr(&home, &key, &["models", "--json"]);
    assert_eq!(code, 0, "{err}");
    let ids: Vec<String> = json(&out)["models"]
        .as_array()
        .expect("models array")
        .iter()
        .filter_map(|m| m["id"].as_str().map(String::from))
        .collect();
    assert!(ids.len() > 20, "catalog too small: {}", ids.len());
    assert!(
        ids.iter().any(|i| i == "anyrouter/auto"),
        "anyrouter/auto missing"
    );

    let (code, out, err) = anyr(&home, &key, &["models", "--free", "--json"]);
    assert_eq!(code, 0, "{err}");
    let free = json(&out)["data"].as_array().expect("data array").len();
    assert!(free > 0, "no free models listed");
}

#[test]
fn live_chat_answers_on_a_free_model() {
    let Some(key) = key() else { return };
    let home = temp_home("chat");
    // Free pools can be saturated for a moment: try the free router, then a
    // few listed free models, and pass on the first answer.
    let (_, out, _) = anyr(&home, &key, &["models", "--free", "--json"]);
    let mut candidates = vec!["anyrouter/free".to_string()];
    if let Ok(v) = serde_json::from_str::<Value>(&out) {
        candidates.extend(
            v["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["id"].as_str())
                .filter(|id| !id.starts_with("anyrouter/") && !id.starts_with("apple/"))
                .take(3)
                .map(String::from),
        );
    }
    let mut failures = Vec::new();
    for model in &candidates {
        let (code, out, err) = anyr(
            &home,
            &key,
            &["chat", "Reply with the single word: pong", "--model", model],
        );
        if code == 0 && !out.trim().is_empty() {
            return;
        }
        failures.push(format!("{model}: exit {code}: {}", err.trim()));
    }
    panic!("no free model answered:\n{}", failures.join("\n"));
}

/// Deletes the artifact even when an assertion fails.
struct ArtifactGuard<'a> {
    home: &'a Path,
    key: &'a str,
    id: String,
}

impl Drop for ArtifactGuard<'_> {
    fn drop(&mut self) {
        let _ = anyr(self.home, self.key, &["artifact", "rm", &self.id, "--yes"]);
    }
}

#[test]
fn live_artifact_publish_private_then_delete() {
    let Some(key) = key() else { return };
    let home = temp_home("artifact");
    let marker = format!(
        "anyr-e2e-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let file = home.join("index.html");
    std::fs::write(
        &file,
        format!("<!doctype html><title>anyr e2e</title><p>{marker}</p>"),
    )
    .unwrap();

    let (code, out, err) = anyr(
        &home,
        &key,
        &[
            "artifact",
            "publish",
            file.to_str().unwrap(),
            "--title",
            "anyr e2e (deleted by the test)",
            "--json",
        ],
    );
    assert_eq!(code, 0, "publish failed: {out}{err}");
    let published = json(&out);
    let id = published["id"].as_str().expect("id").to_string();
    let guard = ArtifactGuard {
        home: &home,
        key: &key,
        id: id.clone(),
    };
    assert_eq!(
        published["visibility"], "private",
        "default must be private"
    );
    assert!(published["url"].as_str().unwrap_or("").contains(&id));

    // It shows up in ls.
    let (code, out, err) = anyr(&home, &key, &["artifact", "ls", "--all", "--json"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(&id), "published artifact missing from ls");

    // Delete, then it is gone.
    let (code, out, err) = anyr(&home, &key, &["artifact", "rm", &id, "--yes", "--json"]);
    assert_eq!(code, 0, "rm failed: {out}{err}");
    assert_eq!(json(&out)["deleted"], true);
    let (code, _, _) = anyr(&home, &key, &["artifact", "rm", &id, "--yes"]);
    assert_eq!(code, 3, "a deleted artifact must be not found (exit 3)");
    std::mem::forget(guard);
}

/// Real coding agents, launched by `anyr`, answering through prod. The fakes
/// in tests/e2e.rs pin what anyr writes; this catches an agent release that
/// reads it differently (#173 Pi, #174 grok, #175 OpenCode).
///
/// `ANYR_E2E_AGENTS` lists `label=tool=path` entries separated by `;`, e.g.
/// `pi-old=pi=/opt/pi073/bin/pi;opencode=opencode=/usr/bin/opencode`. Unset
/// means skip; CI installs the agents and sets it.
#[test]
fn live_real_agents_answer_through_anyr() {
    let Some(key) = key() else { return };
    let Ok(list) = std::env::var("ANYR_E2E_AGENTS") else {
        eprintln!("skipped: ANYR_E2E_AGENTS is not set");
        return;
    };
    let prompt = "Reply with exactly the single word: pong";
    // Chosen from the live free list, never a fixed id: a model can move to
    // BYOK-only (poolside/laguna-s-2.1 did) and fail every agent at once.
    let (_, listed, _) = anyr(
        &temp_home("agent-models"),
        &key,
        &["models", "--free", "--json"],
    );
    let free: Vec<String> = serde_json::from_str::<Value>(&listed)
        .ok()
        .map(|v| {
            v["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["id"].as_str())
                .filter(|id| !id.starts_with("anyrouter/") && !id.starts_with("apple/"))
                .take(2)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    assert!(!free.is_empty(), "no free model listed:\n{listed}");
    let mut failures = Vec::new();
    for entry in list.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        let mut parts = entry.splitn(3, '=');
        let (label, tool, path) = match (parts.next(), parts.next(), parts.next()) {
            (Some(l), Some(t), Some(p)) => (l, t, p),
            _ => panic!("bad ANYR_E2E_AGENTS entry: {entry}"),
        };
        let tail: Vec<&str> = match tool {
            "pi" | "claude" | "grok" => vec!["--", "-p", prompt],
            "opencode" => vec!["--", "run", prompt],
            "codex" => vec!["--", "exec", "--skip-git-repo-check", prompt],
            other => panic!("no non-interactive form for {other}"),
        };
        let home = temp_home(&format!("agent-{label}"));
        let path_var = format!("ANYROUTER_{}_PATH", tool.to_ascii_uppercase());
        let mut last = String::new();
        let mut ok = false;
        // One retry on the next free model: a free upstream can drop a request.
        for model in free.iter().cycle().take(2) {
            let started = std::time::Instant::now();
            let out = run_with_deadline(
                Command::new(env!("CARGO_BIN_EXE_anyr"))
                    .env("ANYR_NO_UPDATE", "1")
                    .env("NO_COLOR", "1")
                    // A fresh HOME per agent: no saved logins or state from
                    // another version can change what it sends.
                    .env("HOME", &home)
                    .env("XDG_CONFIG_HOME", home.join(".config"))
                    .env("XDG_DATA_HOME", home.join(".local/share"))
                    .env("XDG_STATE_HOME", home.join(".local/state"))
                    .env("ANYROUTER_HOME", &home)
                    .env("ANYROUTER_API_KEY", &key)
                    .env(&path_var, path)
                    .env_remove("ANYROUTER_MANAGEMENT_KEY")
                    .args([tool, "--yes", "--model", model])
                    .args(&tail),
                std::time::Duration::from_secs(180),
            );
            eprintln!("{label}: {:?}", started.elapsed());
            let text = format!(
                "exit {:?}\nstdout: {}\nstderr: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
            .replace(&key, "<key>");
            if out.status.success()
                && String::from_utf8_lossy(&out.stdout)
                    .to_ascii_lowercase()
                    .contains("pong")
            {
                ok = true;
                break;
            }
            last = text;
        }
        if ok {
            eprintln!("{label}: answered");
        } else {
            failures.push(format!("{label} ({tool} at {path}):\n{last}"));
        }
    }
    assert!(
        failures.is_empty(),
        "agents failed:\n{}",
        failures.join("\n\n")
    );
}

/// Run `cmd` with stdin closed; kill it if it runs past `limit`, so a hung
/// agent fails the test instead of the CI job timing out.
fn run_with_deadline(cmd: &mut Command, limit: std::time::Duration) -> std::process::Output {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn anyr");
    let deadline = std::time::Instant::now() + limit;
    while child.try_wait().expect("wait").is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    child.wait_with_output().expect("collect output")
}
