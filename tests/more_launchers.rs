//! `anyr qwen|goose|aider|crush|kilo`: each agent reaches AnyRouter with the
//! key from the environment only, on the model anyr picked, and the user's own
//! config files for that agent are never written.

mod common;
use common::{serve, KEY};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const MODEL: &str = "z-ai/glm-4.7-flash";

/// Reads the endpoint, key and model the way each real agent does (from the
/// variables and files anyr hands it), then POSTs one chat completion. Fails
/// when the key shows up in argv.
const FAKE_AGENT: &str = r#"#!/usr/bin/env python3
import json, os, sys, urllib.request, urllib.error
agent = os.path.basename(sys.argv[0]).removeprefix("fake-")
env = os.environ
if sys.argv[1:] == ["--version"]:
    print("0.0.14")
    raise SystemExit(0)
if any(env.get("ANYR_TEST_KEY", "-") in a for a in sys.argv):
    raise SystemExit("key in argv")
def arg(flag):
    return sys.argv[sys.argv.index(flag) + 1] if flag in sys.argv else None
if agent == "qwen":
    url, key, model = env["OPENAI_BASE_URL"] + "/chat/completions", env["OPENAI_API_KEY"], arg("-m")
elif agent == "goose":
    url = env["OPENAI_HOST"] + "/" + env["OPENAI_BASE_PATH"]
    key, model = env["OPENAI_API_KEY"], env["GOOSE_MODEL"]
    assert env["GOOSE_PROVIDER"] == "openai"
elif agent == "aider":
    url, key = env["AIDER_OPENAI_API_BASE"] + "/chat/completions", env["AIDER_OPENAI_API_KEY"]
    model = env["AIDER_MODEL"].removeprefix("openai/")
elif agent == "crush":
    cfg = json.load(open(os.path.join(env["CRUSH_GLOBAL_CONFIG"], "crush.json")))
    p = cfg["providers"]["anyrouter"]
    url, key = p["base_url"] + "/chat/completions", env[p["api_key"].removeprefix("$")]
    model = cfg["models"]["large"]["model"]
    assert env["CRUSH_GLOBAL_DATA"] != os.path.expanduser("~/.local/share/crush")
elif agent == "fx":
    cfg = json.load(open(os.path.join(env["HOME"], ".fx", "settings.json")))
    c = cfg["providers"][env["FX_PROVIDER"]]
    url, key, model = c["base_url"] + "/chat/completions", env[c["auth"]["env"]], env["FX_MODEL"]
elif agent == "kilo":
    cfg = json.loads(env["KILO_CONFIG_CONTENT"])
    o = cfg["provider"]["anyrouter"]["options"]
    url, key = o["baseURL"] + "/chat/completions", env[o["apiKey"][len("{env:"):-1]]
    model = arg("-m").removeprefix("anyrouter/")
body = json.dumps({"model": model}).encode()
req = urllib.request.Request(url, data=body, headers={"Authorization": "Bearer " + key, "Content-Type": "application/json"})
try:
    urllib.request.urlopen(req, timeout=5)
except urllib.error.HTTPError:
    pass
"#;

