//! `anyr mcp url | add <agent> | connections`: wire the AnyRouter MCP endpoint
//! into an agent, and list the upstream MCP connections behind the gateway.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::api::{context, get};
use crate::cmd::view::{print_json, print_table, text};
use crate::http::join_api;
use crate::parse::ParsedArgs;

pub(crate) const SERVER_NAME: &str = "anyrouter";
const KEY_ENV: &str = "ANYROUTER_API_KEY";
const AGENTS: &[&str] = &["claude", "codex", "opencode", "cursor", "grok"];

/// How the bearer token appears in a snippet or config file.
#[derive(Clone, Copy)]
enum Token<'a> {
    /// A reference the agent resolves from the environment at run time.
    Env,
    /// The literal key (`--show-key`, or running `claude mcp add`).
    Literal(&'a str),
}

pub(crate) fn mcp_url(base: &str) -> String {
    join_api(base, "/v1/mcp")
}

pub(crate) fn run_mcp(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let args: Vec<&str> = parsed.passthrough.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["url"] => {
            let url = mcp_url(&context(parsed, env).base);
            if parsed.flag_true("json") {
                print_json(&json!({ "url": url }));
            } else {
                println!("{url}");
            }
            Ok(0)
        }
        ["add", "grok"] => grok(parsed, env, true),
        ["add", agent] => add(parsed, env, agent),
        ["remove", "grok"] => grok(parsed, env, false),
        ["auto"] => auto(parsed, env, None),
        ["auto", v] => auto(parsed, env, Some(v)),
        ["connections"] => connections(parsed, env),
        _ => Err(crate::cmd::dispatch::hint(&format!(
            "Usage: {{bin}} mcp url | add <{}> [--print] [--show-key] | remove grok | auto [on|off] | connections",
            AGENTS.join("|")
        ))),
    }
}

fn header_value(token: Token, env_ref: &str) -> String {
    match token {
        Token::Env => format!("Bearer {env_ref}"),
        Token::Literal(key) => format!("Bearer {key}"),
    }
}

/// The arguments `claude mcp add` is given. Claude Code stores the header as
/// given, so this carries the real key.
fn claude_args(url: &str, key: &str) -> Vec<String> {
    let auth = format!("Authorization: Bearer {key}");
    [
        "mcp",
        "add",
        "--transport",
        "http",
        SERVER_NAME,
        url,
        "--header",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([auth])
    .collect()
}

/// Shell-pasteable form: the header is double-quoted so `$ANYROUTER_API_KEY` expands.
fn claude_snippet(url: &str, token: Token) -> String {
    let auth = header_value(token, &format!("${KEY_ENV}"));
    format!(
        "claude mcp add --transport http {SERVER_NAME} {url} --header \"Authorization: {auth}\""
    )
}

fn codex_block(url: &str, token: Token) -> String {
    let auth = match token {
        Token::Env => format!("bearer_token_env_var = \"{KEY_ENV}\""),
        Token::Literal(key) => {
            format!("http_headers = {{ Authorization = \"Bearer {key}\" }}")
        }
    };
    format!("[mcp_servers.{SERVER_NAME}]\nurl = \"{url}\"\n{auth}\n")
}

fn opencode_entry(url: &str, token: Token) -> Value {
    json!({
        "type": "remote",
        "url": url,
        "enabled": true,
        "headers": { "Authorization": header_value(token, &format!("{{env:{KEY_ENV}}}")) },
    })
}

fn cursor_entry(url: &str, token: Token) -> Value {
    json!({
        "url": url,
        "headers": { "Authorization": header_value(token, &format!("${{env:{KEY_ENV}}}")) },
    })
}

/// Set `<parent>.anyrouter = entry` in a JSON config, keeping everything else.
fn merge_json(
    existing: Option<&str>,
    parent: &str,
    entry: Value,
    schema: Option<&str>,
) -> Result<String, String> {
    let mut root: Value = match existing.map(str::trim).filter(|s| !s.is_empty()) {
        None => json!({}),
        Some(s) => serde_json::from_str(s).map_err(|e| {
            format!("existing config is not plain JSON ({e}); fix it or use --print and add the entry by hand")
        })?,
    };
    let obj = root
        .as_object_mut()
        .ok_or("existing config is not a JSON object")?;
    if let Some(schema) = schema {
        obj.entry("$schema").or_insert_with(|| json!(schema));
    }
    let section = obj.entry(parent).or_insert_with(|| json!({}));
    section
        .as_object_mut()
        .ok_or_else(|| format!("\"{parent}\" in the existing config is not an object"))?
        .insert(SERVER_NAME.into(), entry);
    Ok(serde_json::to_string_pretty(&root).unwrap_or_default() + "\n")
}

/// Replace (or append) the `[mcp_servers.anyrouter]` table, leaving other lines alone.
pub(crate) fn merge_toml(existing: Option<&str>, block: &str) -> String {
    let header = format!("[mcp_servers.{SERVER_NAME}]");
    let existing = existing.unwrap_or("");
    let lines: Vec<&str> = existing.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim() == header) else {
        let mut out = existing.trim_end().to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        return out + block;
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |i| start + 1 + i);
    let mut out: Vec<String> = lines[..start].iter().map(|s| s.to_string()).collect();
    out.extend(block.lines().map(str::to_string));
    if end < lines.len() {
        out.push(String::new());
    }
    out.extend(lines[end..].iter().map(|s| s.to_string()));
    out.join("\n") + "\n"
}

