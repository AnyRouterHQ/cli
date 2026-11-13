use std::collections::BTreeMap;

use crate::help::command_help;
use crate::key::{
    config_for_credential, load_config_if_present, mask_api_key, no_key_error, profile_or_default,
    resolve_api_key,
};
use crate::parse::{get_string_flag, ParsedArgs};
use crate::term;

use crate::cmd::account::{run_account_use, run_logout};
use crate::cmd::dispatch::{config_path, hint, shift_passthrough};
use crate::cmd::login::run_login;
use crate::cmd::status::run_whoami;

pub(crate) fn run_auth(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let sub = parsed
        .passthrough
        .first()
        .map(String::as_str)
        .filter(|s| *s != "-h" && *s != "--help")
        .unwrap_or("");
    if sub.is_empty() {
        print!("{}", command_help("auth").unwrap_or_default());
        return Ok(0);
    }
    let rest = shift_passthrough(parsed);
    match sub {
        "login" | "setup" => run_login(&rest, env),
        "logout" => run_logout(&rest, env),
        "status" if rest.flag_true("local") => run_auth_status_local(&rest, env),
        "status" => run_whoami(&rest, env),
        // The account verbs, on the one account noun: the same code.
        "list" | "add" | "rename" | "remove" => crate::cmd::account::run_account(parsed, env),
        "token" => run_auth_token(&rest, env),
        "switch" => run_auth_switch(&rest, env),
        other => Err(format!(
            "{}unknown command \"{other}\" for \"{} auth\"\n\n{}",
            crate::cmd::dispatch::USAGE,
            crate::help::invoked_bin(),
            command_help("auth").unwrap_or_default()
        )),
    }
}

/// One row per saved account, from config.yaml only: never the key itself.
/// `key_source` is `env` for the active account when `ANYROUTER_API_KEY`
/// overrides it, `config` when the file holds a key, else `none`.
pub(crate) fn profiles_json(
    cfg: &crate::config::Config,
    env: &BTreeMap<String, String>,
) -> serde_json::Value {
    let env_key = env
        .get("ANYROUTER_API_KEY")
        .is_some_and(|k| !k.trim().is_empty());
    let has = |k: &Option<String>| k.as_deref().is_some_and(|k| !k.trim().is_empty());
    serde_json::Value::Array(
        cfg.profiles
            .iter()
            .map(|(name, p)| {
                let active = *name == cfg.active_profile;
                let source = if active && env_key {
                    "env"
                } else if has(&p.api_key) {
                    "config"
                } else {
                    "none"
                };
                serde_json::json!({
                    "name": name,
                    "active": active,
                    "key_masked": mask_api_key(p.api_key.as_deref()),
                    "key_source": source,
                    "has_management_key": has(&p.management_key),
                    "default_model": p.default_model(),
                })
            })
            .collect(),
    )
}

/// `auth status --local`: every saved account, no network. Exit 0 unless the
/// config file exists and cannot be read.
fn run_auth_status_local(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let path = config_path(parsed, env);
    let cfg = if path.exists() {
        crate::config::read_config(&path)?
    } else {
        crate::config::Config::default()
    };
    let profiles = profiles_json(&cfg, env);
    if parsed.flag_true("json") {
        crate::cmd::view::print_json(&serde_json::json!({
            "active": cfg.active_profile,
            "profiles": profiles,
        }));
        return Ok(0);
    }
    for r in profiles.as_array().into_iter().flatten() {
        println!(
            "{} {}  {}  {}  {}",
            if r["active"] == true { "*" } else { " " },
            r["name"].as_str().unwrap_or_default(),
            r["key_masked"].as_str().unwrap_or_default(),
            r["key_source"].as_str().unwrap_or_default(),
            r["default_model"].as_str().unwrap_or_default(),
        );
    }
    Ok(0)
}

pub(crate) fn run_auth_token(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let path = config_path(parsed, env);
    let cfg = config_for_credential(&path, &parsed.flags, env)?;
    let name = get_string_flag(&parsed.flags, "profile")
        .or_else(|| env.get("ANYROUTER_PROFILE").cloned())
        .unwrap_or_else(|| cfg.active_profile.clone());
    let profile = profile_or_default(&cfg, &name, &parsed.flags, env)
        .map_err(|_| format!("Account \"{name}\" was not found."))?;
    let key = resolve_api_key(&parsed.flags, env, Some(&profile)).ok_or_else(no_key_error)?;
    if parsed.flag_true("json") {
        let value = if parsed.flag_true("masked") {
            mask_api_key(Some(&key))
        } else {
            key.clone()
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "account": name,
                "token": value,
            }))
            .unwrap_or_else(|_| "{}".into())
        );
        return Ok(0);
    }
    if parsed.flag_true("masked") {
        println!("{}", mask_api_key(Some(&key)));
    } else {
        println!("{key}");
    }
    Ok(0)
}

pub(crate) fn run_auth_switch(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let path = config_path(parsed, env);
    let cfg = load_config_if_present(&path).ok_or_else(no_key_error)?;
    let names: Vec<String> = cfg.profiles.keys().cloned().collect();
    let name = parsed
        .passthrough
        .first()
        .cloned()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if !term::is_interactive() || names.is_empty() {
                return None;
            }
            let current = names.iter().position(|n| n == &cfg.active_profile);
            term::pick("Active account", &names, current)
                .ok()
                .map(|i| names[i].clone())
        })
        .ok_or_else(|| hint("Usage: {bin} auth switch <account>"))?;
    run_account_use(parsed, env, &name)
}
