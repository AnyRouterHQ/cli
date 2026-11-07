//! Hermetic e2e for MCP auto-connect: the built `anyr` launches
//! tests/fake-mcp-agent.py in place of each agent, against a local gateway
//! that serves `/api/v1/mcp/config` and a tiny MCP server. The fake agent
//! reads the MCP config the way the real agent does and lists the tools, so
//! these tests assert what reaches the server, not what anyr prints.
#![cfg(unix)]

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

const KEY: &str = "sk-ar-v1-mcpautotestkey0123456789";
const CATALOG: &str = r#"{"object":"list","data":[{"id":"z-ai/glm-4.7","owned_by":"z-ai","name":"GLM 4.7","category":"text","capabilities":["chat"],"created":100,"context_length":204800}]}"#;

#[derive(Clone, Debug)]
struct Req {
    method: String,
    path: String,
    auth: String,
}

/// How the gateway answers `GET /api/v1/mcp/config`.
#[derive(Clone, Copy)]
enum Mode {
    /// `true`: full auto-connect. `false`: the owner turned it off explicitly,
    /// so the artifact tools are off too.
    Enabled(bool),
    /// Full auto-connect off by default; the artifact tools stay on.
    ArtifactsOnly,
    /// A gateway that predates `artifacts` in `/mcp/config`, with auto-connect off.
    LegacyOff,
    /// Answers after the CLI's 2.5 s timeout.
    Hang,
    ServerError,
    NotFound,
}

struct Gateway {
    base: String,
    seen: Arc<Mutex<Vec<Req>>>,
}

impl Gateway {
    /// `enabled`: what `GET /api/v1/mcp/config` answers for this workspace.
    fn start(enabled: bool) -> Self {
        Self::start_mode(Mode::Enabled(enabled))
    }

    fn start_mode(mode: Mode) -> Self {
        let enabled = matches!(mode, Mode::Enabled(true));
        let artifacts = match mode {
            Mode::LegacyOff => None,
            Mode::ArtifactsOnly | Mode::Enabled(true) => Some(true),
            _ => Some(false),
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/api", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Req>>> = Arc::default();
        let seen_in = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let seen = Arc::clone(&seen_in);
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let (mut auth, mut len) = (String::new(), 0usize);
                    loop {
                        let mut h = String::new();
                        if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
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
                    let mut body = vec![0u8; len];
                    let _ = reader.read_exact(&mut body);
                    let mut parts = line.split_whitespace();
                    let req = Req {
                        method: parts.next().unwrap_or("").into(),
                        path: parts.next().unwrap_or("").into(),
                        auth,
                    };
                    seen.lock().unwrap().push(req.clone());
                    let is_config = req.path.split('?').next() == Some("/api/v1/mcp/config");
                    if is_config && matches!(mode, Mode::Hang) {
                        std::thread::sleep(std::time::Duration::from_millis(4000));
                    }
                    let (status, out) = match mode {
                        Mode::ServerError if is_config => (500, r#"{"error":"boom"}"#.into()),
                        Mode::NotFound if is_config => (404, r#"{"error":"nope"}"#.into()),
                        _ => route(&req, &body, enabled, artifacts),
                    };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                        out.len()
                    );
                });
            }
        });
        Gateway { base, seen }
    }

    fn hits(&self, method: &str, path: &str) -> Vec<Req> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == method && r.path.split('?').next() == Some(path))
            .cloned()
            .collect()
    }
}

const ARTIFACT_TOOLS: &str = "publish_artifact,list_artifacts,get_artifact";