/// Drop the `header` table (up to the next table) and the blank lines around it.
pub(crate) fn remove_toml_table(existing: &str, header: &str) -> String {
    let lines: Vec<&str> = existing.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim() == header) else {
        return existing.to_string();
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |i| start + 1 + i);
    let before = lines[..start].join("\n");
    let after = lines[end..].join("\n");
    let joined = match (before.trim_end().is_empty(), after.trim().is_empty()) {
        (true, true) => String::new(),
        (true, false) => after.trim_start().to_string(),
        (false, true) => before.trim_end().to_string(),
        (false, false) => format!("{}\n\n{}", before.trim_end(), after.trim_start()),
    };
    if joined.is_empty() {
        joined
    } else {
        joined + "\n"
    }
}

fn home(env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|k| env.get(*k).filter(|v| !v.trim().is_empty()))
        .map(PathBuf::from)
        .ok_or_else(|| "cannot find your home directory (HOME is not set)".to_string())
}

fn opencode_dir(env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    match env.get("XDG_CONFIG_HOME").filter(|v| !v.trim().is_empty()) {
        Some(x) => Ok(PathBuf::from(x).join("opencode")),
        None => Ok(home(env)?.join(".config").join("opencode")),
    }
}

fn codex_path(env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    match env.get("CODEX_HOME").filter(|v| !v.trim().is_empty()) {
        Some(c) => Ok(PathBuf::from(c).join("config.toml")),
        None => Ok(home(env)?.join(".codex").join("config.toml")),
    }
}

/// Write `content` unless the file already holds it. Returns whether it changed.
fn write_if_changed(path: &Path, existing: Option<&str>, content: &str) -> Result<bool, String> {
    if existing == Some(content) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(true)
}

