//! `anyr agents list|get|set|unset|install`: each coding agent's model,
//! account, key and args. Same storage (`agents.<id>.*` in config.yaml) and
//! the same setters as `config set launcher.<agent>.*` and `keys set --agent`.
//! No network; a version comes only from the launch cache, never a spawn.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde_json::{json, Value};

use crate::cmd::dispatch::{config_path, hint, persist_tool_command, tool_command_for};
use crate::cmd::settings::{self, Setting};
use crate::config::{write_config, Config};
use crate::install::{agent_available, ensure_tool_installed, Install, KNOWN_AGENTS};
use crate::key::{load_config_if_present, resolve_launch_key, KeySource};
use crate::parse::{get_string_flag, ParsedArgs};
use crate::term;

const USAGE: &str = "Usage: {bin} agents list | get <agent> | set <agent> [--model M] [--profile P] [--key-stdin] [--args \"...\"] | unset <agent> [--model] [--profile] [--key] [--args] [--all] | install <agent> [--yes]";

pub(crate) fn run(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let path = config_path(parsed, env);
    let verb = parsed.passthrough.first().map_or("list", String::as_str);
    if verb == "list" {
        let cfg = load_config_if_present(&path);
        let rows: Vec<Value> = KNOWN_AGENTS
            .iter()
            .map(|(id, _)| row(&path, cfg.as_ref(), id, env))
            .collect();
        if parsed.flag_true("json") {
            crate::cmd::view::print_json(&Value::Array(rows));
        } else {
            rows.iter().for_each(print_line);
        }
        return Ok(0);
    }
    if !matches!(verb, "get" | "set" | "unset" | "install") {
        return Err(hint(&format!(
            "unknown agents subcommand \"{verb}\"\n{USAGE}"
        )));
    }
    let id = agent_id(parsed)?;
    match verb {
        "get" => show(parsed, &path, id, env),
        "set" => set(parsed, &path, id, env),
        "unset" => unset(parsed, &path, id, env),
        _ => install(parsed, &path, id, env),
    }
}

/// The agent named after the verb, as its canonical id. Unknown: exit 3 with
/// the ids as the hint.
fn agent_id(parsed: &ParsedArgs) -> Result<&'static str, String> {
    let raw = parsed.passthrough.get(1).ok_or_else(|| hint(USAGE))?;
    let id = crate::spawn::canonical_tool(raw);
    KNOWN_AGENTS
        .iter()
        .find(|(k, _)| *k == id)
        .map(|(k, _)| *k)
        .ok_or_else(|| {
            let ids: Vec<&str> = KNOWN_AGENTS.iter().map(|(k, _)| *k).collect();
            crate::exit::tag(
                crate::exit::NOT_FOUND,
                format!(
                    "Unknown agent \"{}\".\n{} agents: {}",
                    crate::key::mask_secrets(raw),
                    term::dim("hint:"),
                    ids.join(", ")
                ),
            )
        })
}

/// One agent's row for `list` / `get` (and the `set` / `unset` result).
/// Never carries a key, masked or not: `key_source` names where it comes from.
fn row(path: &Path, cfg: Option<&Config>, id: &str, env: &BTreeMap<String, String>) -> Value {
    let command =
        crate::spawn::env_command_path(id, env).unwrap_or_else(|| tool_command_for(path, id));
    let installed = agent_available(id, &command, env);
    let resolved = installed
        .then(|| crate::install::resolve_executable(&command))
        .flatten();
    let version = resolved.as_ref().and_then(|p| cached_major(path, p));
    let binding = cfg.and_then(|c| c.agent_binding(id));
    let key_source = match resolve_launch_key(&HashMap::new(), env, cfg, id).map(|(_, s)| s) {
        Some(KeySource::AgentKey) => "agent",
        Some(KeySource::Env | KeySource::Flag) => "env",
        Some(_) => "profile",
        None => "none",
    };
    let name = KNOWN_AGENTS
        .iter()
        .find(|(k, _)| *k == id)
        .map_or(id, |(_, n)| *n);
    json!({
        "id": id,
        "name": name,
        "installed": installed,
        "path": resolved,
        "version": version,
        "profile": binding.and_then(|b| b.profile.clone()),
        "model": binding.and_then(|b| b.default_model.clone()),
        "key_source": key_source,
        "args": binding.map(|b| b.args.clone()).unwrap_or_default(),
        "last": cfg.and_then(|c| c.last_tool.as_deref()).map(crate::spawn::canonical_tool) == Some(id),
    })
}

/// The major version a launch already probed (`tool-versions.json`), if any.
fn cached_major(path: &Path, binary: &str) -> Option<u64> {
    let text = std::fs::read_to_string(path.parent()?.join("tool-versions.json")).ok()?;
    serde_json::from_str::<Value>(&text).ok()?[binary]["major"].as_u64()
}

