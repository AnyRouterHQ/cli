//! Wire the AnyRouter MCP gateway into a launched coding agent.
//!
//! `anyr <agent>` asks the gateway (`GET /v1/mcp/config`, with the inference
//! key it launches with) what to connect, then hands the agent one MCP server
//! the way that agent takes config. With "auto-connect in coding agents" on it
//! is the whole gateway; otherwise it is the artifact tools alone
//! (`/v1/mcp?tools=artifacts`), so the agent can publish a page and hand back a
//! link with no setup. Only an owner who turns auto-connect off explicitly, or
//! a key that may not publish artifacts, gets neither. Per-launch config wins wherever the agent has it, so nothing of the
//! user's is edited. The agent reads the key from `ANYROUTER_API_KEY`, never
//! from argv or a file it shares with other tools.
//!
//! Opt out per launch with `--no-mcp`, or per machine with `mcp.auto: false`
//! (`anyr mcp auto off`). Any failure to read the setting launches without MCP.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::cmd::mcp::{mcp_url, merge_toml, remove_toml_table, SERVER_NAME};
use crate::config::Config;

/// The child env var every strategy reads the key from. Claude Code does not
/// expand `${ANTHROPIC_AUTH_TOKEN}` in MCP headers, so it can't reuse that one.
pub(crate) const KEY_ENV: &str = "ANYROUTER_API_KEY";
/// How long an answer is reused. An unreachable or slow gateway is remembered
/// too (briefly), so it does not add the request timeout to every launch.
const TTL_ANSWER_SECS: u64 = 300;
const TTL_ABSENT_SECS: u64 = 600;
const TTL_ERROR_SECS: u64 = 120;
const CACHE_FILE: &str = "mcp-config-cache.json";

/// How one launcher takes an MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Strategy {
    /// `claude --mcp-config=<inline JSON>`: this session only.
    ClaudeInline,
    /// `codex -c mcp_servers.anyrouter.*=…`: this session only.
    CodexOverrides,
    /// An `mcp` entry in the inline `OPENCODE_CONFIG_CONTENT` (Kilo:
    /// `KILO_CONFIG_CONTENT`): this session only.
    OpencodeInline,
    /// An `mcp` entry in the launch's private Crush config directory, written
    /// by `spawn::prepare_crush_config`; the header reads `$ANYROUTER_API_KEY`.
    CrushPrivate,
    /// Grok's config overlay drops `mcp_servers`, so anyr keeps one marked entry
    /// in the launch's private `$GROK_HOME/config.toml` that reads the key from
    /// `bearer_token_env_var`. Only `anyr mcp add grok` uses a token file.
    GrokManaged,
    /// The agent has no MCP config anyr can set; the reason is shown in `--dry-run`.
    Unsupported(&'static str),
}

/// Every launcher `anyr --help` lists, with how it takes MCP.
pub(crate) const LAUNCHERS: &[(&str, Strategy)] = &[
    ("claude", Strategy::ClaudeInline),
    ("codex", Strategy::CodexOverrides),
    ("opencode", Strategy::OpencodeInline),
    ("grok", Strategy::GrokManaged),
    (
        "pi",
        Strategy::Unsupported("Pi has no MCP support"),
    ),
    ("kilo", Strategy::OpencodeInline),
    ("crush", Strategy::CrushPrivate),
    (
        "qwen",
        Strategy::Unsupported(
            "Qwen Code only reads MCP servers from its settings.json; add one with `qwen mcp add`",
        ),
    ),
    (
        "goose",
        Strategy::Unsupported(
            "Goose takes MCP servers as extensions in its config.yaml; add one with `goose configure`",
        ),
    ),
    ("aider", Strategy::Unsupported("Aider has no MCP support")),
    (
        "fx",
        Strategy::Unsupported(
            "fx reads MCP servers from ~/.fx/mcp.json; add one with `fx mcp add`",
        ),
    ),
    (
        "pool",
        Strategy::Unsupported(
            "Poolside only reads MCP servers from its global settings.yaml; add one with `pool mcp add`",
        ),
    ),
];

pub(crate) fn strategy(tool: &str) -> Strategy {
    let id = crate::spawn::canonical_tool(tool);
    LAUNCHERS
        .iter()
        .find(|(name, _)| *name == id)
        .map(|(_, s)| *s)
        .unwrap_or(Strategy::Unsupported(
            "anyr does not know how this agent takes MCP",
        ))
}

/// Whether this launch gets the MCP server, and why not when it doesn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    On {
        url: String,
    },
    Off(String),
    /// `--dry-run` with nothing cached: the gateway was not asked.
    Unchecked,
}

/// What the gateway said about auto-connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Setting {
    On,
    /// Full auto-connect is off, but the artifact tools are on.
    Artifacts,
    Off,
    /// 403 / 404: the key may not use MCP, or the gateway predates the setting.
    Absent,
}

impl Setting {
    fn ttl(&self) -> u64 {
        match self {
            Setting::Absent => TTL_ABSENT_SECS,
            _ => TTL_ANSWER_SECS,
        }
    }
}