fn read_existing(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn add(parsed: &ParsedArgs, env: &BTreeMap<String, String>, agent: &str) -> Result<i32, String> {
    if !AGENTS.contains(&agent) {
        return Err(crate::cmd::dispatch::hint(&format!(
            "Unknown agent \"{agent}\". Usage: {{bin}} mcp add <{}>",
            AGENTS.join("|")
        )));
    }
    let ctx = context(parsed, env);
    let url = mcp_url(&ctx.base);
    let print_only = parsed.flag_true("print");
    let token = if parsed.flag_true("show-key") {
        Token::Literal(ctx.key()?)
    } else {
        Token::Env
    };
    let json = parsed.flag_true("json");

    if agent == "claude" {
        if print_only {
            println!("{}", claude_snippet(&url, token));
            return Ok(0);
        }
        return run_claude(env, &url, ctx.key()?, json);
    }

    let (path, existing, content, snippet) = match agent {
        "codex" => {
            let path = codex_path(env)?;
            let existing = read_existing(&path)?;
            let block = codex_block(&url, token);
            let content = merge_toml(existing.as_deref(), &block);
            (path, existing, content, block)
        }
        "opencode" => {
            let dir = opencode_dir(env)?;
            let jsonc = dir.join("opencode.jsonc");
            let path = dir.join("opencode.json");
            if jsonc.exists() && !path.exists() && !print_only {
                return Err(format!(
                    "{} exists and may hold comments, which this command will not rewrite; use --print and add the entry by hand",
                    jsonc.display()
                ));
            }
            let entry = opencode_entry(&url, token);
            let snippet = pretty(&json!({ "mcp": { SERVER_NAME: entry.clone() } }));
            let existing = read_existing(&path)?;
            let content = merge_json(
                existing.as_deref(),
                "mcp",
                entry,
                Some("https://opencode.ai/config.json"),
            )
            .map_err(|e| format!("{}: {e}", path.display()))?;
            (path, existing, content, snippet)
        }
        _ => {
            let path = home(env)?.join(".cursor").join("mcp.json");
            let entry = cursor_entry(&url, token);
            let snippet = pretty(&json!({ "mcpServers": { SERVER_NAME: entry.clone() } }));
            let existing = read_existing(&path)?;
            let content = merge_json(existing.as_deref(), "mcpServers", entry, None)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            (path, existing, content, snippet)
        }
    };

    if print_only {
        print!("{snippet}");
        return Ok(0);
    }
    let changed = write_if_changed(&path, existing.as_deref(), &content)?;
    if json {
        print_json(
            &json!({ "agent": agent, "url": url, "path": path.display().to_string(), "changed": changed }),
        );
    } else if changed {
        println!("Wrote the {SERVER_NAME} MCP server to {}", path.display());
        if matches!(token, Token::Env) {
            println!("The config reads the key from ${KEY_ENV}; keep it exported.");
        }
    } else {
        println!(
            "{} already has the {SERVER_NAME} MCP server; nothing changed.",
            path.display()
        );
    }
    Ok(0)
}

/// `mcp auto [on|off]`: the machine-wide opt-out (`mcp.auto` in the config).
/// `on` clears the override so launches follow the workspace setting again.
fn auto(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    value: Option<&str>,
) -> Result<i32, String> {
    let path = crate::cmd::dispatch::config_path(parsed, env);
    let mut cfg = crate::key::load_config_if_present(&path).unwrap_or_default();
    if let Some(v) = value {
        cfg.mcp_auto = match v {
            "on" | "true" => None,
            "off" | "false" => Some(false),
            _ => return Err(crate::cmd::dispatch::hint("Usage: {bin} mcp auto [on|off]")),
        };
        crate::config::write_config(&cfg, &path)?;
    }
    let on = cfg.mcp_auto != Some(false);
    if parsed.flag_true("json") {
        print_json(&json!({ "auto": on }));
    } else if on {
        println!("MCP auto-connect: on (the gateway when your workspace turns it on in the dashboard → MCP, otherwise its artifact tools)");
    } else {
        println!(
            "MCP auto-connect: off on this machine. `{} mcp auto on` turns it back on.",
            crate::help::invoked_bin()
        );
    }
    Ok(0)
}

/// `mcp add grok` / `mcp remove grok`: the anyr-managed entry in Grok's config.
fn grok(parsed: &ParsedArgs, env: &BTreeMap<String, String>, add: bool) -> Result<i32, String> {
    use crate::mcp_wiring::{grok_paths, sync_grok, GrokAuth, GrokSync};
    let ctx = context(parsed, env);
    let path = crate::cmd::dispatch::config_path(parsed, env);
    let paths = grok_paths(env, &path).ok_or("cannot find Grok's home (set GROK_HOME)")?;
    let url = mcp_url(&ctx.base);
    let key = if add {
        ctx.key()?.to_string()
    } else {
        String::new()
    };
    let target = add.then_some((url.as_str(), GrokAuth::File(key.as_str())));
    let result = sync_grok(&paths, target)?;
    if !add {
        // The key file is anyr's; only this command takes it away.
        let _ = std::fs::remove_file(&paths.token);
    }
    let file = paths.config.display();
    let token = paths.token.display();
    let msg = match result {
        GrokSync::Written => format!(
            "Wrote the {SERVER_NAME} MCP server to {file}.\nGrok reads the key from an owner-only file, {token}; `{} mcp remove grok` deletes both. (`anyr grok` itself writes no key file: it passes the key by environment.)",
            crate::help::invoked_bin()
        ),
        GrokSync::Unchanged => {
            format!("{file} already has the {SERVER_NAME} MCP server; nothing changed.")
        }
        GrokSync::Removed => format!("Removed anyr's {SERVER_NAME} MCP server from {file}"),
        GrokSync::Absent => {
            format!("{file} has no anyr-managed {SERVER_NAME} MCP server; nothing changed.")
        }
        GrokSync::UserOwned => {
            format!("{file} has your own [mcp_servers.{SERVER_NAME}] entry; anyr leaves it alone.")
        }
    };
    if parsed.flag_true("json") {
        print_json(
            &json!({ "agent": "grok", "path": paths.config.display().to_string(), "result": format!("{result:?}").to_lowercase() }),
        );
    } else {
        println!("{msg}");
    }
    Ok(0)
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default() + "\n"
}

fn run_claude(
    env: &BTreeMap<String, String>,
    url: &str,
    key: &str,
    json: bool,
) -> Result<i32, String> {
    let bin = crate::spawn::env_command_path("claude", env).unwrap_or_else(|| "claude".into());
    let args = claude_args(url, key);
    let status = std::process::Command::new(&bin)
        .args(&args)
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "`{bin}` was not found on PATH. Install Claude Code, or run `{} mcp add claude --print` and paste the command.",
                    crate::help::invoked_bin()
                )
            } else {
                format!("cannot run `{bin}`: {e}")
            }
        })?;
    let code = status.code().unwrap_or(1);
    if json {
        print_json(&json!({ "agent": "claude", "url": url, "exit_code": code }));
    }
    Ok(code)
}