fn show(
    parsed: &ParsedArgs,
    path: &Path,
    id: &str,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let r = row(path, load_config_if_present(path).as_ref(), id, env);
    if parsed.flag_true("json") {
        crate::cmd::view::print_json(&r);
        return Ok(0);
    }
    print_line(&r);
    Ok(0)
}

/// One agent on one line: id, installed or not, then what is saved.
fn print_line(r: &Value) {
    let mut line = format!(
        "{:<9}{}",
        r["id"].as_str().unwrap_or_default(),
        if r["installed"] == true {
            "installed"
        } else {
            "missing"
        }
    );
    for k in ["model", "profile", "key_source", "path"] {
        if let Some(v) = r[k].as_str() {
            line += &format!("  {k} {v}");
        }
    }
    for a in r["args"].as_array().into_iter().flatten() {
        line += &format!(" {}", a.as_str().unwrap_or_default());
    }
    println!("{line}");
}

/// Write only when something changed, so a repeat leaves the file as it was.
fn save(
    parsed: &ParsedArgs,
    path: &Path,
    id: &str,
    env: &BTreeMap<String, String>,
    before: &Config,
    cfg: &mut Config,
) -> Result<i32, String> {
    cfg.prune_empty_agents();
    // The canonical text, not a derived `==`: the same answer, less code.
    if crate::config::serialize_config(cfg) != crate::config::serialize_config(before) {
        write_config(cfg, path)?;
    }
    show(parsed, path, id, env)
}

fn set(
    parsed: &ParsedArgs,
    path: &Path,
    id: &str,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    if parsed.flags.contains_key("key") {
        return Err(hint(
            "agents set takes the key on stdin, never in argv: {bin} agents set <agent> --key-stdin",
        ));
    }
    let model = get_string_flag(&parsed.flags, "model");
    let profile = get_string_flag(&parsed.flags, "profile");
    let args = get_string_flag(&parsed.flags, "args");
    let key_stdin = parsed.flag_true("key-stdin");
    if model.is_none() && profile.is_none() && args.is_none() && !key_stdin {
        return Err(hint(USAGE));
    }
    // An unreadable file is an error, never an empty config written over it.
    let before = if path.exists() {
        crate::config::read_config(path)?
    } else {
        Config::default()
    };
    let mut cfg = before.clone();
    if let Some(p) = &profile {
        if !cfg.profiles.contains_key(p) {
            return Err(crate::exit::tag(
                crate::exit::NOT_FOUND,
                crate::help::with_bin(&format!(
                    "Account \"{p}\" was not found.\nhint: {{bin}} auth list"
                )),
            ));
        }
        cfg.agent_binding_mut(id).profile = Some(p.clone());
    }
    if let Some(m) = &model {
        settings::set(
            &mut cfg,
            &format!("launcher.{id}.model"),
            &Setting::Model(id.into()),
            Some(m),
        )?;
    }
    if let Some(a) = &args {
        settings::set(
            &mut cfg,
            &format!("launcher.{id}.args"),
            &Setting::Args(id.into()),
            Some(a),
        )?;
    }
    if key_stdin {
        let (key, _) =
            crate::key::key_from_flag("-", &mut std::io::stdin().lock(), &mut std::io::sink())?
                .ok_or_else(|| hint(USAGE))?;
        crate::cmd::keys::set_agent_key(&mut cfg, id, &key)?;
    }
    save(parsed, path, id, env, &before, &mut cfg)
}

fn unset(
    parsed: &ParsedArgs,
    path: &Path,
    id: &str,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let all = parsed.flag_true("all");
    let fields = ["model", "profile", "key", "args"].map(|f| all || parsed.flag_true(f));
    if !fields.contains(&true) {
        return Err(hint(USAGE));
    }
    let Some(before) = load_config_if_present(path) else {
        return show(parsed, path, id, env);
    };
    let mut cfg = before.clone();
    if all {
        cfg.agents.remove(id);
    } else {
        let [model, profile, key, args] = fields;
        if let Some(b) = cfg.agents.get_mut(id) {
            if model {
                b.default_model = None;
            }
            if profile {
                b.profile = None;
            }
            if key {
                b.api_key = None;
            }
            if args {
                b.args.clear();
            }
        }
    }
    save(parsed, path, id, env, &before, &mut cfg)
}

fn install(
    parsed: &ParsedArgs,
    path: &Path,
    id: &str,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let command =
        crate::spawn::env_command_path(id, env).unwrap_or_else(|| tool_command_for(path, id));
    if !agent_available(id, &command, env) {
        let how = crate::install::tool_hint(id).map_or("", |h| h.install);
        crate::api::confirm(parsed, &format!("Install {id} with `{how}`?"))?;
        let resolved = ensure_tool_installed(id, &command, Install::Yes)?;
        persist_tool_command(path, id, &resolved)?;
    }
    show(parsed, path, id, env)
}
