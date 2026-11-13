use std::collections::BTreeMap;
use std::io::IsTerminal;
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
    let path = config_path(parsed, env);
    let cfg = config_for_credential(&path, &parsed.flags, env)?;
    let name =
        get_string_flag(&parsed.flags, "profile").unwrap_or_else(|| cfg.active_profile.clone());
    let profile = profile_or_default(&cfg, &name, &parsed.flags, env)?;
    let base = resolve_base_url(&parsed.flags, Some(&profile));
    let api_key = resolve_api_key(&parsed.flags, env, Some(&profile))
        .ok_or_else(|| hint("No stored credential. Run \"{bin} login\" first."))?;
    Ok((path, cfg, base, api_key))
}

/// The account a key minted for `agent` lands in, and its stored credential.
/// `--key` and `ANYROUTER_API_KEY` are refused: they would mint in their own
/// account, and the env var would keep beating the new key at launch.
fn agent_mint_credential(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    agent: &str,
) -> Result<(std::path::PathBuf, Config, String, String, String), String> {
    for (set, what) in [
        (get_string_flag(&parsed.flags, "key").is_some(), "--key"),
        (
            env.get("ANYROUTER_API_KEY")
                .is_some_and(|k| !k.trim().is_empty() && !crate::key::is_dummy_key(k)),
            "ANYROUTER_API_KEY",
        ),
    ] {
        if set {
            return Err(hint(&format!(
                "{what} is set, so the new key would be minted in that key's account, not {agent}'s, and {what} also beats every agent key at launch. Drop it, or create without --agent: {{bin}} keys create"
            )));
        }
    }
    let path = config_path(parsed, env);
    let cfg = load_config_if_present(&path).ok_or_else(crate::key::no_key_error)?;
    let (account, bound) = account_for_agent(&cfg, &parsed.flags, env, agent);
    let profile = cfg.profiles.get(&account).ok_or_else(|| {
        format!(
            "Account \"{account}\" was not found. See: {} account list",
            crate::help::invoked_bin()
        )
    })?;
    let cred = resolve_api_key(&parsed.flags, env, Some(profile)).ok_or_else(|| {
        let whose = if bound { format!(" (bound to {agent})") } else { String::new() };
        crate::exit::tag(
            crate::exit::AUTH,
            hint(&format!(
                "Account \"{account}\"{whose} has no key. Sign in to it: {{bin}} login --profile {account}"
            )),
        )
    })?;
    let base = resolve_base_url(&parsed.flags, Some(profile));
    Ok((path, cfg, base, cred, account))
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

/// The newest active (not disabled) key named exactly `name`, for
/// `keys create --if-missing`.
fn active_key_named(rows: &[crate::http::RemoteKey], name: &str) -> Option<crate::http::RemoteKey> {
    crate::http::keys_newest_first(rows.to_vec())
        .into_iter()
        .find(|r| r.active && r.name == name)
}

/// `keys create --if-missing` found the key: its metadata, never a secret.
fn print_existing_key(parsed: &ParsedArgs, row: &crate::http::RemoteKey) {
    if parsed.flag_true("json") {
        let body = serde_json::json!({
            "name": row.name,
            "hash": row.hash,
            "id": row.id,
            "masked": row.masked,
            "created_at": row.created_at,
            "created": false,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into())
        );
    } else {
        // stderr: `KEY=$(… --quiet)` must never capture a sentence as a key.
        eprintln!(
            "Key \"{}\" already exists ({}); nothing created.",
            row.name,
            short_ref(row)
        );
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
pub(crate) fn set_agent_key(
    cfg: &mut Config,
    agent: &str,
    key: &str,
) -> Result<Option<String>, String> {
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
            // An existing key's secret cannot be fetched, so it could not be
            // saved for the agent.
            if agent.is_some() && parsed.flag_true("if-missing") {
                return Err(hint(
                    "--if-missing cannot save an existing key for --agent; its secret is shown only once. Use: {bin} keys create --agent <agent>",
                ));
            }
            let (path, mut cfg, base, cred, account) = match &agent {
                Some(agent) => agent_mint_credential(parsed, env, agent)?,
                None => {
                    let (path, cfg, base, cred) = keys_credential(parsed, env)?;
                    (path, cfg, base, cred, String::new())
                }
            };
            let name = get_string_flag(&parsed.flags, "name")
                .or_else(|| parsed.passthrough.get(1).cloned())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| default_key_name(agent.as_deref()));
            if parsed.flag_true("if-missing") {
                if let Some(row) = active_key_named(&fetch_keys(&base, &cred)?, &name) {
                    print_existing_key(parsed, &row);
                    return Ok(0);
                }
            }
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
            let hash = created
                .get("data")
                .and_then(|d| d.get("hash"))
                .and_then(|v| v.as_str())
                .or_else(|| created.get("hash").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let as_json = parsed.flag_true("json");
            let account_str = if account.is_empty() {
                cfg.active_profile.clone()
            } else {
                account.clone()
            };
            let print_created_json = |saved: bool| {
                let body = serde_json::json!({
                    "name": name,
                    "hash": hash,
                    "key": key,
                    "agent": agent,
                    "account": account_str,
                    "saved": saved,
                    "created": true,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into())
                );
            };
            if let Some(agent) = &agent {
                // Show the secret before saving: if the save fails, the key
                // already exists on the server and this is its only copy.
                // Under --json the key is in the printed JSON either way, so
                // a failed save never loses it.
                let bound = cfg.agent_binding(agent).and_then(|b| b.profile.clone());
                let replaced = cfg.agent_binding_mut(agent).api_key.replace(key.clone());
                let save_err = write_config(&cfg, &path).err();
                if as_json {
                    print_created_json(save_err.is_none());
                } else if parsed.flag_true("quiet") {
                    println!("{key}");
                } else {
                    println!("Created \"{name}\" in {account}:\n\n  {key}\n\nShown once.");
                }
                if let Some(err) = save_err {
                    return Err(format!(
                        "The key above was not saved for {agent}: {err}"
                    ));
                }
                if let Some(bound) = bound.filter(|b| *b != account) {
                    eprintln!(
                        "{}",
                        term::dim(&format!(
                            "{agent} is bound to {bound}, but this key is in {account}. Its own key wins at launch."
                        ))
                    );
                }
                if !parsed.flag_true("quiet") && !as_json {
                    print_agent_key_saved(agent, &key, replaced.as_deref());
                }
                return Ok(0);
            }
            if as_json {
                // No prompt under --json; save only on explicit --yes/--ok.
                let save = parsed.skip_confirm();
                let mut saved = false;
                if save {
                    let active = cfg.active_profile.clone();
                    if let Some(p) = cfg.profiles.get_mut(&active) {
                        p.api_key = Some(key.clone());
                    }
                    write_config(&cfg, &path)?;
                    saved = true;
                }
                print_created_json(saved);
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
            if raw != "-" {
                return Err(hint(
                    "Pass - and paste the key on stdin, so it stays out of your shell history: {bin} keys set --agent <agent> -",
                ));
            }
            if std::io::stdin().is_terminal() {
                eprint!("Paste the key for {agent}, then press Enter: ");
            }
            let (key, _) = key_from_flag("-", &mut std::io::stdin().lock(), &mut std::io::sink())?
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
        "rotate" => run_keys_rotate(parsed, env),
        "revoke" | "rm" | "delete" => {
            let hash = parsed.passthrough.get(1).cloned().ok_or_else(|| {
                hint("Usage: {bin} keys revoke <last4|id|hash>   (find keys: {bin} keys list)")
            })?;
            crate::api::require_yes_off_tty(parsed, &format!("Revoke key {hash}?"))?;
            let (_path, _cfg, base, cred) = keys_credential(parsed, env)?;
            let row = match resolve_key(&fetch_keys(&base, &cred)?, &hash) {
                Err(e) if crate::api::absent_ok(parsed, &e, "key", &hash) => return Ok(0),
                other => other?,
            };
            if !parsed.skip_confirm() {
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
            if let Err(e) = delete_key(&base, &cred, &row.hash) {
                // Revoked by someone else since the list: already gone.
                if crate::api::absent_ok(parsed, &e, "key", &hash) {
                    return Ok(0);
                }
                return Err(e);
            }
            println!(
                "{}  revoked \"{}\" ({})",
                term::ok("Revoked"),
                row.name,
                short_ref(&row)
            );
            Ok(0)
        }
        other => Err(crate::cmd::dispatch::hint(&format!(
            "Unknown keys subcommand \"{other}\". Try: list | create | set | unset | show | update | audit | rotate | revoke"
        ))),
    }
}

/// The server's key hash: SHA-256 of the secret, lower-case hex.
fn key_hash(key: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(key.as_bytes()))
}

/// Point every stored copy of the key whose hash is `hash` (a profile's key or
/// an agent's own key) at `new`. Returns where each copy was.
fn replace_stored_key(cfg: &mut Config, hash: &str, new: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let profiles = cfg
        .profiles
        .iter_mut()
        .map(|(n, p)| (format!("account {n}"), &mut p.api_key));
    let agents = cfg
        .agents
        .iter_mut()
        .map(|(n, b)| (format!("agent {n}"), &mut b.api_key));
    for (place, slot) in profiles.chain(agents) {
        if slot.as_deref().is_some_and(|k| key_hash(k.trim()) == hash) {
            *slot = Some(new.to_string());
            hits.push(place);
        }
    }
    hits
}

/// `keys rotate`: a new secret for an existing key (same id, name, limits and
/// spend). The old secret stops working at once, so every copy stored in
/// config.yaml is replaced. Needs a management key with write:llm-keys.
fn run_keys_rotate(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let needle = parsed.passthrough.get(1).cloned().ok_or_else(|| {
        hint("Usage: {bin} keys rotate <last4|id|hash> [--yes]   (find keys: {bin} keys list)")
    })?;
    crate::api::require_yes_off_tty(parsed, &format!("Rotate key {needle}?"))?;
    let (path, _cfg, base, cred) = keys_credential(parsed, env)?;
    let mut mctx = crate::api::management_ctx(
        &crate::api::context(parsed, env),
        "key rotations",
        "write:llm-keys",
    )?;
    mctx.base = base.clone();
    let row = resolve_key(&fetch_keys(&base, &cred)?, &needle)?;
    if !parsed.skip_confirm()
        && !term::confirm(&format!(
            "Rotate \"{}\" ({})? Its current secret stops working now.",
            row.name,
            short_ref(&row)
        ))
    {
        return Ok(1);
    }
    let check = |why: &str| {
        format!(
            "{why} The key may already be rotated, and its old secret dead. Check: {} keys list",
            crate::help::invoked_bin()
        )
    };
    let rotated = send(
        &mctx,
        "POST",
        &format!("/v1/keys/{}/rotate", row.hash),
        Some("{}"),
        true,
    )
    .map_err(
        |e| match crate::exit::refine(crate::exit::classify(&e).0, &e) {
            crate::exit::NETWORK => format!("{e}\n{}", check("The rotate request got no answer.")),
            _ => e,
        },
    )?;
    let key = rotated
        .get("key")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| check("The rotate response did not include the new secret."))?
        .to_string();
    let hash = key_hash(&key);
    // Save before printing: config.yaml is replaced atomically, and a stdout
    // that closes early must not leave the new secret both unshown and unsaved.
    let mut cfg = load_config_if_present(&path);
    let stored = cfg
        .as_mut()
        .map(|c| replace_stored_key(c, &row.hash, &key))
        .unwrap_or_default();
    let save_err = match (&cfg, stored.is_empty()) {
        (Some(c), false) => write_config(c, &path).err(),
        _ => None,
    };
    if parsed.flag_true("json") {
        let body = serde_json::json!({
            "name": row.name,
            "hash": hash,
            "key": key,
            "stored": stored,
            "saved": !stored.is_empty() && save_err.is_none(),
            "rotated": true,
        });
        crate::cmd::view::print_json(&body);
    } else if parsed.flag_true("quiet") {
        println!("{key}");
    } else {
        println!(
            "Rotated \"{}\":\n\n  {key}\n\nShown once — store it now. The old secret no longer works.",
            row.name
        );
    }
    if let Some(err) = save_err {
        return Err(format!(
            "The key above was not saved to config.yaml ({}): {err}",
            stored.join(", ")
        ));
    }
    if !parsed.flag_true("json") && !parsed.flag_true("quiet") && !stored.is_empty() {
        println!(
            "{}  {} now uses the new key",
            term::ok("Saved"),
            stored.join(", ")
        );
    }
    // A key from --key or ANYROUTER_API_KEY beats config.yaml and cannot be
    // updated from here.
    let outside =
        get_string_flag(&parsed.flags, "key").or_else(|| env.get("ANYROUTER_API_KEY").cloned());
    if outside.is_some_and(|k| key_hash(k.trim()) == row.hash) {
        eprintln!(
            "{}",
            term::dim("--key or ANYROUTER_API_KEY holds the old secret, which no longer works; replace it with the new key.")
        );
    }
    Ok(0)
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