fn connections(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let ctx = context(parsed, env);
    let body = get(&ctx, "/v1/connections", true)?;
    if ctx.json {
        print_json(&body);
        return Ok(0);
    }
    let rows = crate::api::rows(&body);
    if rows.is_empty() {
        println!(
            "No MCP connections yet. Add one in the dashboard: https://anyrouter.dev/dashboard/mcp"
        );
        return Ok(0);
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let f = |k: &str| text(r.get(k));
            vec![
                f("slug"),
                f("name"),
                f("status"),
                f("auth_health"),
                f("tool_count"),
                f("recent_call_count"),
                f("recent_error_count"),
                f("server_url"),
            ]
        })
        .collect();
    print_table(
        &[
            "SLUG",
            "NAME",
            "STATUS",
            "AUTH",
            "TOOLS",
            "CALLS 24H",
            "ERRORS 24H",
            "URL",
        ],
        &table,
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://anyrouter.dev/api/v1/mcp";
    const KEY: &str = "sk-ar-v1-secret";

    #[test]
    fn url_follows_the_base_url() {
        assert_eq!(mcp_url("https://anyrouter.dev/api"), URL);
        assert_eq!(
            mcp_url("http://127.0.0.1:9/api/"),
            "http://127.0.0.1:9/api/v1/mcp"
        );
    }

    #[test]
    fn claude_snippet_uses_env_reference_and_redacts_the_key() {
        assert_eq!(
            claude_snippet(URL, Token::Env),
            format!("claude mcp add --transport http anyrouter {URL} --header \"Authorization: Bearer $ANYROUTER_API_KEY\"")
        );
        assert!(claude_snippet(URL, Token::Literal(KEY)).contains(&format!("Bearer {KEY}\"")));
    }

    #[test]
    fn codex_block_golden() {
        assert_eq!(
            codex_block(URL, Token::Env),
            format!("[mcp_servers.anyrouter]\nurl = \"{URL}\"\nbearer_token_env_var = \"ANYROUTER_API_KEY\"\n")
        );
        let lit = codex_block(URL, Token::Literal(KEY));
        assert!(lit.contains(&format!(
            "http_headers = {{ Authorization = \"Bearer {KEY}\" }}"
        )));
        assert!(!lit.contains("bearer_token_env_var"));
    }

    #[test]
    fn opencode_and_cursor_entries_golden() {
        assert_eq!(
            opencode_entry(URL, Token::Env),
            json!({"type":"remote","url":URL,"enabled":true,"headers":{"Authorization":"Bearer {env:ANYROUTER_API_KEY}"}})
        );
        assert_eq!(
            cursor_entry(URL, Token::Env),
            json!({"url":URL,"headers":{"Authorization":"Bearer ${env:ANYROUTER_API_KEY}"}})
        );
    }

    #[test]
    fn json_merge_keeps_user_content_and_is_idempotent() {
        let existing = r#"{"theme":"dark","mcp":{"other":{"type":"local","command":["x"]}}}"#;
        let once =
            merge_json(Some(existing), "mcp", opencode_entry(URL, Token::Env), None).unwrap();
        let v: Value = serde_json::from_str(&once).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcp"]["other"]["command"][0], "x");
        assert_eq!(v["mcp"]["anyrouter"]["url"], URL);
        let twice = merge_json(Some(&once), "mcp", opencode_entry(URL, Token::Env), None).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn json_merge_refuses_unparseable_or_wrong_shaped_config() {
        assert!(merge_json(Some("{ // comment\n}"), "mcp", json!({}), None).is_err());
        assert!(merge_json(Some("[]"), "mcp", json!({}), None).is_err());
        assert!(merge_json(Some(r#"{"mcp":"x"}"#), "mcp", json!({}), None).is_err());
    }

    #[test]
    fn toml_remove_drops_only_that_table() {
        let text = "model = \"m\"\n\n[mcp_servers.anyrouter]\nurl = \"u\"\n\n[profiles.x]\nmodel = \"y\"\n";
        assert_eq!(
            remove_toml_table(text, "[mcp_servers.anyrouter]"),
            "model = \"m\"\n\n[profiles.x]\nmodel = \"y\"\n"
        );
        assert_eq!(
            remove_toml_table(
                "[mcp_servers.anyrouter]\nurl = \"u\"\n",
                "[mcp_servers.anyrouter]"
            ),
            ""
        );
        assert_eq!(remove_toml_table(text, "[nope]"), text);
        let block = codex_block(URL, Token::Env);
        let base = "model = \"m\"\n";
        assert_eq!(
            remove_toml_table(&merge_toml(Some(base), &block), "[mcp_servers.anyrouter]"),
            base
        );
    }

    #[test]
    fn toml_merge_appends_replaces_and_is_idempotent() {
        let block = codex_block(URL, Token::Env);
        let fresh = merge_toml(None, &block);
        assert_eq!(fresh, block);

        let existing = "model = \"gpt-5\"\n\n[mcp_servers.other]\nurl = \"https://o\"\n";
        let added = merge_toml(Some(existing), &block);
        assert!(added.starts_with(existing.trim_end()));
        assert!(added.ends_with(&block));
        assert_eq!(merge_toml(Some(&added), &block), added);

        let stale =
            "[mcp_servers.anyrouter]\nurl = \"https://old\"\n\n[profiles.x]\nmodel = \"m\"\n";
        let fixed = merge_toml(Some(stale), &block);
        assert!(!fixed.contains("https://old"));
        assert!(fixed.contains("[profiles.x]\nmodel = \"m\""));
        assert_eq!(merge_toml(Some(&fixed), &block), fixed);
    }
}