type Lookup = Result<Setting, String>;

pub(crate) struct DecideInput<'a> {
    pub tool: &'a str,
    pub no_mcp_flag: bool,
    pub config: Option<&'a Config>,
    pub base: &'a str,
    pub key: &'a str,
    /// Where the short-lived cache of the workspace setting lives.
    pub cache_dir: Option<&'a Path>,
    /// `--dry-run` never asks the gateway and never writes the cache; it reads
    /// the cache when there is one.
    pub dry_run: bool,
}

/// Decide without touching the network unless every local opt-out is clear.
pub(crate) fn decide(input: &DecideInput<'_>) -> Decision {
    decide_with(input, &|base, key| fetch_enabled(base, key))
}

fn decide_with(input: &DecideInput<'_>, fetch: &dyn Fn(&str, &str) -> Lookup) -> Decision {
    if input.no_mcp_flag {
        return Decision::Off("--no-mcp".into());
    }
    if input.config.and_then(|c| c.mcp_auto) == Some(false) {
        return Decision::Off("mcp.auto is off on this machine (`anyr mcp auto on`)".into());
    }
    if let Strategy::Unsupported(why) = strategy(input.tool) {
        return Decision::Off(why.into());
    }
    let cache = input
        .cache_dir
        .map(|d| d.join(CACHE_FILE))
        .filter(|_| cfg!(feature = "native"));
    let cache_key = cache_key(input.base, input.key);
    let looked_up = match cache
        .as_deref()
        .and_then(|p| read_cache(p, &cache_key, now_secs()))
    {
        Some(v) => v,
        None if input.dry_run => return Decision::Unchecked,
        None => {
            let fetched = fetch(input.base, input.key);
            if let Some(p) = cache.as_deref() {
                write_cache(p, &cache_key, &fetched, now_secs());
            }
            fetched
        }
    };
    match looked_up {
        Ok(Setting::On) => Decision::On {
            url: mcp_url(input.base),
        },
        Ok(Setting::Artifacts) => Decision::On {
            url: artifacts_url(input.base),
        },
        Ok(Setting::Off) => {
            Decision::Off("auto-connect is off for this workspace or key (dashboard → MCP)".into())
        }
        Ok(Setting::Absent) => {
            Decision::Off("this gateway or key has no MCP auto-connect setting".into())
        }
        Err(e) => Decision::Off(format!("could not read the MCP setting ({e})")),
    }
}

/// The gateway reduced to the artifact tools: what a launch connects when full
/// auto-connect is off.
pub(crate) fn artifacts_url(base: &str) -> String {
    format!("{}?tools=artifacts", mcp_url(base))
}

/// `GET /v1/mcp/config`. 403 (key not allowed MCP) and 404 (older gateway) read
/// as absent; anything else that isn't a 2xx is an error, which also means off.
fn fetch_enabled(base: &str, key: &str) -> Lookup {
    let url = crate::http::join_api(base, "/v1/mcp/config");
    let (status, body) =
        crate::http::http_get_timeout(&url, Some(key), std::time::Duration::from_millis(2500))?;
    match status {
        200..=299 => serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| parse_setting(&v))
            .ok_or_else(|| "unexpected response".to_string()),
        401 => Err("the key was rejected (HTTP 401)".into()),
        403 | 404 => Ok(Setting::Absent),
        s => Err(format!("HTTP {s}")),
    }
}

/// `enabled` connects the whole gateway; failing that, `artifacts.enabled`
/// connects the artifact tools. A gateway without the `artifacts` field
/// predates them, so it gets only what it asked for.
fn parse_setting(v: &serde_json::Value) -> Option<Setting> {
    if v.get("enabled")?.as_bool()? {
        return Some(Setting::On);
    }
    let artifacts = v
        .get("artifacts")
        .and_then(|a| a.get("enabled"))
        .and_then(|e| e.as_bool())
        .unwrap_or(false);
    Some(if artifacts {
        Setting::Artifacts
    } else {
        Setting::Off
    })
}

/// One line for an interactive launch of an agent anyr cannot give MCP, so the
/// user knows why it cannot publish artifacts. `None` for every other launch.
pub(crate) fn unsupported_hint(tool: &str, decision: &Decision) -> Option<String> {
    let Strategy::Unsupported(why) = strategy(tool) else {
        return None;
    };
    // An opt-out (`--no-mcp`, `mcp.auto: false`) is decided first, so the
    // decision carries the agent's own reason only when nothing opted out.
    match decision {
        Decision::Off(reason) if reason == why => Some(format!(
            "anyr: {tool} gets no AnyRouter artifact tools: {why}."
        )),
        _ => None,
    }
}