/// The user's own config for each agent, planted before the launch.
const USER_FILES: &[(&str, &str, &str)] = &[
    (
        "qwen",
        ".qwen/settings.json",
        r#"{"model":{"name":"qwen3-coder-plus"}}"#,
    ),
    (
        "goose",
        ".config/goose/config.yaml",
        "GOOSE_PROVIDER: anthropic\n",
    ),
    ("aider", ".aider.conf.yml", "model: sonnet\n"),
    (
        "crush",
        ".config/crush/crush.json",
        r#"{"options":{"debug":true},"providers":{"mine":{"type":"openai-compat","base_url":"https://example.invalid/v1"}}}"#,
    ),
    ("crush", ".config/crush/crushrc", "# the user's crushrc\n"),
    ("kilo", ".config/kilo/kilo.json", r#"{"theme":"mine"}"#),
];

fn temp_dir(tag: &str) -> PathBuf {
    common::unique_temp_dir(&format!("anyr-more-{tag}"))
}

fn plant_user_files(home: &Path, agent: &str) {
    for (a, rel, body) in USER_FILES.iter().filter(|(a, ..)| *a == agent) {
        let path = home.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap_or_else(|e| panic!("{a}: {e}"));
    }
}

fn anyr(dir: &Path, home: &Path, agent: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("NO_COLOR", "1")
        .env("HOME", home)
        .env("ANYROUTER_HOME", dir.join("anyr"))
        .env("ANYROUTER_API_KEY", KEY)
        .env("ANYR_TEST_KEY", KEY)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("CRUSH_GLOBAL_CONFIG")
        .args([agent, "--model", MODEL, "--yes", "--no-mcp"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    cmd
}

#[cfg(unix)]
#[test]
fn each_new_agent_sends_the_anyrouter_key_from_env_and_leaves_user_config_alone() {
    use std::os::unix::fs::PermissionsExt;
    for agent in ["qwen", "goose", "aider", "crush", "kilo"] {
        let dir = temp_dir(agent);
        let home = dir.join("home");
        plant_user_files(&home, agent);
        let fake = dir.join(format!("fake-{agent}"));
        std::fs::write(&fake, FAKE_AGENT).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (base, rx) = serve(vec![(401, r#"{"error":{"message":"x"}}"#.into())]);
        let out = anyr(&dir, &home, agent, &["--base-url", &base, "--command-path"])
            .arg(&fake)
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{agent}: {err}");
        let seen = rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("{agent} sent nothing: {err}"));
        assert_eq!(seen.path, "/api/v1/chat/completions", "{agent}");
        assert_eq!(seen.auth, format!("Bearer {KEY}"), "{agent}");
        assert!(seen.body.contains(MODEL), "{agent}: {}", seen.body);
        for (_, rel, body) in USER_FILES.iter().filter(|(a, ..)| *a == agent) {
            assert_eq!(
                std::fs::read_to_string(home.join(rel)).unwrap(),
                *body,
                "{agent} launch changed the user's {rel}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn crush_private_config_keeps_the_users_setup_and_holds_no_key() {
    let dir = temp_dir("crush-private");
    let home = dir.join("home");
    plant_user_files(&home, "crush");
    let out = anyr(
        &dir,
        &home,
        "crush",
        &["--dry-run", "--command-path", "/nonexistent/crush"],
    )
    .output()
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let private = dir.join("anyr").join("crush");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("CRUSH_GLOBAL_CONFIG={}", private.display())),
        "{stdout}"
    );
    let raw = std::fs::read_to_string(private.join("crush.json")).unwrap();
    assert!(
        !raw.contains(KEY),
        "crush.json must not hold the key:\n{raw}"
    );
    let cfg: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        cfg["providers"]["anyrouter"]["api_key"],
        "$ANYROUTER_API_KEY"
    );
    assert_eq!(cfg["models"]["large"]["model"], MODEL);
    assert_eq!(cfg["options"]["debug"], true, "user option dropped");
    assert!(
        cfg["providers"]["mine"].is_object(),
        "user provider dropped"
    );
    assert_eq!(
        std::fs::read_to_string(private.join("crushrc")).unwrap(),
        "# the user's crushrc\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `anyr <agent> --dry-run` env lines as a map.
fn dry_env(
    agent: &str,
    extra: &[&str],
) -> (Vec<String>, std::collections::BTreeMap<String, String>) {
    let dir = temp_dir(&format!("dry-{agent}"));
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let out = anyr(
        &dir,
        &home,
        agent,
        &[
            "--dry-run",
            "--base-url",
            "https://gw.example/api",
            "--command-path",
            "/nonexistent/agent",
        ],
    )
    .args(extra)
    .output()
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !stdout.contains(KEY),
        "{agent} dry run printed the key:\n{stdout}"
    );
    let args = stdout
        .lines()
        .find_map(|l| l.strip_prefix("args: "))
        .map(|j| serde_json::from_str(j).unwrap())
        .unwrap();
    let env = stdout
        .lines()
        .skip_while(|l| *l != "env:")
        .skip(1)
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    (args, env)
}

#[test]
fn dry_run_points_each_agent_at_anyrouter_with_the_chosen_model() {
    let (args, env) = dry_env("qwen", &[]);
    assert_eq!(env["OPENAI_BASE_URL"], "https://gw.example/api/v1");
    assert_eq!(env["OPENAI_MODEL"], MODEL);
    assert_eq!(
        args,
        ["-m", MODEL],
        "settings.json model.name beats OPENAI_MODEL"
    );

    let (_, env) = dry_env("goose", &[]);
    assert_eq!(env["OPENAI_HOST"], "https://gw.example");
    assert_eq!(env["OPENAI_BASE_PATH"], "api/v1/chat/completions");
    assert_eq!(env["GOOSE_PROVIDER"], "openai");
    assert_eq!(env["GOOSE_MODEL"], MODEL);

    let (_, env) = dry_env("aider", &[]);
    assert_eq!(env["AIDER_OPENAI_API_BASE"], "https://gw.example/api/v1");
    assert_eq!(env["AIDER_MODEL"], format!("openai/{MODEL}"));

    let (args, env) = dry_env("kilo", &[]);
    let cfg: serde_json::Value = serde_json::from_str(&env["KILO_CONFIG_CONTENT"]).unwrap();
    assert_eq!(cfg["model"], format!("anyrouter/{MODEL}"));
    assert_eq!(args, ["-m", &format!("anyrouter/{MODEL}")]);
    let options = &cfg["provider"]["anyrouter"]["options"];
    assert_eq!(options["baseURL"], "https://gw.example/api/v1");
    assert_eq!(options["apiKey"], "{env:ANYROUTER_API_KEY}");
}

#[test]
fn goose_yolo_sets_its_mode_instead_of_a_flag() {
    let (args, env) = dry_env("goose", &["--yolo", "run", "-t", "hi"]);
    assert_eq!(env["GOOSE_MODE"], "auto");
    assert_eq!(args, ["run", "-t", "hi"]);
    let (_, env) = dry_env("goose", &["run", "-t", "hi"]);
    assert!(!env.contains_key("GOOSE_MODE"), "no --yolo, no auto mode");
}

#[test]
fn missing_binary_names_the_install_command() {
    for (agent, install) in [
        ("qwen", "@qwen-code/qwen-code"),
        ("goose", "download_cli.sh"),
        ("aider", "aider.chat/install.sh"),
        ("crush", "@charmland/crush"),
        ("kilo", "@kilocode/cli"),
        ("fx", "fx.sh/setup.sh"),
    ] {
        let dir = temp_dir(&format!("missing-{agent}"));
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let out = anyr(
            &dir,
            &home,
            agent,
            &["--command-path", "no-such-agent-binary"],
        )
        .env("PATH", "/nonexistent")
        .output()
        .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{agent}");
        assert!(err.contains(install), "{agent}: {err}");
        assert!(
            !home.join(".fx").exists(),
            "{agent}: a failed launch created ~/.fx"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn kilo_keeps_a_model_the_user_passed() {
    let (args, _) = dry_env("kilo", &["--", "-m", "other/model"]);
    assert_eq!(args, ["-m", "other/model"], "anyr must not add a second -m");
}

#[test]
fn crush_data_dir_is_private_and_the_users_data_file_is_untouched() {
    let dir = temp_dir("crush-data");
    let home = dir.join("home");
    plant_user_files(&home, "crush");
    let user_data = home.join(".local/share/crush/crush.json");
    std::fs::create_dir_all(user_data.parent().unwrap()).unwrap();
    let planted = r#"{"models":{"large":{"provider":"mine","model":"my-model"}}}"#;
    std::fs::write(&user_data, planted).unwrap();
    let out = anyr(
        &dir,
        &home,
        "crush",
        &["--dry-run", "--command-path", "/nonexistent/crush"],
    )
    .env("CRUSH_GLOBAL_DATA", home.join(".local/share/crush"))
    .output()
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let private = dir.join("anyr").join("crush-data");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("CRUSH_GLOBAL_DATA={}", private.display())),
        "{stdout}"
    );
    assert!(private.is_dir());
    assert_eq!(std::fs::read_to_string(&user_data).unwrap(), planted);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn fx_gets_only_the_anyrouter_connection_in_its_settings() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("fx");
    let home = dir.join("home");
    let settings = home.join(".fx").join("settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        r#"{"provider":"gateway","theme":"dark","providers":{"local":{"protocol":"openai-chat-completions","base_url":"http://127.0.0.1:11434/v1","auth":{"type":"none"}}}}"#,
    )
    .unwrap();
    let before = std::fs::read_to_string(&settings).unwrap();
    let dry = anyr(
        &dir,
        &home,
        "fx",
        &["--dry-run", "--command-path", "/nonexistent/fx"],
    )
    .output()
    .unwrap();
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert!(String::from_utf8_lossy(&dry.stdout).contains("would set providers.anyrouter"));
    assert_eq!(
        std::fs::read_to_string(&settings).unwrap(),
        before,
        "--dry-run wrote"
    );

    let fake = dir.join("fake-fx");
    std::fs::write(&fake, FAKE_AGENT).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let launch = |what: &str| {
        let (base, rx) = serve(vec![(401, r#"{"error":{"message":"x"}}"#.into())]);
        let out = anyr(&dir, &home, "fx", &["--base-url", &base, "--command-path"])
            .arg(&fake)
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{what}: {err}");
        let seen = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(seen.auth, format!("Bearer {KEY}"), "{what}");
        assert!(seen.body.contains(MODEL), "{what}: {}", seen.body);
        (base, std::fs::read_to_string(&settings).unwrap())
    };
    let (base, first) = launch("first launch");
    assert!(!first.contains(KEY), "settings.json must not hold the key");
    let cfg: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(
        cfg["provider"], "gateway",
        "the user's own provider choice changed"
    );
    assert_eq!(cfg["theme"], "dark");
    assert!(
        cfg["providers"]["local"].is_object(),
        "user connection dropped"
    );
    let conn = &cfg["providers"]["anyrouter"];
    assert_eq!(conn["base_url"], format!("{base}/v1"));
    assert_eq!(conn["auth"]["env"], "ANYROUTER_API_KEY");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode(&settings),
        0o600,
        "fx ignores a settings.json others can read"
    );
    assert_eq!(mode(settings.parent().unwrap()), 0o700);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fx_connection_is_left_alone_when_already_current() {
    let conn = serde_json::json!({ "base_url": "https://gw.example/api/v1" });
    let once = anyr_cli::spawn::fx_settings_with(None, &conn)
        .unwrap()
        .expect("first write");
    assert_eq!(
        anyr_cli::spawn::fx_settings_with(Some(once), &conn),
        Ok(None)
    );
}

/// fx's `/model` picker lists only the ids in `model_metadata`, so every
/// AnyRouter model fx can drive (chat with tool calls) must be declared there,
/// not just the one the launch started on.
#[test]
fn fx_connection_lists_every_tool_capable_chat_model_for_the_model_picker() {
    let row = |id: &str, caps: &[&str], context: Option<i64>| anyr_cli::http::CatalogModel {
        id: id.into(),
        context_length: context,
        capabilities: caps.iter().map(|c| c.to_string()).collect(),
        ..Default::default()
    };
    let catalog = [
        row(
            "nvidia/nemotron-3.5-lightning-30b-a3b",
            &["chat", "function-calling"],
            Some(128_000),
        ),
        row(
            "minimax/minimax-m3.1-flash",
            &["chat", "function-calling", "vision"],
            Some(200_000),
        ),
        row("baai/bge-base-en-v1.5", &["embedding"], Some(512)),
        row("anyrouter/decision", &["systemone"], None),
        row("apple/foundation-model", &["chat", "streaming"], None),
        anyr_cli::http::CatalogModel {
            max_completion_tokens: Some(131_072),
            ..row(
                "meta/muse-glimmer-30b",
                &["chat", "function-calling"],
                Some(131_072),
            )
        },
    ];
    let conn = anyr_cli::spawn::fx_connection(
        "https://gw.example/api/v1",
        "ANYROUTER_API_KEY",
        "nvidia/nemotron-3.5-lightning-30b-a3b",
        &catalog,
    );
    let meta = conn["model_metadata"].as_object().unwrap();
    let mut ids: Vec<&str> = meta.keys().map(String::as_str).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "meta/muse-glimmer-30b",
            "minimax/minimax-m3.1-flash",
            "nvidia/nemotron-3.5-lightning-30b-a3b"
        ],
        "the picker must offer the second model and skip models fx cannot drive"
    );
    let minimax = &meta["minimax/minimax-m3.1-flash"];
    assert_eq!(minimax["context_window"], 200_000);
    assert_eq!(minimax["supports_tool_use"], true);
    assert_eq!(minimax["supports_vision"], true);
    assert!(meta["nvidia/nemotron-3.5-lightning-30b-a3b"]
        .get("supports_vision")
        .is_none());
    // fx sends `max_output_tokens` as `max_tokens` on every request, and the
    // catalog cap is not one every route accepts (a 400), nor one fx accepts
    // when it reaches the context window (fx refuses to start).
    for (id, m) in meta {
        assert!(m.get("max_output_tokens").is_none(), "{id}: {m}");
    }
    assert_eq!(meta["meta/muse-glimmer-30b"]["context_window"], 131_072);
}

/// fx exits `fx: LimitExceeded` on a connection with more than 256
/// `model_metadata` entries; the launch model must survive the cut even when
/// it sits past the cap in catalog order.
#[test]
fn fx_connection_stays_within_fxs_model_metadata_cap() {
    let max = anyr_cli::spawn::FX_MAX_MODEL_METADATA;
    let catalog: Vec<_> = (0..max + 54)
        .map(|i| anyr_cli::http::CatalogModel {
            id: format!("owner/model-{i}"),
            context_length: Some(8_000),
            capabilities: vec!["chat".into(), "function-calling".into()],
            ..Default::default()
        })
        .collect();
    let last = catalog.last().unwrap().id.clone();
    let conn = anyr_cli::spawn::fx_connection(
        "https://gw.example/api/v1",
        "ANYROUTER_API_KEY",
        &last,
        &catalog,
    );
    let meta = conn["model_metadata"].as_object().unwrap();
    assert_eq!(meta.len(), max);
    assert!(meta.contains_key(&last), "the launch model must stay listed");
}

/// A launch model the catalog does not list (a preset, an offline launch)
/// still has to be selectable.
#[test]
fn fx_connection_keeps_the_launch_model_without_a_catalog_row() {
    let conn = anyr_cli::spawn::fx_connection(
        "https://gw.example/api/v1",
        "ANYROUTER_API_KEY",
        "@preset/coding-stack",
        &[],
    );
    let meta = conn["model_metadata"].as_object().unwrap();
    assert_eq!(meta.len(), 1);
    assert_eq!(meta["@preset/coding-stack"]["supports_tool_use"], true);
}

#[cfg(unix)]
#[test]
fn fx_settings_symlink_is_written_through_not_replaced() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("fx-link");
    let home = dir.join("home");
    let dotfiles = dir.join("dotfiles").join("fx-settings.json");
    std::fs::create_dir_all(dotfiles.parent().unwrap()).unwrap();
    std::fs::write(&dotfiles, r#"{"theme":"dark"}"#).unwrap();
    std::fs::create_dir_all(home.join(".fx")).unwrap();
    let link = home.join(".fx").join("settings.json");
    std::os::unix::fs::symlink(&dotfiles, &link).unwrap();
    let fake = dir.join("fake-fx");
    std::fs::write(&fake, FAKE_AGENT).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (base, _rx) = serve(vec![(401, r#"{"error":{"message":"x"}}"#.into())]);
    let out = anyr(&dir, &home, "fx", &["--base-url", &base, "--command-path"])
        .arg(&fake)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&dotfiles).unwrap()).unwrap();
    assert_eq!(cfg["theme"], "dark");
    assert!(cfg["providers"]["anyrouter"].is_object());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A settings file the user already tuned: other connections, a selected
/// provider, top-level keys anyr does not know, and nested unknown fields.
const FX_USER_SETTINGS: &str = r#"{
  "provider": "local",
  "theme": "dark",
  "experimental": { "fancy": [1, 2, { "deep": true }] },
  "providers": {
    "local": {
      "protocol": "openai-chat-completions",
      "base_url": "http://127.0.0.1:11434/v1",
      "headers": { "x-team": "core" }
    },
    "work": { "protocol": "anthropic-messages", "unknown_flag": 7 }
  }
}"#;

fn fx_conn() -> serde_json::Value {
    anyr_cli::spawn::fx_connection(
        "https://gw.example/api/v1",
        "ANYROUTER_API_KEY",
        "anyrouter/free",
        &[],
    )
}

fn fx_settings_file(tag: &str, body: &[u8]) -> (PathBuf, PathBuf) {
    let dir = temp_dir(tag);
    let path = dir.join(".fx").join("settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    (dir, path)
}

#[cfg(unix)]
#[test]
fn fx_unreadable_settings_abort_and_are_left_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, path) = fx_settings_file("fx-unreadable", FX_USER_SETTINGS.as_bytes());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&path).is_ok() {
        eprintln!("skipped: this user can read a mode-000 file (root)");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    for dry_run in [true, false] {
        let err = anyr_cli::spawn::prepare_fx_settings(&path, &fx_conn(), dry_run)
            .expect_err("an unreadable settings.json must stop the launch");
        assert!(err.contains(&path.display().to_string()), "{err}");
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        FX_USER_SETTINGS,
        "the user's fx settings were overwritten"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fx_non_utf8_settings_abort_and_are_left_untouched() {
    let body = b"{\"theme\":\"\xff\xfe\"}";
    let (dir, path) = fx_settings_file("fx-non-utf8", body);
    let err = anyr_cli::spawn::prepare_fx_settings(&path, &fx_conn(), false)
        .expect_err("a settings.json that is not UTF-8 must stop the launch");
    assert!(err.contains(&path.display().to_string()), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), body);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fx_invalid_json_settings_abort_and_are_left_untouched() {
    for (tag, body) in [
        ("fx-bad-json", "{ \"theme\": \"dark\", nope"),
        ("fx-array-json", "[\"not\", \"an\", \"object\"]"),
        (
            "fx-bad-providers",
            "{ \"theme\": \"dark\", \"providers\": \"local\" }",
        ),
    ] {
        let (dir, path) = fx_settings_file(tag, body.as_bytes());
        let err = anyr_cli::spawn::prepare_fx_settings(&path, &fx_conn(), false).expect_err(tag);
        assert!(err.contains(&path.display().to_string()), "{tag}: {err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body, "{tag}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn fx_settings_keep_every_other_key_and_provider() {
    let (dir, path) = fx_settings_file("fx-preserve", FX_USER_SETTINGS.as_bytes());
    anyr_cli::spawn::prepare_fx_settings(&path, &fx_conn(), false).unwrap();
    let mut after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(after["providers"]["anyrouter"], fx_conn());
    after["providers"]
        .as_object_mut()
        .unwrap()
        .remove("anyrouter");
    let before: serde_json::Value = serde_json::from_str(FX_USER_SETTINGS).unwrap();
    assert_eq!(
        after, before,
        "a key other than providers.anyrouter changed"
    );
    let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["settings.json"], "temp file left behind");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn fx_missing_settings_are_created_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("fx-missing");
    let path = dir.join(".fx").join("settings.json");
    let line = anyr_cli::spawn::prepare_fx_settings(&path, &fx_conn(), false).unwrap();
    assert!(line.contains("set providers.anyrouter"), "{line}");
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        cfg,
        serde_json::json!({ "providers": { "anyrouter": fx_conn() } })
    );
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "fx ignores a settings.json others can read");
    let _ = std::fs::remove_dir_all(&dir);
}
