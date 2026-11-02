use std::collections::BTreeMap;
use std::path::Path;

use crate::config::write_config;
use crate::config::Config;
use crate::http::{delete_key, fetch_keys, is_active_key_row};
use crate::key::{
    account_for_agent, config_for_credential, key_from_flag, load_config_if_present, mask_api_key,
    profile_or_default, resolve_api_key, resolve_base_url, validate_api_key,
};
use crate::parse::{get_string_flag, ParsedArgs};
use crate::term;

use crate::api::{get, send, Ctx};
use crate::cmd::dispatch::{config_path, hint};
use crate::cmd::key_audit::{last4, revoke_ref, short_ref, suggest_removals};
use crate::cmd::key_scope::{render_show, scope_fields};
use crate::cmd::models::{flag_agent, known_agent};

pub(crate) fn keys_credential(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<(std::path::PathBuf, crate::config::Config, String, String), String> {
    keys_credential_for(parsed, env, None)
}

/// Like [`keys_credential`], but with `agent` it signs in as the account that
/// agent launches with, so a key minted for it lands in that account.
fn keys_credential_for(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    agent: Option<&str>,
) -> Result<(std::path::PathBuf, crate::config::Config, String, String), String> {
    let path = config_path(parsed, env);
    let cfg = config_for_credential(&path, &parsed.flags, env)?;
    let name = match agent {
        Some(agent) => account_for_agent(&cfg, &parsed.flags, env, agent).0,
        None => {
            get_string_flag(&parsed.flags, "profile").unwrap_or_else(|| cfg.active_profile.clone())
        }
    };
    let profile = profile_or_default(&cfg, &name, &parsed.flags, env)?;
    let base = resolve_base_url(&parsed.flags, Some(&profile));
    let api_key = resolve_api_key(&parsed.flags, env, Some(&profile))
        .ok_or_else(|| hint("No stored credential. Run \"{bin} login\" first."))?;
    Ok((path, cfg, base, api_key))
}

fn key_ctx(parsed: &ParsedArgs, base: &str, cred: &str) -> Ctx {
    Ctx {
        base: base.to_string(),
        key: Some(cred.to_string()),
        management_key: None,
        json: parsed.flag_true("json"),
    }
}

/// The one key matching `needle`: its id, its hash or a hash prefix, or the
/// last four characters of the key.
fn resolve_key(
    rows: &[crate::http::RemoteKey],
    needle: &str,
) -> Result<crate::http::RemoteKey, String> {
    let matches: Vec<_> = rows
        .iter()
        .filter(|r| {
            r.id == needle
                || r.hash == needle
                || r.hash.starts_with(needle)
                || (needle.len() == 4 && last4(&r.masked) == needle)
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(crate::exit::tag(
            crate::exit::NOT_FOUND,
            hint(&format!(
                "No key matches \"{needle}\". See: {{bin}} keys list"
            )),
        )),
        _ => Err(format!(
            "\"{needle}\" matches {} keys — use the key id or a longer hash prefix.",
            matches.len()
        )),
    }
}

pub(crate) fn default_key_name(agent: Option<&str>) -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "device".into());
    let short = host.split('.').next().unwrap_or("device");
    let name = match agent {
        Some(agent) => format!("cli-{agent}-{short}"),
        None => format!("cli-{short}"),
    };
    name.chars().take(40).collect()
}

/// `--agent <id>`, checked against the known coding agents.
fn agent_flag(parsed: &ParsedArgs) -> Result<Option<String>, String> {
    flag_agent(parsed).map(|a| known_agent(&a)).transpose()
}

/// Store `key` as `agent`'s own launch key. Returns the key it replaced.
fn set_agent_key(cfg: &mut Config, agent: &str, key: &str) -> Result<Option<String>, String> {
    validate_api_key(key)?;
    Ok(cfg
        .agent_binding_mut(agent)
        .api_key
        .replace(key.to_string()))
}

/// Drop `agent`'s own launch key. Returns the key it removed.
fn unset_agent_key(cfg: &mut Config, agent: &str) -> Option<String> {
    let old = cfg
        .agents
        .get_mut(agent)
        .and_then(|b| b.api_key.take())
        .filter(|k| !k.trim().is_empty());
    cfg.prune_empty_agents();
    old
}

fn print_agent_key_saved(agent: &str, key: &str, replaced: Option<&str>) {
    println!(
        "{}  {agent} key  {}",
        term::ok("Saved"),
        mask_api_key(Some(key))
    );
    if let Some(old) = replaced.filter(|old| *old != key) {
        println!(
            "{}",
            term::dim(&format!(
                "Replaced {}. It still works until you revoke it.",
                mask_api_key(Some(old))
            ))
        );
    }
}

