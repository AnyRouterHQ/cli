//! `anyr presets` and `--preset` at launch against a local scripted server.

mod common;
use common::{serve, Seen, KEY};
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::Duration;

const MK: &str = "ak_testmanagement0123";
const CODING: &str = r#"{"id":"preset_1","object":"preset","slug":"coding","name":"Coding","description":null,"config":{"model":"a/one","models":["a/one","b/two"],"paid_fallback":true,"provider":{"sort":"price"},"system":"keep"},"created_at":"2026-10-24T00:00:00Z","updated_at":"2026-10-24T01:00:00Z"}"#;

fn home(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("anyr-presets-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run_with(base: &str, args: &[&str], envs: &[(&str, &str)], tag: &str) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("NO_COLOR", "1")
        .env("ANYROUTER_API_KEY", KEY)
        .env_remove("ANYROUTER_MANAGEMENT_KEY")
        .env("ANYROUTER_HOME", home(tag))
        .args(args)
        .args(["--base-url", base])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn managed(base: &str, args: &[&str], tag: &str) -> (i32, String, String) {
    run_with(base, args, &[("ANYROUTER_MANAGEMENT_KEY", MK)], tag)
}

fn next(rx: &Receiver<Seen>) -> Seen {
    rx.recv_timeout(Duration::from_secs(5)).unwrap()
}

#[test]
fn list_uses_the_management_key_and_prints_models_and_routing() {
    let (base, rx) = serve(vec![(
        200,
        format!(r#"{{"object":"list","data":[{CODING}]}}"#),
    )]);
    let (code, out, err) = managed(&base, &["presets"], "list");
    assert_eq!(code, 0, "{out}{err}");
    let seen = next(&rx);
    assert_eq!(
        (seen.method.as_str(), seen.path.as_str()),
        ("GET", "/api/v1/presets")
    );
    assert_eq!(seen.auth, format!("Bearer {MK}"));
    assert!(
        out.contains("coding") && out.contains("a/one, b/two"),
        "{out}"
    );
    assert!(out.contains("fallback, paid-fallback, sort=price"), "{out}");
}

#[test]
fn list_json_is_the_raw_body() {
    let (base, _rx) = serve(vec![(
        200,
        format!(r#"{{"object":"list","data":[{CODING}]}}"#),
    )]);
    let (code, out, _) = managed(&base, &["presets", "list", "--json"], "listjson");
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["data"][0]["slug"], "coding");
}

#[test]
fn show_gets_one_slug() {
    let (base, rx) = serve(vec![(200, CODING.into())]);
    let (code, out, err) = managed(&base, &["presets", "show", "coding"], "show");
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(next(&rx).path, "/api/v1/presets/coding");
    assert!(
        out.contains("a/one, b/two") && out.contains("sort=price"),
        "{out}"
    );
}

#[test]
fn create_posts_slug_name_and_config_from_flags() {
    let (base, rx) = serve(vec![(201, CODING.into())]);
    let (code, _, err) = managed(
        &base,
        &[
            "presets",
            "create",
            "coding",
            "--model",
            "a/one,b/two",
            "--fallback",
            "--sort",
            "price",
            "--name",
            "Coding",
        ],
        "create",
    );
    assert_eq!(code, 0, "{err}");
    let seen = next(&rx);
    assert_eq!(
        (seen.method.as_str(), seen.path.as_str()),
        ("POST", "/api/v1/presets")
    );
    let body: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(body["slug"], "coding");
    assert_eq!(body["name"], "Coding");
    assert_eq!(
        body["config"],
        serde_json::json!({"model":"a/one","models":["a/one","b/two"],"paid_fallback":true,"provider":{"sort":"price"}})
    );
}

#[test]
fn edit_reads_then_patches_only_the_asked_fields() {
    let (base, rx) = serve(vec![(200, CODING.into()), (200, CODING.into())]);
    let (code, _, err) = managed(
        &base,
        &[
            "presets",
            "edit",
            "coding",
            "--model",
            "a/one,b/two,c/three",
        ],
        "edit",
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(next(&rx).method, "GET");
    let patch = next(&rx);
    assert_eq!(
        (patch.method.as_str(), patch.path.as_str()),
        ("PATCH", "/api/v1/presets/coding")
    );
    let body: serde_json::Value = serde_json::from_str(&patch.body).unwrap();
    assert_eq!(
        body["config"]["models"],
        serde_json::json!(["a/one", "b/two", "c/three"])
    );
    assert_eq!(
        body["config"]["system"], "keep",
        "untouched fields survive the PATCH"
    );
    assert_eq!(body["config"]["provider"]["sort"], "price");
}

#[test]
fn rm_needs_yes_and_sends_nothing_without_it() {
    let (base, rx) = serve(vec![(200, r#"{"deleted":true,"slug":"coding"}"#.into())]);
    let (code, _, err) = managed(&base, &["presets", "rm", "coding"], "rmno");
    assert_ne!(code, 0);
    assert!(err.contains("--yes"), "{err}");
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "no request without --yes"
    );
    let (code, _, err) = managed(&base, &["presets", "rm", "coding", "--yes"], "rmyes");
    assert_eq!(code, 0, "{err}");
    let seen = next(&rx);
    assert_eq!(
        (seen.method.as_str(), seen.path.as_str()),
        ("DELETE", "/api/v1/presets/coding")
    );
}

#[test]
fn missing_management_key_says_how_to_get_one_and_sends_nothing() {
    let (base, rx) = serve(vec![(200, "{}".into())]);
    let (code, _, err) = run_with(&base, &["presets"], &[], "nokey");
    assert_eq!(
        code, 4,
        "a missing management key is an auth failure: {err}"
    );
    assert!(
        err.contains("management key") && err.contains("settings/management-keys"),
        "{err}"
    );
    assert!(err.contains("ANYROUTER_MANAGEMENT_KEY"), "{err}");
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn create_without_model_or_with_bad_slug_is_a_usage_error() {
    let (code, _, err) = managed(
        "http://127.0.0.1:1/api",
        &["presets", "create", "coding"],
        "u1",
    );
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = managed(
        "http://127.0.0.1:1/api",
        &["presets", "create", "Bad Slug", "--model", "a"],
        "u2",
    );
    assert_eq!(code, 2, "{err}");
}

/// A fake agent that records that it ran and the model it was given.
fn fake_agent(dir: &std::path::Path) -> (String, std::path::PathBuf) {
    let marker = dir.join("agent-ran");
    let script = dir.join("fake-agent.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$ANTHROPIC_MODEL\" > {}\n",
            marker.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    (script.display().to_string(), marker)
}

#[cfg(unix)]
#[test]
fn launch_with_a_missing_preset_fails_before_the_agent_starts() {
    let (base, rx) = serve(vec![(
        404,
        r#"{"error":{"message":"Preset not found"}}"#.into(),
    )]);
    let dir = home("launch404");
    let (agent, marker) = fake_agent(&dir);
    let _ = std::fs::remove_file(&marker);
    let (code, _, err) = managed(
        &base,
        &[
            "claude",
            "--preset",
            "nope",
            "--command-path",
            &agent,
            "--yes",
        ],
        "launch404",
    );
    assert_eq!(code, 3, "a missing preset is not found: {err}");
    assert!(
        err.contains("preset \"nope\" not found") && err.contains("presets create"),
        "{err}"
    );
    assert_eq!(next(&rx).path, "/api/v1/presets/nope");
    assert!(!marker.exists(), "agent must not run");
}

#[cfg(unix)]
#[test]
fn launch_with_an_existing_preset_runs_the_agent_on_the_preset_model() {
    let (base, rx) = serve(vec![(200, CODING.into())]);
    let dir = home("launchok");
    let (agent, marker) = fake_agent(&dir);
    let _ = std::fs::remove_file(&marker);
    let (code, _, err) = managed(
        &base,
        &[
            "claude",
            "--preset",
            "coding",
            "--command-path",
            &agent,
            "--yes",
        ],
        "launchok",
    );
    assert_eq!(code, 0, "{err}");
    let seen = next(&rx);
    assert_eq!(seen.path, "/api/v1/presets/coding");
    assert_eq!(seen.auth, format!("Bearer {MK}"));
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap().trim(),
        "@preset/coding"
    );
}

/// Most users hold only an inference key, which may not read /presets. The
/// gateway still resolves `@preset/<slug>` for that key, so the launch must go
/// on instead of demanding a management key.
#[cfg(unix)]
#[test]
fn launch_with_only_an_inference_key_runs_the_agent_when_presets_are_forbidden() {
    let (base, rx) = serve(vec![(
        401,
        r#"{"error":{"message":"Invalid or expired token"}}"#.into(),
    )]);
    let dir = home("launchapikey");
    let (agent, marker) = fake_agent(&dir);
    let _ = std::fs::remove_file(&marker);
    let (code, _, err) = run_with(
        &base,
        &[
            "claude",
            "--preset",
            "coding",
            "--command-path",
            &agent,
            "--yes",
        ],
        &[],
        "launchapikey",
    );
    assert_eq!(code, 0, "{err}");
    let seen = next(&rx);
    assert_eq!(seen.path, "/api/v1/presets/coding");
    assert_eq!(seen.auth, format!("Bearer {KEY}"));
    assert!(err.contains("could not check preset \"coding\""), "{err}");
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap().trim(),
        "@preset/coding"
    );
}

#[test]
fn preset_with_model_is_rejected_before_anything_runs() {
    let (code, _, err) = managed(
        "http://127.0.0.1:1/api",
        &[
            "claude",
            "--preset",
            "coding",
            "--model",
            "a/b",
            "--dry-run",
            "--yes",
        ],
        "combo",
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("cannot be combined"), "{err}");
}

#[test]
fn rm_if_exists_of_a_missing_preset_is_success_and_without_it_exit_3() {
    let (base, _rx) = serve(vec![(404, r#"{"error":{"message":"no preset"}}"#.into())]);
    let (code, out, err) = managed(
        &base,
        &["presets", "rm", "gone", "--yes", "--if-exists", "--json"],
        "rmgone",
    );
    assert_eq!(code, 0, "{out}{err}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["deleted"], false);
    let (base, _rx) = serve(vec![(404, r#"{"error":{"message":"no preset"}}"#.into())]);
    let (code, _, err) = managed(&base, &["presets", "rm", "gone", "--yes"], "rmgone3");
    assert_eq!(code, 3, "{err}");
}