/// Cache rows are keyed by a hash of the base URL and key, never the key.
pub(crate) fn cache_key(base: &str, key: &str) -> String {
    let digest = Sha256::digest(format!("{base}\n{key}").as_bytes());
    digest.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_cache(path: &Path, key: &str, now: u64) -> Option<Lookup> {
    let text = std::fs::read_to_string(path).ok()?;
    let row = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let row = row.get(key)?;
    let at = row.get("at")?.as_u64()?;
    let ttl = row.get("ttl")?.as_u64()?;
    if at > now || now - at >= ttl {
        return None;
    }
    match row.get("state")?.as_str()? {
        "on" => Some(Ok(Setting::On)),
        "artifacts" => Some(Ok(Setting::Artifacts)),
        "off" => Some(Ok(Setting::Off)),
        "absent" => Some(Ok(Setting::Absent)),
        "error" => Some(Err(row.get("error")?.as_str()?.to_string())),
        _ => None,
    }
}

fn write_cache(path: &Path, key: &str, looked_up: &Lookup, now: u64) {
    let mut rows = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({}));
    // Keep the file small: drop expired rows from other keys.
    if let Some(map) = rows.as_object_mut() {
        map.retain(|_, v| {
            let at = v.get("at").and_then(|a| a.as_u64());
            let ttl = v.get("ttl").and_then(|t| t.as_u64());
            matches!((at, ttl), (Some(at), Some(ttl)) if now.saturating_sub(at) < ttl)
        });
        let row = match looked_up {
            Ok(setting) => {
                let state = match setting {
                    Setting::On => "on",
                    Setting::Artifacts => "artifacts",
                    Setting::Off => "off",
                    Setting::Absent => "absent",
                };
                json!({ "state": state, "at": now, "ttl": setting.ttl() })
            }
            Err(e) => json!({ "state": "error", "error": e, "at": now, "ttl": TTL_ERROR_SECS }),
        };
        map.insert(key.into(), row);
    }
    if let Some(dir) = path.parent() {
        let _ = crate::config::ensure_private_dir(dir);
    }
    let _ = crate::config::write_private(path, rows.to_string().as_bytes());
}

/// Paths the Grok strategy reads and writes.
pub(crate) struct GrokPaths {
    pub config: PathBuf,
    pub token: PathBuf,
}

pub(crate) fn grok_paths(
    env: &BTreeMap<String, String>,
    anyr_config_path: &Path,
) -> Option<GrokPaths> {
    let home = env
        .get("GROK_HOME")
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            ["HOME", "USERPROFILE"]
                .iter()
                .find_map(|k| env.get(*k).filter(|v| !v.trim().is_empty()))
                .map(|h| PathBuf::from(h).join(".grok"))
        })?;
    let anyr_dir = anyr_config_path.parent().unwrap_or(Path::new("."));
    Some(GrokPaths {
        config: home.join("config.toml"),
        token: anyr_dir.join("mcp").join("grok-token"),
    })
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// First words of the comment that marks an entry as anyr's.
const GROK_MARKER: &str = "# Managed by anyr";

/// How Grok gets the key for the managed entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GrokAuth<'a> {
    /// `bearer_token_env_var`: nothing but the variable name reaches a file.
    Env,
    /// `bearer_token_file`: the key goes to anyr's owner-only token file. Only
    /// the explicit `anyr mcp add grok`, whose entry outlives a launch.
    File(&'a str),
}

pub(crate) fn grok_block(url: &str, auth: GrokAuth<'_>, token_path: &Path) -> String {
    let (note, auth_line) = match auth {
        GrokAuth::Env => (
            "`anyr grok` keeps this entry in its own Grok home; the key comes from $ANYROUTER_API_KEY.".to_string(),
            format!("bearer_token_env_var = {}", toml_string(KEY_ENV)),
        ),
        GrokAuth::File(_) => (
            "`anyr mcp remove grok` deletes it and the key file.".to_string(),
            format!(
                "bearer_token_file = {}",
                toml_string(&token_path.to_string_lossy())
            ),
        ),
    };
    format!(
        "[mcp_servers.{SERVER_NAME}]\n{GROK_MARKER}: {note}\nurl = {}\n{auth_line}\n",
        toml_string(url),
    )
}

/// The lines of `[mcp_servers.anyrouter]` in `text`, if that table exists.
fn grok_table(text: &str) -> Option<Vec<&str>> {
    let header = format!("[mcp_servers.{SERVER_NAME}]");
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.trim() == header)?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |i| start + 1 + i);
    Some(lines[start..end].to_vec())
}

/// anyr owns the table when it carries anyr's marker comment; a server the user
/// added under the same name is left alone.
pub(crate) fn grok_table_is_ours(text: &str) -> bool {
    grok_table(text).is_some_and(|t| t.iter().any(|l| l.trim_start().starts_with(GROK_MARKER)))
}

/// Outcome of syncing Grok's managed entry, for the launch note and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GrokSync {
    Written,
    Unchanged,
    Removed,
    Absent,
    /// The user has their own `anyrouter` server; anyr does not touch it.
    UserOwned,
}