pub(crate) fn run_keys(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let sub = parsed
        .passthrough
        .first()
        .map(String::as_str)
        .unwrap_or("list");
    // `ls` and `rm` / `delete` are hidden aliases, matching artifact, byok and
    // relay devices.
    match sub {
        "list" | "ls" => {
            let (_path, _cfg, base, api_key) = keys_credential(parsed, env)?;
            let rows = crate::http::keys_newest_first(fetch_keys(&base, &api_key)?);
            if parsed.flag_true("json") {
                let payload: Vec<_> = rows
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "name": r.name,
                            "hash": r.hash,
                            "masked": r.masked,
                            "created_at": r.created_at,
                            "last_used_at": r.last_used_at,
                            "active": r.active,
                            "current": is_active_key_row(&r.masked, Some(&api_key)),
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "[]".into())
                );
                return Ok(0);
            }
            if rows.is_empty() {
                println!(
                    "{}",
                    term::dim(&hint("No API keys. Create one: {bin} keys create"))
                );
                return Ok(0);
            }
            let name_w = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
            for r in &rows {
                let marker = if is_active_key_row(&r.masked, Some(&api_key)) {
                    "*"
                } else {
                    " "
                };
                let state = if r.active { "" } else { "  (disabled)" };
                println!(
                    "{marker} {:name_w$}  {}  {}{state}",
                    r.name,
                    r.masked,
                    r.last_used_at.as_deref().unwrap_or("never used")
                );
            }
            println!();
            println!(
                "{}",
                term::dim(&crate::help::with_bin(
                    "* = key this machine uses · new key: {bin} login · unused keys: {bin} keys audit"
                ))
            );
            Ok(0)
        }
        "create" => {
            let agent = agent_flag(parsed)?;
            let (path, mut cfg, base, cred) = keys_credential_for(parsed, env, agent.as_deref())?;
            let name = parsed
                .passthrough
                .get(1)
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| default_key_name(agent.as_deref()));
            let mut body = scope_fields(parsed, crate::civil::now_secs())?;
            body.remove("disabled");
            body.insert("name".into(), serde_json::Value::String(name.clone()));
            let ctx = key_ctx(parsed, &base, &cred);
            let created = send(
                &ctx,
                "POST",
                "/v1/keys",
                Some(&serde_json::Value::Object(body).to_string()),
                true,
            )?;
            let key = created
                .get("key")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or("Create-key response did not include a key.")?
                .to_string();
            if let Some(agent) = &agent {
                let replaced = set_agent_key(&mut cfg, agent, &key)?;
                write_config(&cfg, &path)?;
                if parsed.flag_true("quiet") {
                    println!("{key}");
                    return Ok(0);
                }
                println!("Created \"{name}\":\n\n  {key}\n\nShown once. {agent} now launches with it.");
                print_agent_key_saved(agent, &key, replaced.as_deref());
                return Ok(0);
            }
            if parsed.flag_true("quiet") {
                println!("{key}");
                return Ok(0);
            }
            println!("Created \"{name}\":\n\n  {key}\n\nShown once — store it now.");
            let save = parsed.skip_confirm()
                || (term::is_interactive()
                    && term::confirm("Use this key for the current profile?"));
            if save {
                let active = cfg.active_profile.clone();
                if let Some(p) = cfg.profiles.get_mut(&active) {
                    p.api_key = Some(key);
                }
                write_config(&cfg, &path)?;
                println!("{}  saved to config.yaml", term::ok("Saved"));
            }
            Ok(0)
        }
        "show" | "update" => {
            let hash = parsed.passthrough.get(1).cloned().ok_or_else(|| {
                hint(&format!(
                    "Usage: {{bin}} keys {sub} <hash>   (find hashes: {{bin}} keys list)"
                ))
            })?;
            let (_path, _cfg, base, cred) = keys_credential(parsed, env)?;
            let ctx = key_ctx(parsed, &base, &cred);
            let row = resolve_key(&fetch_keys(&base, &cred)?, &hash)?;
            let url = format!("/v1/keys/{}", row.hash);
            if sub == "update" {
                let body = scope_fields(parsed, crate::civil::now_secs())?;
                if body.is_empty() {
                    return Err(hint(
                        "Nothing to update. Pass --limit, --reset, --endpoints, --expires, --disable or --enable.",
                    ));
                }
                send(
                    &ctx,
                    "PATCH",
                    &url,
                    Some(&serde_json::Value::Object(body).to_string()),
                    true,
                )?;
                if !ctx.json {
                    println!(
                        "{}  updated \"{}\" ({})",
                        term::ok("Updated"),
                        row.name,
                        row.masked
                    );
                    return Ok(0);
                }
            }
            let key = get(&ctx, &url, true)?;
            if ctx.json {
                crate::cmd::view::print_json(&key);
            } else {
                print!("{}", render_show(&key));
            }
            Ok(0)
        }
        "set" => {
            let agent = agent_flag(parsed)?
                .ok_or_else(|| hint("Usage: {bin} keys set --agent <agent> -   (reads the key from stdin)"))?;
            let raw = parsed
                .passthrough
                .get(1)
                .cloned()
                .ok_or_else(|| hint("Usage: {bin} keys set --agent <agent> -   (reads the key from stdin)"))?;
            let (key, _) = key_from_flag(&raw, &mut std::io::stdin().lock(), &mut std::io::stderr())?
                .ok_or_else(|| hint("Usage: {bin} keys set --agent <agent> -   (reads the key from stdin)"))?;
            let path = config_path(parsed, env);
            let mut cfg = load_config_if_present(&path).unwrap_or_default();
            let replaced = set_agent_key(&mut cfg, &agent, &key)?;
            write_config(&cfg, &path)?;
            print_agent_key_saved(&agent, &key, replaced.as_deref());
            Ok(0)
        }
        "unset" => {
            let agent = agent_flag(parsed)?
                .ok_or_else(|| hint("Usage: {bin} keys unset --agent <agent>"))?;
            let path = config_path(parsed, env);
            let Some(mut cfg) = load_config_if_present(&path) else {
                println!("{agent} has no key of its own.");
                return Ok(0);
            };
            let Some(old) = unset_agent_key(&mut cfg, &agent) else {
                println!("{agent} has no key of its own.");
                return Ok(0);
            };
            write_config(&cfg, &path)?;
            println!(
                "{}  {agent} key  {}",
                term::ok("Removed"),
                mask_api_key(Some(&old))
            );
            let last4: String = old.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
            println!(
                "{}",
                term::dim(&crate::help::with_bin(&format!(
                    "{agent} now launches with its account key. The removed key still works until you revoke it: {{bin}} keys revoke {last4}"
                )))
            );
            Ok(0)
        }
        "use" => Err(hint(
            "This machine cannot fetch another key's secret. Get a fresh key: {bin} login, or paste one copied from the dashboard: {bin} login --key -",
        )),
        "audit" => run_keys_audit(parsed, env),
        "revoke" | "rm" | "delete" => {
            let hash = parsed.passthrough.get(1).cloned().ok_or_else(|| {
                hint("Usage: {bin} keys revoke <last4|id|hash>   (find keys: {bin} keys list)")
            })?;
            let (_path, _cfg, base, cred) = keys_credential(parsed, env)?;
            let row = resolve_key(&fetch_keys(&base, &cred)?, &hash)?;
            if !parsed.skip_confirm() {
                if !term::is_interactive() {
                    return Err(crate::cmd::dispatch::hint(
                        "Revoking a key is destructive; pass --yes to run non-interactively.",
                    ));
                }
                let this_machine = if is_active_key_row(&row.masked, Some(&cred)) {
                    " This is the key this machine uses."
                } else {
                    ""
                };
                if !term::confirm(&format!(
                    "Revoke \"{}\" ({})?{this_machine}",
                    row.name,
                    short_ref(&row)
                )) {
                    return Ok(1);
                }
            }
            delete_key(&base, &cred, &row.hash)?;
            println!(
                "{}  revoked \"{}\" ({})",
                term::ok("Revoked"),
                row.name,
                short_ref(&row)
            );
            Ok(0)
        }
        other => Err(crate::cmd::dispatch::hint(&format!(
            "Unknown keys subcommand \"{other}\". Try: list | create | set | unset | show | update | audit | revoke"
        ))),
    }
}

