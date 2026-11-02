use std::collections::BTreeMap;

use crate::config::{set_active_profile, valid_account_name, write_config, DEFAULT_PROFILE};
use crate::key::{load_config_if_present, mask_api_key, no_key_error};
use crate::parse::{get_string_flag, FlagValue, ParsedArgs};
use crate::spawn::session_model_label;
use crate::term;

use crate::cmd::dispatch::{config_path, hint};
use crate::cmd::login::run_login;
use crate::cmd::models::{clear_agent_account, flag_agent, save_agent_account};

pub(crate) fn run_logout(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let path = config_path(parsed, env);
    let Some(mut cfg) = load_config_if_present(&path) else {
        return Err(no_key_error());
    };
    let name =
        get_string_flag(&parsed.flags, "profile").unwrap_or_else(|| cfg.active_profile.clone());
    let Some(profile) = cfg.profiles.get_mut(&name) else {
        return Err(format!("Account \"{name}\" was not found."));
    };
    profile.api_key = None;
    profile.management_key = None;
    write_config(&cfg, &path)?;
    if parsed.flag_true("json") {
        println!("{}", serde_json::json!({ "logged_out": name }));
        return Ok(0);
    }
    println!(
        "{}  cleared keys for {}",
        term::ok("Logged out"),
        term::accent(&name)
    );
    Ok(0)
}

pub(crate) fn run_account_use(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    name: &str,
) -> Result<i32, String> {
    let path = config_path(parsed, env);
    if let Some(agent) = flag_agent(parsed) {
        return save_agent_account(&path, &agent, name);
    }
    let cfg = load_config_if_present(&path).ok_or_else(no_key_error)?;
    let cfg = set_active_profile(cfg, name)?;
    write_config(&cfg, &path)?;
    println!(
        "{}  active account  {}",
        term::ok("Switched"),
        term::accent(name)
    );
    Ok(0)
}

pub(crate) fn run_account(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let path = config_path(parsed, env);
    let sub = parsed
        .passthrough
        .first()
        .map(String::as_str)
        .unwrap_or("list");
    // `ls` and `rm` are hidden aliases, matching the other list/remove verbs.
    match sub {
        "list" | "ls" => {
            let Some(cfg) = load_config_if_present(&path) else {
                return Err(no_key_error());
            };
            if parsed.flag_true("json") {
                let rows: Vec<_> = cfg
                    .profiles
                    .iter()
                    .map(|(name, p)| {
                        serde_json::json!({
                            "name": name,
                            "active": name == &cfg.active_profile,
                            "default_model": p.default_model(),
                            "has_key": p.api_key.as_ref().is_some_and(|s| !s.is_empty()),
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into())
                );
                return Ok(0);
            }
            for (name, profile) in &cfg.profiles {
                let marker = if name == &cfg.active_profile {
                    "*"
                } else {
                    " "
                };
                println!(
                    "{marker} {}  {}  {}",
                    term::accent(name),
                    term::model_id(&session_model_label(profile.default_model())),
                    mask_api_key(profile.api_key.as_deref())
                );
            }
            if cfg.profiles.is_empty() {
                println!("{}", term::dim(&hint("No accounts. Run: {bin} login")));
            }
            Ok(0)
        }
        "use" => {
            if parsed.flag_true("clear") {
                let agent = flag_agent(parsed).ok_or_else(|| {
                    hint("--clear needs an agent: {bin} account use --agent <agent> --clear")
                })?;
                return clear_agent_account(&path, &agent);
            }
            let name = parsed
                .passthrough
                .get(1)
                .cloned()
                .ok_or_else(|| hint("Usage: {bin} account use <name>"))?;
            run_account_use(parsed, env, &name)
        }
        "add" => {
            let name = parsed
                .passthrough
                .get(1)
                .cloned()
                .unwrap_or_else(|| DEFAULT_PROFILE.into());
            if !valid_account_name(&name) {
                return Err(format!(
                    "Invalid account name \"{name}\". Use letters, digits, \".\", \"_\", \"-\"."
                ));
            }
            let mut flags = parsed.flags.clone();
            flags.insert("profile".into(), FlagValue::Value(name));
            let next = ParsedArgs {
                command: "login".into(),
                flags,
                passthrough: Vec::new(),
                raw_from: None,
            };
            run_login(&next, env)
        }
        "rename" => {
            let old = parsed
                .passthrough
                .get(1)
                .cloned()
                .ok_or_else(|| hint("Usage: {bin} account rename <old> <new>"))?;
            let new = parsed
                .passthrough
                .get(2)
                .cloned()
                .ok_or_else(|| hint("Usage: {bin} account rename <old> <new>"))?;
            if !valid_account_name(&new) {
                return Err(format!(
                    "Invalid account name \"{new}\". Use letters, digits, \".\", \"_\", \"-\"."
                ));
            }
            let mut cfg = load_config_if_present(&path).ok_or_else(no_key_error)?;
            let profile = cfg.profiles.remove(&old).ok_or_else(|| {
                crate::exit::tag(
                    crate::exit::NOT_FOUND,
                    format!("Account \"{old}\" was not found."),
                )
            })?;
            if cfg.profiles.contains_key(&new) {
                return Err(format!("Account \"{new}\" already exists."));
            }
            cfg.profiles.insert(new.clone(), profile);
            if cfg.active_profile == old {
                cfg.active_profile = new.clone();
            }
            cfg.rebind_agents(&old, Some(&new));
            write_config(&cfg, &path)?;
            println!("{}  {old} → {}", term::ok("Renamed"), term::accent(&new));
            Ok(0)
        }
        "remove" | "rm" => {
            let name = parsed
                .passthrough
                .get(1)
                .cloned()
                .ok_or_else(|| hint("Usage: {bin} account remove <name>"))?;
            let mut cfg = load_config_if_present(&path).ok_or_else(no_key_error)?;
            if cfg.active_profile == name {
                return Err(hint(&format!(
                    "\"{name}\" is the active account. Switch first: {{bin}} account use <other>"
                )));
            }
            if cfg.profiles.remove(&name).is_none() {
                return Err(crate::exit::tag(
                    crate::exit::NOT_FOUND,
                    format!("Account \"{name}\" was not found."),
                ));
            }
            let unbound = cfg.rebind_agents(&name, None);
            write_config(&cfg, &path)?;
            println!("{}  removed {}", term::ok("Removed"), term::accent(&name));
            if !unbound.is_empty() {
                println!(
                    "{}",
                    term::dim(&format!(
                        "Unbound {}. An unbound agent uses the active account {}.",
                        unbound.join(", "),
                        cfg.active_profile
                    ))
                );
            }
            Ok(0)
        }
        other => Err(hint(&format!(
            "Unknown account subcommand \"{other}\". Try: list | use | add | rename | remove"
        ))),
    }
}