/// Make Grok's config hold anyr's entry (`target = Some((url, auth))`) or not
/// hold it (`None`). The token file is only ever written here, never deleted:
/// `anyr mcp remove grok` owns that, so a launch that drops the entry (a
/// `--no-mcp`, auto off) cannot break an explicit `anyr mcp add grok`.
pub(crate) fn sync_grok(
    paths: &GrokPaths,
    target: Option<(&str, GrokAuth<'_>)>,
) -> Result<GrokSync, String> {
    let existing = match std::fs::read_to_string(&paths.config) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("cannot read {}: {e}", paths.config.display())),
    };
    let text = existing.as_deref().unwrap_or("");
    let has_table = grok_table(text).is_some();
    let ours = grok_table_is_ours(text);
    if has_table && !ours {
        return Ok(GrokSync::UserOwned);
    }
    let Some((url, auth)) = target else {
        if !ours {
            return Ok(GrokSync::Absent);
        }
        let next = remove_toml_table(text, &format!("[mcp_servers.{SERVER_NAME}]"));
        write_file(&paths.config, &next)?;
        return Ok(GrokSync::Removed);
    };
    if let GrokAuth::File(key) = auth {
        write_token(&paths.token, key)?;
    }
    let next = merge_toml(existing.as_deref(), &grok_block(url, auth, &paths.token));
    if existing.as_deref() == Some(next.as_str()) {
        return Ok(GrokSync::Unchanged);
    }
    write_file(&paths.config, &next)?;
    Ok(GrokSync::Written)
}

