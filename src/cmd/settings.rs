//! `anyr config get|set|unset <key>`: launcher and update settings by one
//! dotted name each. Values live in the existing `config.yaml` fields, which
//! the TypeScript CLI also reads, so no field is renamed.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::config::{Config, YamlValue};
use crate::parse::ParsedArgs;

use crate::cmd::dispatch::{config_path, hint};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tier {
    Haiku,
    Sonnet,
    Opus,
    Fable,
}

const TIERS: &[(&str, Tier)] = &[
    ("haiku", Tier::Haiku),
    ("sonnet", Tier::Sonnet),
    ("opus", Tier::Opus),
    ("fable", Tier::Fable),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Setting {
    /// `launcher.agent` → `profiles.<active>.default_tool`
    DefaultAgent,
    /// `launcher.<agent>.model` → `agents.<id>.default_model`
    Model(String),
    /// `launcher.<agent>.args` → `agents.<id>.args`
    Args(String),
    /// `launcher.<agent>.yolo` → `tools.<id>.yolo`
    Yolo(String),
    /// `launcher.claude.<tier>` → `profiles.<active>.claude_<tier>`
    ClaudeTier(Tier),
    /// `launcher.mcp_auto` → `mcp.auto`
    McpAuto,
    /// `update.auto` → `auto_update`
    UpdateAuto,
    /// `update.channel` → `channel`
    UpdateChannel,
}

const KEYS_HELP: &str = "launcher.agent, launcher.<agent>.model, launcher.<agent>.args, \
launcher.<agent>.yolo, launcher.claude.{haiku,sonnet,opus,fable}, launcher.mcp_auto, \
update.auto, update.channel";

fn known_agent(id: &str) -> Option<String> {
    let id = crate::spawn::canonical_tool(id);
    crate::install::KNOWN_AGENTS
        .iter()
        .any(|(known, _)| *known == id)
        .then(|| id.to_string())
}

fn agent_ids() -> String {
    crate::install::KNOWN_AGENTS
        .iter()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn parse_key(key: &str) -> Result<Setting, String> {
    let unknown = || hint(&format!("unknown setting \"{key}\"; settings: {KEYS_HELP}"));
    let parts: Vec<&str> = key.split('.').collect();
    Ok(match parts.as_slice() {
        ["launcher", "agent"] => Setting::DefaultAgent,
        ["launcher", "mcp_auto"] => Setting::McpAuto,
        ["update", "auto"] => Setting::UpdateAuto,
        ["update", "channel"] => Setting::UpdateChannel,
        ["launcher", "claude", tier] if TIERS.iter().any(|(n, _)| n == tier) => {
            Setting::ClaudeTier(TIERS.iter().find(|(n, _)| n == tier).unwrap().1)
        }
        ["launcher", agent, field] => {
            let id = known_agent(agent).ok_or_else(|| {
                hint(&format!(
                    "unknown agent \"{agent}\" in \"{key}\"; agents: {}",
                    agent_ids()
                ))
            })?;
            match *field {
                "model" => Setting::Model(id),
                "args" => Setting::Args(id),
                "yolo" => Setting::Yolo(id),
                _ => return Err(unknown()),
            }
        }
        _ => return Err(unknown()),
    })
}

fn tier_slot(p: &mut crate::config::Profile, tier: Tier) -> &mut Option<String> {
    match tier {
        Tier::Haiku => &mut p.claude_haiku,
        Tier::Sonnet => &mut p.claude_sonnet,
        Tier::Opus => &mut p.claude_opus,
        Tier::Fable => &mut p.claude_fable,
    }
}

fn nonempty(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The stored value, or None when the setting is not set on this machine.
pub(crate) fn get(cfg: &Config, setting: &Setting) -> Option<String> {
    let profile = cfg.profiles.get(&cfg.active_profile);
    match setting {
        Setting::DefaultAgent => nonempty(profile.and_then(|p| p.default_tool.as_deref())),
        Setting::Model(id) => nonempty(
            cfg.agent_binding(id)
                .and_then(|b| b.default_model.as_deref()),
        ),
        Setting::Args(id) => cfg
            .agent_binding(id)
            .map(|b| b.args.join(" "))
            .filter(|s| !s.is_empty()),
        Setting::Yolo(id) => cfg
            .tools
            .get(id.as_str())
            .filter(|t| t.extra.contains_key("yolo"))
            .map(|t| t.extra_flag("yolo").to_string()),
        Setting::ClaudeTier(tier) => {
            let mut p = profile.cloned()?;
            tier_slot(&mut p, *tier)
                .clone()
                .and_then(|v| nonempty(Some(&v)))
        }
        Setting::McpAuto => cfg.mcp_auto.map(|on| on.to_string()),
        Setting::UpdateAuto => cfg.auto_update.map(|on| on.to_string()),
        Setting::UpdateChannel => nonempty(cfg.channel.as_deref()),
    }
}

fn parse_bool(key: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        _ => Err(hint(&format!("{key} takes true or false, not \"{value}\""))),
    }
}

/// Store `value`, or clear the setting when `value` is None.
pub(crate) fn set(
    cfg: &mut Config,
    key: &str,
    setting: &Setting,
    value: Option<&str>,
) -> Result<(), String> {
    let value = value.map(str::trim);
    let active = cfg.active_profile.clone();
    match setting {
        Setting::DefaultAgent => {
            let agent = match value {
                Some(v) => Some(known_agent(v).ok_or_else(|| {
                    hint(&format!("unknown agent \"{v}\"; agents: {}", agent_ids()))
                })?),
                None => None,
            };
            cfg.profiles.entry(active).or_default().default_tool = agent;
        }
        Setting::Model(id) => {
            let model = value
                .map(crate::spawn::catalog_model_id)
                .filter(|m| !m.is_empty() && !crate::spawn::is_auto_model(m));
            cfg.agent_binding_mut(id).default_model = model;
        }
        Setting::Args(id) => {
            cfg.agent_binding_mut(id).args =
                value.map(crate::config::split_args).unwrap_or_default();
        }
        Setting::Yolo(id) => {
            let on = value.map(|v| parse_bool(key, v)).transpose()?;
            let tool = cfg.tools.entry(id.clone()).or_insert_with(|| {
                crate::spawn::resolve_tool(None, id).unwrap_or_else(|_| Default::default())
            });
            match on {
                Some(on) => {
                    tool.extra.insert("yolo".into(), YamlValue::Bool(on));
                }
                None => {
                    tool.extra.remove("yolo");
                }
            }
        }
        Setting::ClaudeTier(tier) => {
            let profile = cfg.profiles.entry(active).or_default();
            *tier_slot(profile, *tier) = value.filter(|v| !v.is_empty()).map(str::to_string);
        }
        Setting::McpAuto => cfg.mcp_auto = value.map(|v| parse_bool(key, v)).transpose()?,
        Setting::UpdateAuto => cfg.auto_update = value.map(|v| parse_bool(key, v)).transpose()?,
        Setting::UpdateChannel => {
            cfg.channel = match value {
                Some(ch @ ("stable" | "beta")) => Some(ch.to_string()),
                Some(other) => {
                    return Err(hint(&format!(
                        "update.channel is stable or beta, not \"{other}\""
                    )))
                }
                None => None,
            }
        }
    }
    cfg.prune_empty_agents();
    Ok(())
}

/// Where the model an agent launches with comes from.
pub(crate) fn launch_model(cfg: &Config, agent: &str) -> (String, &'static str) {
    if let Some(m) = get(cfg, &Setting::Model(agent.to_string())) {
        return (m, "agent");
    }
    let profile = cfg.profiles.get(&cfg.active_profile);
    match profile.and_then(|p| nonempty(p.default_model.as_deref())) {
        Some(m) => (m, "account"),
        None => (crate::config::DEFAULT_MODEL.to_string(), "default"),
    }
}

/// The launcher section of `anyr config --json`.
pub(crate) fn launcher_json(cfg: &Config) -> Value {
    let mut agents = serde_json::Map::new();
    // `extend` inserts one by one; `collect` would compile in a sort.
    agents.extend(crate::install::KNOWN_AGENTS.iter().map(|(id, _)| {
        let (model, source) = launch_model(cfg, id);
        let args = cfg
            .agent_binding(id)
            .map(|b| b.args.clone())
            .unwrap_or_default();
        let yolo = get(cfg, &Setting::Yolo(id.to_string())).as_deref() == Some("true");
        (
            id.to_string(),
            json!({ "model": model, "model_source": source, "args": args, "yolo": yolo }),
        )
    }));
    let profile = cfg
        .profiles
        .get(&cfg.active_profile)
        .cloned()
        .unwrap_or_default();
    json!({
        "agent": get(cfg, &Setting::DefaultAgent),
        "last_agent": cfg.last_tool,
        "mcp_auto": cfg.mcp_auto,
        "agents": agents,
        "claude": {
            "haiku": profile.claude_haiku(),
            "sonnet": profile.claude_sonnet(),
            "opus": profile.claude_opus(),
            "fable": profile.claude_fable(),
        },
    })
}

/// Text rows for `anyr config`: agents with something set, plus the default.
pub(crate) fn launcher_rows(cfg: &Config) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(agent) = get(cfg, &Setting::DefaultAgent) {
        rows.push(("launch".to_string(), agent));
    }
    for (id, _) in crate::install::KNOWN_AGENTS {
        let (model, source) = launch_model(cfg, id);
        let args = get(cfg, &Setting::Args(id.to_string()));
        let yolo = get(cfg, &Setting::Yolo(id.to_string())).as_deref() == Some("true");
        if source != "agent" && args.is_none() && !yolo {
            continue;
        }
        let mut detail = crate::spawn::session_model_label(&model);
        if let Some(args) = args {
            detail.push_str(&format!("  ·  {args}"));
        }
        if yolo {
            detail.push_str("  ·  yolo");
        }
        rows.push((id.to_string(), detail));
    }
    rows
}

/// `anyr config get <key>`, `set <key> <value>`, `unset <key>`.
pub(crate) fn run(
    verb: &str,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let bin_usage = match verb {
        "set" => "Usage: {bin} config set <key> <value>",
        _ => "Usage: {bin} config get|unset <key>",
    };
    let key = parsed
        .passthrough
        .get(1)
        .cloned()
        .ok_or_else(|| hint(bin_usage))?;
    let setting = parse_key(&key)?;
    let path = config_path(parsed, env);
    let json_out = parsed.flag_true("json");
    if verb == "get" {
        let cfg = crate::key::load_config_if_present(&path).unwrap_or_default();
        let value = get(&cfg, &setting);
        if json_out {
            println!("{}", json!({ "key": key, "value": value }));
            return Ok(0);
        }
        return match value {
            Some(v) => {
                println!("{v}");
                Ok(0)
            }
            None => Ok(1),
        };
    }
    let value = match verb {
        "set" => {
            let rest = &parsed.passthrough[2..];
            if rest.is_empty() {
                return Err(hint(bin_usage));
            }
            Some(rest.join(" "))
        }
        _ => None,
    };
    let mut cfg = crate::key::load_config_if_present(&path).unwrap_or_default();
    set(&mut cfg, &key, &setting, value.as_deref())?;
    crate::config::write_config(&cfg, &path)?;
    let stored = get(&cfg, &setting);
    if json_out {
        println!("{}", json!({ "key": key, "value": stored }));
    } else {
        match &stored {
            Some(v) => println!("{key} = {v}"),
            None => println!("{key} unset"),
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn every_documented_key_parses_and_unknown_ones_are_usage_errors() {
        assert_eq!(parse_key("launcher.agent"), Ok(Setting::DefaultAgent));
        assert_eq!(
            parse_key("launcher.cc.model"),
            Ok(Setting::Model("claude".into()))
        );
        assert_eq!(
            parse_key("launcher.pi.args"),
            Ok(Setting::Args("pi".into()))
        );
        assert_eq!(
            parse_key("launcher.codex.yolo"),
            Ok(Setting::Yolo("codex".into()))
        );
        assert_eq!(
            parse_key("launcher.claude.opus"),
            Ok(Setting::ClaudeTier(Tier::Opus))
        );
        assert_eq!(parse_key("launcher.mcp_auto"), Ok(Setting::McpAuto));
        assert_eq!(parse_key("update.channel"), Ok(Setting::UpdateChannel));
        for bad in [
            "launcher",
            "launcher.vim.model",
            "launcher.claude.colour",
            "mcp.auto",
        ] {
            let err = parse_key(bad).unwrap_err();
            assert!(err.starts_with(crate::cmd::dispatch::USAGE), "{bad}: {err}");
        }
    }

    #[test]
    fn launcher_settings_land_in_the_existing_config_fields() {
        let mut c = cfg();
        let put = |c: &mut Config, k: &str, v: Option<&str>| {
            set(c, k, &parse_key(k).unwrap(), v).unwrap()
        };
        put(&mut c, "launcher.agent", Some("codex"));
        put(
            &mut c,
            "launcher.claude.model",
            Some("minimax/minimax-m3.1-flash"),
        );
        put(
            &mut c,
            "launcher.claude.args",
            Some("--verbose  --add-dir /tmp"),
        );
        put(&mut c, "launcher.codex.yolo", Some("on"));
        put(&mut c, "launcher.claude.haiku", Some("anyrouter/free"));
        put(&mut c, "launcher.mcp_auto", Some("off"));
        put(&mut c, "update.channel", Some("beta"));
        let p = &c.profiles["default"];
        assert_eq!(p.default_tool.as_deref(), Some("codex"));
        assert_eq!(p.claude_haiku.as_deref(), Some("anyrouter/free"));
        let claude = &c.agents["claude"];
        assert_eq!(
            claude.default_model.as_deref(),
            Some("minimax/minimax-m3.1-flash")
        );
        assert_eq!(claude.args, vec!["--verbose", "--add-dir", "/tmp"]);
        assert!(c.tools["codex"].extra_flag("yolo"));
        assert_eq!(
            (c.mcp_auto, c.channel.as_deref()),
            (Some(false), Some("beta"))
        );

        let yaml = crate::config::serialize_config(&c);
        let back = crate::config::parse_config(&yaml);
        assert_eq!(back.agents["claude"].args, claude.args, "{yaml}");
        assert_eq!(
            get(&back, &Setting::Yolo("codex".into())).as_deref(),
            Some("true")
        );
    }

    #[test]
    fn unset_clears_and_auto_model_means_no_pin() {
        let mut c = cfg();
        let key = "launcher.pi.model";
        set(&mut c, key, &parse_key(key).unwrap(), Some("a/b")).unwrap();
        assert_eq!(launch_model(&c, "pi"), ("a/b".to_string(), "agent"));
        set(&mut c, key, &parse_key(key).unwrap(), Some("auto")).unwrap();
        assert_eq!(launch_model(&c, "pi").1, "default");
        assert!(c.agents.is_empty(), "an empty binding is pruned");
        set(&mut c, "launcher.agent", &Setting::DefaultAgent, Some("pi")).unwrap();
        set(&mut c, "launcher.agent", &Setting::DefaultAgent, None).unwrap();
        assert_eq!(get(&c, &Setting::DefaultAgent), None);
    }

    #[test]
    fn bad_values_are_usage_errors_and_change_nothing() {
        let mut c = cfg();
        for (k, v) in [
            ("launcher.agent", "vim"),
            ("launcher.mcp_auto", "maybe"),
            ("update.channel", "nightly"),
        ] {
            let err = set(&mut c, k, &parse_key(k).unwrap(), Some(v)).unwrap_err();
            assert!(
                err.starts_with(crate::cmd::dispatch::USAGE),
                "{k}={v}: {err}"
            );
        }
        assert_eq!(c, cfg());
    }
}