fn run_keys_audit(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let (_path, _cfg, base, cred) = keys_credential(parsed, env)?;
    let rows = fetch_keys(&base, &cred)?;
    let found = suggest_removals(&rows, Some(&cred), crate::civil::now_secs());
    if parsed.flag_true("json") {
        let payload: Vec<_> = found
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.row.id,
                    "name": s.row.name,
                    "last4": last4(&s.row.masked),
                    "created_at": s.row.created_at,
                    "last_used_at": s.row.last_used_at,
                    "requests_30d": s.row.requests_30d,
                    "reason": s.reason.code(),
                    "detail": s.reason.text(),
                })
            })
            .collect();
        crate::cmd::view::print_json(&serde_json::Value::Array(payload));
        return Ok(0);
    }
    if found.is_empty() {
        println!("No unused keys. Checked {} keys.", rows.len());
        return Ok(0);
    }
    println!(
        "{} of {} keys look unused. Nothing is removed until you revoke it.",
        found.len(),
        rows.len()
    );
    println!("{}", term::dim(&"─".repeat(40)));
    let name_w = found.iter().map(|s| s.row.name.len()).max().unwrap_or(4);
    for s in &found {
        println!(
            "{:name_w$}  {}  {}",
            s.row.name,
            short_ref(&s.row),
            term::dim(&s.reason.text())
        );
    }
    println!();
    println!(
        "{}",
        term::dim(&crate::help::with_bin(&format!(
            "Revoke one: {{bin}} keys revoke {}",
            revoke_ref(&found[0].row)
        )))
    );
    Ok(0)
}

pub(crate) fn stored_api_key(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
) -> Option<String> {
    let existing = load_config_if_present(path);
    let profile = existing
        .as_ref()
        .and_then(|c| c.profiles.get(&c.active_profile));
    resolve_api_key(&parsed.flags, env, profile)
}