/// Write `content` to `path`. A symlink is replaced by a regular file (its
/// target's content is already in `content`), so a launch-private Grok home
/// that links to the user's config never writes through into the user's file.
fn write_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        std::fs::remove_file(path)
            .map_err(|e| format!("cannot replace link {}: {e}", path.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Owner-only token file, replaced atomically (Grok reads it on every request).
fn write_token(path: &Path, key: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).is_ok_and(|s| s.trim() == key) {
        return Ok(());
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    crate::config::ensure_private_dir(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let tmp = path.with_extension("tmp");
    crate::config::write_private(&tmp, key.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// What one launch does with MCP, applied to the child's args and env.
pub(crate) struct Wiring {
    /// One line for `--dry-run`.
    pub summary: String,
}

pub(crate) struct ApplyInput<'a> {
    pub tool: &'a str,
    pub decision: &'a Decision,
    pub key: &'a str,
    pub env: &'a BTreeMap<String, String>,
    pub config_path: &'a Path,
    /// `--dry-run`: describe the Grok file change, don't make it.
    pub dry_run: bool,
}

/// Add the MCP server to `args` / `child_env` for this launch. Grok's managed
/// entry is also removed here when the decision is off.
pub(crate) fn apply(
    input: &ApplyInput<'_>,
    args: &mut Vec<String>,
    child_env: &mut BTreeMap<String, String>,
) -> Wiring {
    let tool = crate::spawn::canonical_tool(input.tool);
    let strategy = strategy(tool);
    let url = match input.decision {
        Decision::On { url } => Some(url.as_str()),
        Decision::Off(_) | Decision::Unchecked => None,
    };
    if strategy == Strategy::GrokManaged {
        return apply_grok(input, url, child_env);
    }
    let Some(url) = url else {
        return Wiring {
            summary: not_connected(input.decision),
        };
    };
    child_env.insert(KEY_ENV.into(), input.key.to_string());
    let how = match strategy {
        Strategy::ClaudeInline => {
            args.push(format!("--mcp-config={}", claude_config(url)));
            "--mcp-config"
        }
        Strategy::CodexOverrides => {
            args.extend(codex_args(url));
            "-c mcp_servers.anyrouter"
        }
        Strategy::OpencodeInline => {
            let var = crate::spawn::inline_config_env(tool).unwrap_or("OPENCODE_CONFIG_CONTENT");
            let current = child_env.get(var).map(String::as_str).unwrap_or("{}");
            child_env.insert(var.into(), opencode_config(current, url));
            if var == "KILO_CONFIG_CONTENT" {
                "KILO_CONFIG_CONTENT mcp"
            } else {
                "OPENCODE_CONFIG_CONTENT mcp"
            }
        }
        Strategy::CrushPrivate => "anyr's private crush.json mcp",
        Strategy::GrokManaged | Strategy::Unsupported(_) => unreachable!(),
    };
    Wiring {
        summary: format!("mcp: {SERVER_NAME} {url} via {how} (key from ${KEY_ENV})"),
    }
}

/// The `--dry-run` / launch line for a launch that connects nothing.
fn not_connected(decision: &Decision) -> String {
    match decision {
        Decision::Off(why) => format!("mcp: off ({why})"),
        Decision::Unchecked => {
            "mcp: not checked (--dry-run does not ask the gateway; a launch connects the gateway or its artifact tools unless the workspace turned auto-connect off)".into()
        }
        Decision::On { .. } => unreachable!("a connected launch has a url"),
    }
}

fn apply_grok(
    input: &ApplyInput<'_>,
    url: Option<&str>,
    child_env: &mut BTreeMap<String, String>,
) -> Wiring {
    // A launch that gives Grok its own home wins over the user's GROK_HOME.
    let mut env = input.env.clone();
    if let Some(home) = child_env.get("GROK_HOME").filter(|v| !v.trim().is_empty()) {
        env.insert("GROK_HOME".into(), home.clone());
    }
    let Some(paths) = grok_paths(&env, input.config_path) else {
        return Wiring {
            summary: "mcp: off (cannot find Grok's home; set GROK_HOME)".into(),
        };
    };
    let file = paths.config.display();
    if input.dry_run {
        let summary = match url {
            Some(url) => format!(
                "mcp: {SERVER_NAME} {url} via [mcp_servers.{SERVER_NAME}] in {file} (managed by anyr, key from ${KEY_ENV}, no key file)"
            ),
            None => match input.decision {
                Decision::Off(why) => format!(
                    "mcp: off ({why}); anyr's entry in {file} is removed if present"
                ),
                other => not_connected(other),
            },
        };
        return Wiring { summary };
    }
    // `Unchecked` only happens in a dry run, which returned above.
    let target = url.map(|u| (u, GrokAuth::Env));
    if target.is_some() {
        child_env.insert(KEY_ENV.into(), input.key.to_string());
    }
    let summary = match (sync_grok(&paths, target), url) {
        (Ok(GrokSync::UserOwned), _) => {
            format!("mcp: left alone (your own [mcp_servers.{SERVER_NAME}] in {file})")
        }
        (Ok(_), Some(url)) => format!("mcp: {SERVER_NAME} {url} via {file} (key from ${KEY_ENV})"),
        (Ok(_), None) => not_connected(input.decision),
        (Err(e), _) => {
            eprintln!("anyr: could not update Grok's MCP config: {e}");
            format!("mcp: off ({e})")
        }
    };
    Wiring { summary }
}

pub(crate) fn claude_config(url: &str) -> String {
    json!({
        "mcpServers": {
            SERVER_NAME: {
                "type": "http",
                "url": url,
                "headers": { "Authorization": format!("Bearer ${{{KEY_ENV}}}") },
            }
        }
    })
    .to_string()
}

pub(crate) fn codex_args(url: &str) -> Vec<String> {
    vec![
        "-c".into(),
        format!("mcp_servers.{SERVER_NAME}.url={}", toml_string(url)),
        "-c".into(),
        format!("mcp_servers.{SERVER_NAME}.bearer_token_env_var=\"{KEY_ENV}\""),
    ]
}

/// Add `mcp.anyrouter` to opencode's inline config, keeping what is there.
pub(crate) fn opencode_config(current: &str, url: &str) -> String {
    let mut config = serde_json::from_str::<serde_json::Value>(current)
        .ok()
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({}));
    if !config["mcp"].is_object() {
        config["mcp"] = json!({});
    }
    config["mcp"][SERVER_NAME] = json!({
        "type": "remote",
        "url": url,
        "enabled": true,
        "headers": { "Authorization": format!("Bearer {{env:{KEY_ENV}}}") },
    });
    config.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const URL: &str = "https://anyrouter.dev/api/v1/mcp";
    const KEY: &str = "sk-ar-v1-secret-test-key";
    const BASE: &str = "https://anyrouter.dev/api";

    fn input<'a>(tool: &'a str, config: Option<&'a Config>) -> DecideInput<'a> {
        DecideInput {
            tool,
            no_mcp_flag: false,
            config,
            base: BASE,
            key: KEY,
            cache_dir: None,
            dry_run: false,
        }
    }

    fn on(_: &str, _: &str) -> Lookup {
        Ok(Setting::On)
    }

    fn tmp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("anyr-mcpw-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_launcher_in_help_has_a_strategy() {
        let aliases = ["cc", "poolside"];
        for tool in crate::registry::LAUNCHERS.iter().chain(&aliases) {
            let id = crate::spawn::canonical_tool(tool);
            assert!(
                LAUNCHERS.iter().any(|(n, _)| *n == id),
                "{tool} has no MCP strategy"
            );
        }
        assert_eq!(strategy("cc"), Strategy::ClaudeInline);
        assert!(matches!(strategy("pi"), Strategy::Unsupported(_)));
    }

    #[test]
    fn local_opt_outs_win_before_any_network_call() {
        let never = |_: &str, _: &str| -> Lookup { panic!("fetched") };
        let mut i = input("claude", None);
        i.no_mcp_flag = true;
        assert_eq!(decide_with(&i, &never), Decision::Off("--no-mcp".into()));

        let cfg = Config {
            mcp_auto: Some(false),
            ..Config::default()
        };
        assert!(matches!(
            decide_with(&input("claude", Some(&cfg)), &never),
            Decision::Off(_)
        ));
        assert!(matches!(
            decide_with(&input("pi", None), &never),
            Decision::Off(_)
        ));
    }

    #[test]
    fn the_workspace_setting_decides_and_failures_launch_without_mcp() {
        assert_eq!(
            decide_with(&input("codex", None), &on),
            Decision::On { url: URL.into() }
        );
        let off = |_: &str, _: &str| Ok(Setting::Off);
        assert!(matches!(
            decide_with(&input("codex", None), &off),
            Decision::Off(_)
        ));
        let err = |_: &str, _: &str| Err("timeout".to_string());
        assert!(matches!(
            decide_with(&input("codex", None), &err),
            Decision::Off(_)
        ));
    }

    #[test]
    fn artifacts_are_the_default_and_only_an_explicit_off_drops_them() {
        let parse = |v: Value| parse_setting(&v);
        // Full auto-connect wins and already carries the artifact tools.
        assert_eq!(
            parse(serde_json::json!({"enabled": true, "artifacts": {"enabled": true}})),
            Some(Setting::On)
        );
        assert_eq!(
            parse(serde_json::json!({"enabled": false, "artifacts": {"enabled": true}})),
            Some(Setting::Artifacts)
        );
        // Owner turned it off, or the key may not publish.
        assert_eq!(
            parse(serde_json::json!({"enabled": false, "artifacts": {"enabled": false}})),
            Some(Setting::Off)
        );
        // A gateway that predates artifacts over MCP: nothing new is connected.
        assert_eq!(
            parse(serde_json::json!({"enabled": false})),
            Some(Setting::Off)
        );
        assert_eq!(parse(serde_json::json!({"artifacts": {}})), None);

        let artifacts = |_: &str, _: &str| Ok(Setting::Artifacts);
        assert_eq!(
            decide_with(&input("claude", None), &artifacts),
            Decision::On {
                url: format!("{URL}?tools=artifacts")
            }
        );
    }

    #[test]
    fn only_an_agent_without_mcp_gets_the_hint_and_only_without_an_opt_out() {
        let never = |_: &str, _: &str| -> Lookup { panic!("fetched") };
        for tool in ["pi", "aider", "qwen", "goose", "pool"] {
            let d = decide_with(&input(tool, None), &never);
            let hint = unsupported_hint(tool, &d).expect(tool);
            assert!(hint.contains(tool) && hint.contains("artifact"), "{hint}");
        }
        let mut i = input("pi", None);
        i.no_mcp_flag = true;
        assert_eq!(unsupported_hint("pi", &decide_with(&i, &never)), None);
        let cfg = Config {
            mcp_auto: Some(false),
            ..Config::default()
        };
        assert_eq!(
            unsupported_hint("aider", &decide_with(&input("aider", Some(&cfg)), &never)),
            None
        );
        let artifacts = |_: &str, _: &str| Ok(Setting::Artifacts);
        assert_eq!(
            unsupported_hint("claude", &decide_with(&input("claude", None), &artifacts)),
            None
        );
    }

    #[test]
    fn the_cache_hides_the_key_and_expires() {
        let dir = tmp("cache");
        let path = dir.join(CACHE_FILE);
        let k = cache_key(BASE, KEY);
        write_cache(&path, &k, &Ok(Setting::On), 1_000);
        assert!(!std::fs::read_to_string(&path).unwrap().contains(KEY));
        assert_eq!(
            read_cache(&path, &k, 1_000 + TTL_ANSWER_SECS - 1),
            Some(Ok(Setting::On))
        );
        assert_eq!(read_cache(&path, &k, 1_000 + TTL_ANSWER_SECS), None);
        write_cache(&path, &k, &Ok(Setting::Artifacts), 1_000);
        assert_eq!(read_cache(&path, &k, 1_001), Some(Ok(Setting::Artifacts)));
        assert_eq!(
            read_cache(&path, &cache_key(BASE, "sk-ar-v1-other"), 1_001),
            None
        );

        // A cached answer is used instead of fetching.
        let never = |_: &str, _: &str| -> Lookup { panic!("fetched") };
        write_cache(&path, &k, &Ok(Setting::On), now_secs());
        let mut i = input("claude", None);
        i.cache_dir = Some(&dir);
        assert!(matches!(decide_with(&i, &never), Decision::On { .. }));
    }

    #[test]
    fn failures_and_absent_gateways_are_cached_with_their_own_short_ttls() {
        let dir = tmp("neg-cache");
        let path = dir.join(CACHE_FILE);
        let k = cache_key(BASE, KEY);
        let err: Lookup = Err("timed out".into());
        write_cache(&path, &k, &err, 1_000);
        assert_eq!(read_cache(&path, &k, 1_000 + TTL_ERROR_SECS - 1), Some(err));
        assert_eq!(read_cache(&path, &k, 1_000 + TTL_ERROR_SECS), None);
        write_cache(&path, &k, &Ok(Setting::Absent), 1_000);
        assert_eq!(
            read_cache(&path, &k, 1_000 + TTL_ABSENT_SECS - 1),
            Some(Ok(Setting::Absent))
        );
        assert_eq!(read_cache(&path, &k, 1_000 + TTL_ABSENT_SECS), None);

        // The second decision is served from the cache: it never fetches.
        let calls = std::cell::Cell::new(0);
        let failing = |_: &str, _: &str| -> Lookup {
            calls.set(calls.get() + 1);
            Err("timed out".into())
        };
        std::fs::remove_file(&path).unwrap();
        let mut i = input("claude", None);
        i.cache_dir = Some(&dir);
        assert!(matches!(decide_with(&i, &failing), Decision::Off(_)));
        assert!(matches!(decide_with(&i, &failing), Decision::Off(_)));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn dry_run_neither_fetches_nor_writes_the_cache() {
        let dir = tmp("dry-cache");
        let never = |_: &str, _: &str| -> Lookup { panic!("fetched") };
        let mut i = input("claude", None);
        i.cache_dir = Some(&dir);
        i.dry_run = true;
        assert_eq!(decide_with(&i, &never), Decision::Unchecked);
        assert!(!dir.join(CACHE_FILE).exists());

        // A warm cache is read, not refreshed.
        write_cache(
            &dir.join(CACHE_FILE),
            &cache_key(BASE, KEY),
            &Ok(Setting::On),
            now_secs(),
        );
        assert!(matches!(decide_with(&i, &never), Decision::On { .. }));
    }

    fn wire(
        tool: &str,
        start_env: &[(&str, &str)],
    ) -> (Vec<String>, BTreeMap<String, String>, String) {
        let mut args = vec!["--existing".to_string()];
        let mut child: BTreeMap<String, String> = start_env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let decision = Decision::On { url: URL.into() };
        let env = BTreeMap::new();
        let w = apply(
            &ApplyInput {
                tool,
                decision: &decision,
                key: KEY,
                env: &env,
                config_path: Path::new("/nonexistent/config.yaml"),
                dry_run: false,
            },
            &mut args,
            &mut child,
        );
        (args, child, w.summary)
    }

    #[test]
    fn claude_gets_one_inline_config_that_reads_the_key_from_env() {
        let (args, env, summary) = wire("claude", &[]);
        assert_eq!(args.len(), 2);
        // `=` form: --mcp-config is variadic and would swallow a positional prompt.
        let json = args[1]
            .strip_prefix("--mcp-config=")
            .expect("--mcp-config=");
        let v: Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"mcpServers":{"anyrouter":{"type":"http","url":URL,
                "headers":{"Authorization":"Bearer ${ANYROUTER_API_KEY}"}}}})
        );
        assert_eq!(env.get(KEY_ENV).map(String::as_str), Some(KEY));
        assert!(!args.iter().any(|a| a.contains(KEY)), "key in argv");
        assert!(!summary.contains(KEY));
    }

    #[test]
    fn codex_gets_per_launch_overrides_naming_the_env_var() {
        let (args, env, _) = wire("codex", &[]);
        assert_eq!(
            &args[1..],
            &[
                "-c",
                &format!("mcp_servers.anyrouter.url=\"{URL}\""),
                "-c",
                "mcp_servers.anyrouter.bearer_token_env_var=\"ANYROUTER_API_KEY\"",
            ]
        );
        assert_eq!(env.get(KEY_ENV).map(String::as_str), Some(KEY));
    }

    #[test]
    fn opencode_keeps_the_provider_config_and_adds_mcp() {
        let provider = r#"{"$schema":"https://opencode.ai/config.json","provider":{"anyrouter":{"name":"AnyRouter"}},"model":"anyrouter/z-ai/glm-4.7"}"#;
        let (args, env, _) = wire("opencode", &[("OPENCODE_CONFIG_CONTENT", provider)]);
        assert_eq!(args, vec!["--existing"]);
        let v: Value = serde_json::from_str(&env["OPENCODE_CONFIG_CONTENT"]).unwrap();
        assert_eq!(v["provider"]["anyrouter"]["name"], "AnyRouter");
        assert_eq!(v["model"], "anyrouter/z-ai/glm-4.7");
        assert_eq!(
            v["mcp"]["anyrouter"],
            serde_json::json!({"type":"remote","url":URL,"enabled":true,
                "headers":{"Authorization":"Bearer {env:ANYROUTER_API_KEY}"}})
        );
        assert!(!env["OPENCODE_CONFIG_CONTENT"].contains(KEY));
    }

    #[test]
    fn off_changes_nothing_for_per_launch_agents() {
        for tool in ["claude", "codex", "opencode"] {
            let mut args = vec![];
            let mut child = BTreeMap::new();
            let decision = Decision::Off("--no-mcp".into());
            let w = apply(
                &ApplyInput {
                    tool,
                    decision: &decision,
                    key: KEY,
                    env: &BTreeMap::new(),
                    config_path: Path::new("/x/config.yaml"),
                    dry_run: false,
                },
                &mut args,
                &mut child,
            );
            assert!(args.is_empty() && child.is_empty(), "{tool}");
            assert_eq!(w.summary, "mcp: off (--no-mcp)");
        }
    }

    fn grok_env(dir: &Path) -> (BTreeMap<String, String>, PathBuf) {
        let env = BTreeMap::from([(
            "GROK_HOME".to_string(),
            dir.join("grok").display().to_string(),
        )]);
        (env, dir.join("anyr").join("config.yaml"))
    }

    #[test]
    fn grok_entry_is_written_once_kept_beside_user_config_and_removed_when_off() {
        let dir = tmp("grok");
        let (env, cfg_path) = grok_env(&dir);
        let paths = grok_paths(&env, &cfg_path).unwrap();
        std::fs::create_dir_all(paths.config.parent().unwrap()).unwrap();
        let user = "[models]\ndefault = \"grok-4.5\"\n\n[mcp_servers.linear]\nurl = \"https://mcp.linear.app/mcp\"\n";
        std::fs::write(&paths.config, user).unwrap();

        assert_eq!(
            sync_grok(&paths, Some((URL, GrokAuth::File(KEY)))).unwrap(),
            GrokSync::Written
        );
        let text = std::fs::read_to_string(&paths.config).unwrap();
        assert!(text.starts_with(user.trim_end()));
        assert!(text.contains("# Managed by anyr"));
        assert!(text.contains(&format!("url = \"{URL}\"")));
        assert!(!text.contains(KEY), "key written into Grok's config");
        assert_eq!(std::fs::read_to_string(&paths.token).unwrap(), KEY);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&paths.token)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(
            sync_grok(&paths, Some((URL, GrokAuth::File(KEY)))).unwrap(),
            GrokSync::Unchanged
        );

        assert_eq!(sync_grok(&paths, None).unwrap(), GrokSync::Removed);
        assert_eq!(
            std::fs::read_to_string(&paths.config).unwrap().trim_end(),
            user.trim_end()
        );
        // Dropping the entry never deletes the key file: `mcp remove grok` does.
        assert_eq!(std::fs::read_to_string(&paths.token).unwrap(), KEY);
        assert_eq!(sync_grok(&paths, None).unwrap(), GrokSync::Absent);
    }

    #[test]
    fn grok_never_touches_a_server_the_user_named_anyrouter() {
        let dir = tmp("grok-user");
        let (env, cfg_path) = grok_env(&dir);
        let paths = grok_paths(&env, &cfg_path).unwrap();
        std::fs::create_dir_all(paths.config.parent().unwrap()).unwrap();
        let user = "[mcp_servers.anyrouter]\nurl = \"https://example.test/mcp\"\nheaders = { Authorization = \"Bearer mine\" }\n";
        std::fs::write(&paths.config, user).unwrap();
        assert_eq!(
            sync_grok(&paths, Some((URL, GrokAuth::Env))).unwrap(),
            GrokSync::UserOwned
        );
        assert_eq!(sync_grok(&paths, None).unwrap(), GrokSync::UserOwned);
        assert_eq!(std::fs::read_to_string(&paths.config).unwrap(), user);
        assert!(!paths.token.exists());
    }

    #[cfg(unix)]
    #[test]
    fn grok_follows_the_launch_grok_home_and_never_writes_through_a_link() {
        let dir = tmp("grok-link");
        let user_home = dir.join("user-grok");
        std::fs::create_dir_all(&user_home).unwrap();
        let user = "[models]\ndefault = \"grok-4.5\"\n";
        std::fs::write(user_home.join("config.toml"), user).unwrap();
        // The launch gives Grok a private home whose config links to the user's.
        let launch_home = dir.join("anyr").join("grok");
        std::fs::create_dir_all(&launch_home).unwrap();
        std::os::unix::fs::symlink(
            user_home.join("config.toml"),
            launch_home.join("config.toml"),
        )
        .unwrap();
        let env = BTreeMap::from([("GROK_HOME".to_string(), user_home.display().to_string())]);
        let mut child =
            BTreeMap::from([("GROK_HOME".to_string(), launch_home.display().to_string())]);
        let decision = Decision::On { url: URL.into() };
        apply(
            &ApplyInput {
                tool: "grok",
                decision: &decision,
                key: KEY,
                env: &env,
                config_path: &dir.join("anyr").join("config.yaml"),
                dry_run: false,
            },
            &mut vec![],
            &mut child,
        );
        assert_eq!(
            std::fs::read_to_string(user_home.join("config.toml")).unwrap(),
            user
        );
        let private = launch_home.join("config.toml");
        assert!(!std::fs::symlink_metadata(&private)
            .unwrap()
            .file_type()
            .is_symlink());
        let text = std::fs::read_to_string(&private).unwrap();
        assert!(text.starts_with(user.trim_end()) && text.contains("[mcp_servers.anyrouter]"));
        // The launch passes the key by env: no key in the file, no token file.
        assert!(text.contains("bearer_token_env_var = \"ANYROUTER_API_KEY\""));
        assert!(!text.contains(KEY) && !text.contains("bearer_token_file"));
        assert_eq!(child.get(KEY_ENV).map(String::as_str), Some(KEY));
        assert!(!dir.join("anyr").join("mcp").join("grok-token").exists());
    }

    #[test]
    fn grok_dry_run_describes_without_writing() {
        let dir = tmp("grok-dry");
        let (env, cfg_path) = grok_env(&dir);
        let decision = Decision::On { url: URL.into() };
        let w = apply(
            &ApplyInput {
                tool: "grok",
                decision: &decision,
                key: KEY,
                env: &env,
                config_path: &cfg_path,
                dry_run: true,
            },
            &mut vec![],
            &mut BTreeMap::new(),
        );
        assert!(w.summary.contains(URL) && !w.summary.contains(KEY));
        assert!(w.summary.contains("no key file"), "{}", w.summary);
        assert!(!dir.join("grok").join("config.toml").exists());
    }
}