fn route(req: &Req, body: &[u8], enabled: bool, artifacts: Option<bool>) -> (u16, String) {
    let (path, query) = req.path.split_once('?').unwrap_or((&req.path, ""));
    let authed = req.auth == format!("Bearer {KEY}");
    match (req.method.as_str(), path) {
        ("GET", "/api/v1/models") => (200, CATALOG.into()),
        ("GET", "/api/v1/credits") => (
            200,
            r#"{"data":{"total_credits":1,"total_usage":0}}"#.into(),
        ),
        ("GET", "/api/v1/mcp/config") if authed => {
            let mut v = json!({"enabled": enabled, "setting": null, "default": enabled,
                   "url": "ignored-by-the-cli", "can_edit": false});
            if let Some(on) = artifacts {
                v["artifacts"] = json!({"enabled": on, "url": "ignored-by-the-cli"});
            }
            (200, v.to_string())
        }
        ("POST", "/api/v1/mcp") if authed => {
            let rpc: Value = serde_json::from_slice(body).unwrap_or_default();
            let result = match rpc["method"].as_str() {
                Some("tools/list") if query == "tools=artifacts" => json!({"tools":
                    ARTIFACT_TOOLS.split(',').map(|n| json!({"name": n})).collect::<Vec<_>>()}),
                Some("tools/list") => {
                    json!({"tools": [{"name": "list_models"}, {"name": "get_system_status"}]})
                }
                _ => json!({"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}}),
            };
            (
                200,
                json!({"jsonrpc": "2.0", "id": rpc["id"], "result": result}).to_string(),
            )
        }
        (_, p) if p.starts_with("/api/v1/mcp") => (401, r#"{"error":"unauthorized"}"#.into()),
        _ => (404, r#"{"error":{"message":"not found"}}"#.into()),
    }
}

fn temp_home(tag: &str) -> PathBuf {
    common::unique_temp_dir(&format!("anyr-mcpauto-{tag}"))
}

fn anyr(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env("NO_COLOR", "1")
        .env("ANYROUTER_HOME", home)
        .env("HOME", home)
        .env("GROK_HOME", home.join("grok"))
        .env_remove("ANYROUTER_API_KEY")
        .env_remove("ANYROUTER_MANAGEMENT_KEY")
        .env_remove("ANYROUTER_PROFILE")
        .env_remove("OPENCODE_CONFIG_CONTENT")
        .stdin(Stdio::null());
    cmd
}

fn run(cmd: &mut Command) -> (i32, String, String) {
    let out = cmd.output().expect("spawn anyr");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn login(home: &Path, base: &str) {
    let mut child = anyr(home)
        .args(["login", "--key", "-", "--base-url", base])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{KEY}\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "login failed: {out:?}");
}

/// `anyr <agent> [extra]` with the fake agent; returns its stdout.
fn launch(gw: &Gateway, home: &Path, agent: &str, extra: &[&str]) -> String {
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-mcp-agent.py");
    let (code, out, err) = run(anyr(home)
        .args([agent, "--yes", "--model", "z-ai/glm-4.7"])
        .args(extra)
        .env(
            format!("ANYROUTER_{}_PATH", agent.to_ascii_uppercase()),
            &fake,
        )
        .env("FAKE_AGENT", agent));
    assert_eq!(code, 0, "anyr {agent} failed:\nstdout:{out}\nstderr:{err}");
    assert!(
        !out.contains(KEY) && !err.contains(KEY),
        "{agent} printed the key"
    );
    let _ = gw;
    out
}

#[test]
fn every_auto_agent_lists_the_gateway_tools_with_the_launch_key() {
    for agent in ["claude", "codex", "opencode", "grok", "kilo", "crush"] {
        let gw = Gateway::start(true);
        let home = temp_home(agent);
        login(&home, &gw.base);
        let out = launch(&gw, &home, agent, &[]);
        assert!(
            out.contains("TOOLS list_models,get_system_status"),
            "{agent} did not reach the MCP tools:\n{out}"
        );
        let mcp = gw.hits("POST", "/api/v1/mcp");
        assert_eq!(mcp.len(), 2, "{agent}: {mcp:?}");
        assert!(mcp.iter().all(|r| r.auth == format!("Bearer {KEY}")));
    }
}

#[test]
fn a_workspace_with_auto_connect_off_gets_nothing() {
    for agent in ["claude", "codex", "opencode", "grok", "kilo", "crush"] {
        let gw = Gateway::start(false);
        let home = temp_home(&format!("{agent}-off"));
        login(&home, &gw.base);
        assert_eq!(launch(&gw, &home, agent, &[]).trim(), "NO_MCP", "{agent}");
        assert_eq!(gw.hits("GET", "/api/v1/mcp/config").len(), 1, "{agent}");
    }
}

#[test]
fn no_mcp_and_mcp_auto_off_skip_the_lookup_entirely() {
    let gw = Gateway::start(true);
    let home = temp_home("optout");
    login(&home, &gw.base);
    assert_eq!(launch(&gw, &home, "claude", &["--no-mcp"]).trim(), "NO_MCP");
    assert!(gw.hits("GET", "/api/v1/mcp/config").is_empty());

    let (code, out, _) = run(anyr(&home).args(["mcp", "auto", "off"]));
    assert_eq!(code, 0);
    assert!(out.contains("off"), "{out}");
    assert_eq!(launch(&gw, &home, "codex", &[]).trim(), "NO_MCP");
    assert!(gw.hits("GET", "/api/v1/mcp/config").is_empty());

    run(anyr(&home).args(["mcp", "auto", "on"]));
    assert!(launch(&gw, &home, "codex", &[]).contains("TOOLS"));
}

#[test]
fn grok_entry_follows_the_setting_and_mcp_remove_cleans_it() {
    let gw = Gateway::start(true);
    let home = temp_home("grok-cycle");
    login(&home, &gw.base);
    let grok_config = home.join("grok").join("config.toml");
    assert!(launch(&gw, &home, "grok", &[]).contains("TOOLS"));
    let text = std::fs::read_to_string(&grok_config).unwrap();
    assert!(
        text.contains("# Managed by anyr") && !text.contains(KEY),
        "{text}"
    );
    // A launch passes the key by env: it writes no key file.
    assert!(!home.join("mcp").join("grok-token").exists());

    // Turning it off on the next launch takes anyr's entry back out.
    assert_eq!(launch(&gw, &home, "grok", &["--no-mcp"]).trim(), "NO_MCP");
    assert!(!std::fs::read_to_string(&grok_config)
        .unwrap()
        .contains("anyrouter"));

    run(anyr(&home).args(["mcp", "add", "grok"]));
    assert!(std::fs::read_to_string(&grok_config)
        .unwrap()
        .contains("[mcp_servers.anyrouter]"));
    let (code, out, _) = run(anyr(&home).args(["mcp", "remove", "grok"]));
    assert_eq!(code, 0, "{out}");
    assert!(!std::fs::read_to_string(&grok_config)
        .unwrap()
        .contains("anyrouter"));
}

#[test]
fn a_launch_dropping_the_grok_entry_keeps_the_key_of_an_explicit_mcp_add() {
    let gw = Gateway::start(true);
    let home = temp_home("grok-token");
    login(&home, &gw.base);
    let token = home.join("mcp").join("grok-token");
    let (code, out, _) = run(anyr(&home).args(["mcp", "add", "grok"]));
    assert_eq!(code, 0, "{out}");
    assert_eq!(std::fs::read_to_string(&token).unwrap().trim(), KEY);
    assert!(out.contains("owner-only file"), "{out}");

    assert_eq!(launch(&gw, &home, "grok", &["--no-mcp"]).trim(), "NO_MCP");
    assert!(token.exists(), "--no-mcp deleted the key of `mcp add grok`");

    run(anyr(&home).args(["mcp", "auto", "off"]));
    assert_eq!(launch(&gw, &home, "grok", &[]).trim(), "NO_MCP");
    assert!(
        token.exists(),
        "mcp auto off deleted the key of `mcp add grok`"
    );

    let (code, out, _) = run(anyr(&home).args(["mcp", "remove", "grok"]));
    assert_eq!(code, 0, "{out}");
    assert!(!token.exists(), "mcp remove grok leaves the key behind");
}

#[test]
fn dry_run_never_asks_the_gateway_and_shows_the_wiring_from_a_warm_cache() {
    let gw = Gateway::start(true);
    let home = temp_home("dry");
    login(&home, &gw.base);
    let cache = home.join("mcp-config-cache.json");

    let (code, out, err) = run(anyr(&home).args(["claude", "--yes", "--dry-run"]));
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("mcp: not checked"), "{out}");
    assert!(!out.contains("--mcp-config="), "{out}");
    assert!(gw.hits("GET", "/api/v1/mcp/config").is_empty());
    assert!(!cache.exists(), "--dry-run wrote the cache");

    // A real launch warms the cache; the dry run then reads it, without a request.
    assert!(launch(&gw, &home, "claude", &[]).contains("TOOLS"));
    assert_eq!(gw.hits("GET", "/api/v1/mcp/config").len(), 1);
    let (code, out, err) = run(anyr(&home).args(["claude", "--yes", "--dry-run"]));
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("--mcp-config="), "{out}");
    assert!(out.contains("mcp: anyrouter http://127.0.0.1"), "{out}");
    assert!(!out.contains(KEY), "dry-run printed the key:\n{out}");
    assert_eq!(gw.hits("GET", "/api/v1/mcp/config").len(), 1);

    let (_, out, _) = run(anyr(&home).args(["pi", "--yes", "--dry-run"]));
    assert!(out.contains("mcp: off (Pi has no MCP support)"), "{out}");

    let (_, out, _) = run(anyr(&home).args(["grok", "--yes", "--dry-run"]));
    assert!(out.contains("no key file"), "{out}");
    assert!(!home.join("mcp").join("grok-token").exists());
}

#[test]
fn a_slow_failing_or_absent_gateway_is_asked_once_not_on_every_launch() {
    for (tag, mode) in [
        ("hang", Mode::Hang),
        ("500", Mode::ServerError),
        ("404", Mode::NotFound),
    ] {
        let gw = Gateway::start_mode(mode);
        let home = temp_home(&format!("neg-{tag}"));
        login(&home, &gw.base);
        assert_eq!(launch(&gw, &home, "codex", &[]).trim(), "NO_MCP", "{tag}");
        let first = gw.hits("GET", "/api/v1/mcp/config").len();
        assert_eq!(first, 1, "{tag}");
        let started = std::time::Instant::now();
        assert_eq!(launch(&gw, &home, "codex", &[]).trim(), "NO_MCP", "{tag}");
        assert_eq!(
            gw.hits("GET", "/api/v1/mcp/config").len(),
            1,
            "{tag}: the second launch asked the gateway again"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_millis(2000),
            "{tag}: the second launch waited on the gateway"
        );
    }
}

#[test]
fn every_auto_agent_gets_the_artifact_tools_with_zero_setup() {
    for agent in ["claude", "codex", "opencode", "grok", "kilo", "crush"] {
        let gw = Gateway::start_mode(Mode::ArtifactsOnly);
        let home = temp_home(&format!("{agent}-artifacts"));
        login(&home, &gw.base);
        // The user's own agent config files must come out byte-identical.
        let user_files = [
            (
                home.join(".claude.json"),
                "{\"mcpServers\":{\"mine\":{\"url\":\"https://x.test\"}}}\n",
            ),
            (
                home.join(".claude").join("settings.json"),
                "{\"theme\":\"dark\"}\n",
            ),
            (
                home.join(".codex").join("config.toml"),
                "model = \"gpt-5\"\n",
            ),
            (
                home.join(".config").join("opencode").join("opencode.json"),
                "{\"theme\":\"tokyonight\"}\n",
            ),
            (
                home.join(".config").join("crush").join("crush.json"),
                "{}\n",
            ),
            (home.join(".config").join("kilo").join("kilo.json"), "{}\n"),
        ];
        for (path, text) in &user_files {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let out = launch(&gw, &home, agent, &[]);
        assert!(
            out.contains(&format!("TOOLS {ARTIFACT_TOOLS}")),
            "{agent} did not reach the artifact tools:\n{out}"
        );
        let mcp: Vec<Req> = gw
            .hits("POST", "/api/v1/mcp")
            .into_iter()
            .filter(|r| r.path.ends_with("?tools=artifacts"))
            .collect();
        assert_eq!(mcp.len(), 2, "{agent}: {mcp:?}");
        assert!(mcp.iter().all(|r| r.auth == format!("Bearer {KEY}")));
        for (path, text) in &user_files {
            assert_eq!(
                &std::fs::read_to_string(path).unwrap(),
                text,
                "{agent} changed {}",
                path.display()
            );
        }
    }
}

#[test]
fn a_gateway_without_artifacts_over_mcp_gets_nothing_new() {
    for agent in ["claude", "codex"] {
        let gw = Gateway::start_mode(Mode::LegacyOff);
        let home = temp_home(&format!("{agent}-legacy"));
        login(&home, &gw.base);
        assert_eq!(launch(&gw, &home, agent, &[]).trim(), "NO_MCP", "{agent}");
        assert!(gw.hits("POST", "/api/v1/mcp").is_empty(), "{agent}");
    }
}

#[test]
fn a_scripted_launch_of_an_agent_without_mcp_prints_no_hint() {
    let gw = Gateway::start_mode(Mode::ArtifactsOnly);
    let home = temp_home("pi-hint");
    login(&home, &gw.base);
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-mcp-agent.py");
    let (code, out, err) = run(anyr(&home)
        .args(["aider", "--yes", "--model", "z-ai/glm-4.7"])
        .env("ANYROUTER_AIDER_PATH", &fake)
        .env("FAKE_AGENT", "aider"));
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        !err.contains("artifact"),
        "scripted run printed the hint:\n{err}"
    );
    assert!(gw.hits("GET", "/api/v1/mcp/config").is_empty());
}

#[test]
fn grok_artifacts_entry_stays_out_of_the_users_own_grok_config() {
    let gw = Gateway::start_mode(Mode::ArtifactsOnly);
    let home = temp_home("grok-user-home");
    login(&home, &gw.base);
    // The user's Grok home is separate from anyr's private one.
    let user_grok = home.join("user-grok");
    std::fs::create_dir_all(&user_grok).unwrap();
    let user = "[models]\ndefault = \"grok-4.5\"\n\n[mcp_servers.linear]\nurl = \"https://mcp.linear.app/mcp\"\n";
    std::fs::write(user_grok.join("config.toml"), user).unwrap();
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-mcp-agent.py");
    let (code, out, err) = run(anyr(&home)
        .args(["grok", "--yes", "--model", "z-ai/glm-4.7"])
        .env("GROK_HOME", &user_grok)
        .env("ANYROUTER_GROK_PATH", &fake)
        .env("FAKE_AGENT", "grok"));
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains(&format!("TOOLS {ARTIFACT_TOOLS}")), "{out}");
    assert_eq!(
        std::fs::read_to_string(user_grok.join("config.toml")).unwrap(),
        user,
        "anyr wrote into the user's own Grok config"
    );
    assert!(!out.contains(KEY) && !err.contains(KEY));
}
