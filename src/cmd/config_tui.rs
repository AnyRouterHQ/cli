use std::collections::BTreeMap;
use std::path::Path;

use crate::config::{valid_account_name, write_config, Profile};
use crate::http::{fetch_credits, fetch_me};
use crate::install::{
    agent_available, available_agents, ensure_tool_installed, Install, KNOWN_AGENTS,
};
use crate::key::{
    load_config_if_present, mask_api_key, no_key_error, resolve_api_key, resolve_base_url,
};
use crate::parse::{get_string_flag, FlagValue, ParsedArgs};
use crate::spawn::{catalog_model_id, display_model_id, resolve_tool, session_model_label};
use crate::term;

use crate::cmd::account::{run_account_use, run_logout};

#[cfg(not(feature = "native"))]
use crate::cmd::auth::run_auth_switch;
#[cfg(not(feature = "native"))]
use crate::cmd::dispatch::tui_menu_select;
use crate::cmd::dispatch::{
    config_path, hint, launcher_last_tool, persist_tool_command, tool_command_for, tui_wants_dump,
    CreditsCache,
};
#[cfg(feature = "native")]
use crate::cmd::dispatch::{tui_dump_settings, tui_settings_select};
use crate::cmd::keys::stored_api_key;
use crate::cmd::login::run_login;
use crate::cmd::models::{
    flag_agent, models_for_picker, pick_list, pick_model, run_models, save_model_slot,
    slot_current, slot_title, toggle_agent_routing_field, RoutingField,
};
#[cfg(not(feature = "native"))]
use crate::cmd::usage::run_usage;

pub(crate) fn print_config_status(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
) -> Result<(), String> {
    let cfg = load_config_if_present(path).unwrap_or_default();
    let profile = cfg.profiles.get(&cfg.active_profile);
    let key = resolve_api_key(&parsed.flags, env, profile);
    let base = resolve_base_url(&parsed.flags, profile);
    // Account and credits cost two network calls; a pipe or script gets the
    // local settings at once unless it asks with --verbose.
    let online = term::is_interactive() || parsed.flag_true("verbose");
    let key_online = key.as_deref().filter(|_| online);
    let me = key_online.and_then(|k| fetch_me(&base, k).ok());
    let credits_res = key_online.map(|k| fetch_credits(&base, k));
    let restricted = matches!(&credits_res, Some(Err(e)) if e.contains(crate::http::SCOPE_ERROR));
    let credits = credits_res.and_then(Result::ok);
    let account = me
        .as_ref()
        .map(|m| m.display_label())
        .unwrap_or_else(|| cfg.active_profile.clone());
    let extra_email = me
        .as_ref()
        .and_then(|m| m.email.as_deref())
        .filter(|e| !account.contains(e))
        .unwrap_or("");
    let account_value = if extra_email.is_empty() {
        account
    } else {
        format!("{account}  {extra_email}")
    };
    let model = session_model_label(
        profile
            .map(|p| p.default_model())
            .unwrap_or(crate::config::DEFAULT_MODEL),
    );
    let agent = cfg.preferred_agent(None).unwrap_or_else(|| "claude".into());
    let credits_value = credits
        .as_ref()
        .and_then(|c| c.get("balance").and_then(|v| v.as_f64()))
        .map(crate::http::format_usd)
        .unwrap_or_else(|| "-".into());
    let bin = crate::help::invoked_bin();
    fn row(label: &str, value: &str) {
        println!("  {}  {value}", term::dim(&format!("{label:<12}")));
    }
    println!();
    row("account", &account_value);
    row("key", &mask_api_key(key.as_deref()));
    println!(
        "  {}  {}",
        term::dim(&format!("{:<12}", "model")),
        term::model_id(&model)
    );
    row(
        "auto",
        profile
            .map(|p| p.pinned_preset())
            .unwrap_or(crate::config::DEFAULT_PRESET),
    );
    row("agent", &agent);
    if online {
        println!(
            "  {}  {}",
            term::dim(&format!("{:<12}", "credits")),
            if restricted {
                term::dim("- (restricted key: no credits scope)")
            } else if credits_value == "-" {
                term::dim("-")
            } else {
                term::ok(&credits_value)
            }
        );
    }
    row("channel", cfg.channel());
    println!(
        "  {}  {}",
        term::dim(&format!("{:<12}", "auto_update")),
        if cfg.auto_update() {
            term::ok("on")
        } else {
            "off".into()
        }
    );
    if let Some(on) = cfg.mcp_auto {
        row("mcp_auto", if on { "on" } else { "off" });
    }
    for (label, value) in crate::cmd::settings::launcher_rows(&cfg) {
        row(&label, &value);
    }
    println!();
    println!(
        "  {}  {}",
        term::dim(&format!("$ {bin} config set launcher.claude.model <id>")),
        term::dim("per-agent settings")
    );
    println!(
        "  {}    {}",
        term::dim(&format!("$ {bin} config get --json")),
        term::dim("for scripts")
    );
    println!(
        "  {}      {}",
        term::dim(&format!("$ {bin} config use work")),
        term::dim("switch account")
    );
    println!(
        "  {}        {}",
        term::dim(&format!("$ {bin} models --pick")),
        term::dim("interactive picker only when asked")
    );
    println!();
    Ok(())
}

#[cfg_attr(not(feature = "native"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingKind {
    Account,
    ApiKey,
    /// Model slot: "default", "haiku", "sonnet", "opus", "fable".
    Model(&'static str),
    Agent,
    AutoUpdate,
    Channel,
    Install(&'static str),
    ToolCommand(&'static str),
    GatewayDiscovery,
    /// Read-only mapping row.
    Mapping,
    AgentAccount(&'static str),
    AgentKey(&'static str),
    AgentModel(&'static str),
    AgentRouting(&'static str, RoutingField),
}

/// The CLI's own key cannot read another key's secret (reveal takes a browser
/// session), so a different key means a fresh login or a pasted key.
fn print_new_key_hint() {
    println!(
        "{}",
        term::dim(&crate::help::with_bin(
            "New key for this machine: {bin} login · paste one: {bin} login --key - · unused keys: {bin} keys audit"
        ))
    );
}

pub(crate) fn run_config_tui(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let path = config_path(parsed, env);
    if !tui_wants_dump(parsed, env) && stored_api_key(parsed, env, &path).is_none() {
        run_login(parsed, env)?;
        if stored_api_key(parsed, env, &path).is_none() {
            return Err(hint(
                "Not signed in. Run `{bin} auth login` or pass --key / ANYROUTER_API_KEY.",
            ));
        }
    }
    #[cfg(feature = "native")]
    {
        if tui_wants_dump(parsed, env) {
            let tab = settings_tab_index(env);
            let (state, _) =
                config_settings_frame(parsed, env, &path, false, &mut CreditsCache::fresh(), tab);
            print!("{}", tui_dump_settings(state, env));
            return Ok(0);
        }
        config_settings_loop(parsed, env, &path)
    }
    #[cfg(not(feature = "native"))]
    config_menu_loop_legacy(parsed, env, &path)
}

#[cfg(feature = "native")]
pub(crate) fn settings_tab_index(env: &BTreeMap<String, String>) -> usize {
    let Some(raw) = env
        .get("ANYR_TUI_TAB")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    else {
        return 0;
    };
    if let Ok(i) = raw.parse::<usize>() {
        return i;
    }
    settings_tab_names()
        .iter()
        .position(|t| t.eq_ignore_ascii_case(raw))
        .unwrap_or(0)
}

#[cfg(feature = "native")]
pub(crate) fn settings_tab_names() -> Vec<String> {
    let mut tabs = vec!["general".to_string()];
    tabs.extend(KNOWN_AGENTS.iter().map(|(id, _)| (*id).to_string()));
    tabs
}

#[cfg(feature = "native")]
pub(crate) fn config_settings_frame(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
    online: bool,
    cache: &mut CreditsCache,
    tab: usize,
) -> (crate::tui::SettingsState, Vec<Option<SettingKind>>) {
    use crate::tui::{SettingRow, SettingsState, Tone};

    let cfg = load_config_if_present(path).unwrap_or_default();
    let profile = cfg.profiles.get(&cfg.active_profile);
    let key = stored_api_key(parsed, env, path);
    let base = resolve_base_url(&parsed.flags, profile);
    let signed_in = key.is_some();

    let identity = if online {
        cache.identity(&base, key.as_deref())
    } else {
        None
    };
    let account_value = match (&identity, signed_in) {
        (Some(me), _) => me.display_label(),
        (None, true) => cfg.active_profile.clone(),
        (None, false) => "(not signed in)".into(),
    };
    let account_tone = if signed_in { Tone::Normal } else { Tone::Warn };
    let credits_value = if !online {
        "-".into()
    } else {
        match identity.as_ref().and_then(|me| me.balance) {
            Some(b) => crate::http::format_usd(b),
            None => cache.get(&base, key.as_deref()),
        }
    };

    let header = vec![
        format!("account  {account_value}"),
        format!("credits  {credits_value}"),
        format!("file     {}", path.display()),
    ];

    let mut rows: Vec<SettingRow> = Vec::new();
    let mut kinds: Vec<Option<SettingKind>> = Vec::new();

    let tabs = settings_tab_names();
    let tab = tab.min(tabs.len().saturating_sub(1));
    if tab == 0 {
        fill_general_settings(
            &mut rows,
            &mut kinds,
            path,
            parsed,
            env,
            profile,
            signed_in,
            key.as_deref(),
            account_value.clone(),
            account_tone,
            &cfg,
        );
    } else if let Some((id, label)) = KNOWN_AGENTS.get(tab - 1).copied() {
        fill_agent_settings(&mut rows, &mut kinds, path, env, profile, id, label);
    }

    (
        SettingsState::new("Config", header, rows).with_tabs(tabs, tab),
        kinds,
    )
}

#[cfg(feature = "native")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn fill_general_settings(
    rows: &mut Vec<crate::tui::SettingRow>,
    kinds: &mut Vec<Option<SettingKind>>,
    path: &Path,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    profile: Option<&Profile>,
    signed_in: bool,
    key: Option<&str>,
    account_value: String,
    account_tone: crate::tui::Tone,
    cfg: &crate::config::Config,
) {
    use crate::tui::{SettingRow, Tone};
    fn section(rows: &mut Vec<SettingRow>, kinds: &mut Vec<Option<SettingKind>>, name: &str) {
        if !rows.is_empty() {
            rows.push(SettingRow::Gap);
            kinds.push(None);
        }
        rows.push(SettingRow::Section(name.into()));
        kinds.push(None);
    }
    fn entry(
        rows: &mut Vec<SettingRow>,
        kinds: &mut Vec<Option<SettingKind>>,
        label: &str,
        value: String,
        tone: Tone,
        kind: SettingKind,
    ) {
        rows.push(SettingRow::Entry {
            label: label.into(),
            value,
            tone,
        });
        kinds.push(Some(kind));
    }

    section(rows, kinds, "Account");
    entry(
        rows,
        kinds,
        "account",
        account_value,
        account_tone,
        SettingKind::Account,
    );
    let key_value = if signed_in {
        mask_api_key(key)
    } else {
        "(not set)".into()
    };
    entry(
        rows,
        kinds,
        "api key",
        key_value,
        if signed_in { Tone::Normal } else { Tone::Muted },
        SettingKind::ApiKey,
    );

    section(rows, kinds, "Model");
    entry(
        rows,
        kinds,
        "default",
        session_model_label(
            profile
                .map(|p| p.default_model())
                .unwrap_or(crate::config::DEFAULT_MODEL),
        ),
        Tone::Model,
        SettingKind::Model("default"),
    );

    section(rows, kinds, "Agent");
    let agent = launcher_last_tool(path, parsed, env);
    let agent_pinned = profile.and_then(|p| p.default_tool.clone()).is_some();
    entry(
        rows,
        kinds,
        "coding agent",
        agent,
        if agent_pinned {
            Tone::Normal
        } else {
            Tone::Muted
        },
        SettingKind::Agent,
    );
    let present = available_agents(env, |id| tool_command_for(path, id));
    entry(
        rows,
        kinds,
        "on PATH",
        if present.is_empty() {
            "none detected".into()
        } else {
            present
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>()
                .join(", ")
        },
        if present.is_empty() {
            Tone::Warn
        } else {
            Tone::Good
        },
        SettingKind::Mapping,
    );

    section(rows, kinds, "General");
    entry(
        rows,
        kinds,
        "auto-update",
        if cfg.auto_update() {
            "enabled".into()
        } else {
            "disabled".into()
        },
        if cfg.auto_update() {
            Tone::Good
        } else {
            Tone::Muted
        },
        SettingKind::AutoUpdate,
    );
    entry(
        rows,
        kinds,
        "update channel",
        cfg.channel().into(),
        Tone::Normal,
        SettingKind::Channel,
    );
}

#[cfg(feature = "native")]
pub(crate) fn fill_agent_settings(
    rows: &mut Vec<crate::tui::SettingRow>,
    kinds: &mut Vec<Option<SettingKind>>,
    path: &Path,
    env: &BTreeMap<String, String>,
    profile: Option<&Profile>,
    id: &'static str,
    label: &str,
) {
    use crate::tui::{SettingRow, Tone};
    fn section(rows: &mut Vec<SettingRow>, kinds: &mut Vec<Option<SettingKind>>, name: &str) {
        if !rows.is_empty() {
            rows.push(SettingRow::Gap);
            kinds.push(None);
        }
        rows.push(SettingRow::Section(name.into()));
        kinds.push(None);
    }
    fn entry(
        rows: &mut Vec<SettingRow>,
        kinds: &mut Vec<Option<SettingKind>>,
        label: &str,
        value: String,
        tone: Tone,
        kind: SettingKind,
    ) {
        rows.push(SettingRow::Entry {
            label: label.into(),
            value,
            tone,
        });
        kinds.push(Some(kind));
    }

    let cfg = load_config_if_present(path);
    let tool = resolve_tool(cfg.as_ref(), id)
        .unwrap_or_else(|_| crate::spawn::resolve_tool(None, id).expect("known agent"));
    let present = agent_available(id, &tool.command, env);

    section(rows, kinds, label);
    entry(
        rows,
        kinds,
        "status",
        if present {
            "on PATH".into()
        } else {
            "not installed".into()
        },
        if present { Tone::Good } else { Tone::Warn },
        SettingKind::Mapping,
    );
    entry(
        rows,
        kinds,
        "command",
        tool.command.clone(),
        Tone::Normal,
        SettingKind::ToolCommand(id),
    );
    let hint = crate::install::tool_hint(id);
    entry(
        rows,
        kinds,
        "install",
        if present {
            "reinstall / update".into()
        } else {
            hint.map(|h| h.install.to_string())
                .unwrap_or_else(|| "install".into())
        },
        if present { Tone::Muted } else { Tone::Warn },
        SettingKind::Install(id),
    );

    let binding = cfg.as_ref().and_then(|c| c.agent_binding(id));
    section(rows, kinds, "Bindings");
    let account = binding
        .and_then(|b| b.profile.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "session default".into());
    entry(
        rows,
        kinds,
        "account",
        account,
        if binding.and_then(|b| b.profile.as_deref()).is_some() {
            Tone::Normal
        } else {
            Tone::Muted
        },
        SettingKind::AgentAccount(id),
    );
    let key_value = binding
        .and_then(|b| b.api_key.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|k| mask_api_key(Some(k)))
        .unwrap_or_else(|| "profile default".into());
    entry(
        rows,
        kinds,
        "api key",
        key_value,
        if binding.and_then(|b| b.api_key.as_deref()).is_some() {
            Tone::Normal
        } else {
            Tone::Muted
        },
        SettingKind::AgentKey(id),
    );
    let model_value = binding
        .and_then(|b| b.default_model.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(session_model_label)
        .unwrap_or_else(|| "session default".into());
    entry(
        rows,
        kinds,
        "model",
        model_value,
        if binding.and_then(|b| b.default_model.as_deref()).is_some() {
            Tone::Model
        } else {
            Tone::Muted
        },
        SettingKind::AgentModel(id),
    );

    section(rows, kinds, "Routing");
    let routing = binding.map(|b| b.routing.clone()).unwrap_or_default();
    entry(
        rows,
        kinds,
        "exacto",
        if routing.wants_exacto() {
            "on".into()
        } else {
            "off".into()
        },
        if routing.wants_exacto() {
            Tone::Good
        } else {
            Tone::Muted
        },
        SettingKind::AgentRouting(id, RoutingField::Exacto),
    );
    entry(
        rows,
        kinds,
        "tools",
        if routing.requires_tools() {
            "on".into()
        } else {
            "off".into()
        },
        if routing.requires_tools() {
            Tone::Good
        } else {
            Tone::Muted
        },
        SettingKind::AgentRouting(id, RoutingField::Tools),
    );
    entry(
        rows,
        kinds,
        "1M ctx",
        if routing.requires_1m_context() {
            "on".into()
        } else {
            "off".into()
        },
        if routing.requires_1m_context() {
            Tone::Good
        } else {
            Tone::Muted
        },
        SettingKind::AgentRouting(id, RoutingField::MinContext),
    );

    section(rows, kinds, "Mapping");
    entry(
        rows,
        kinds,
        "base URL env",
        tool.base_url_env.clone(),
        Tone::Muted,
        SettingKind::Mapping,
    );
    entry(
        rows,
        kinds,
        "auth env",
        tool.auth_env.clone(),
        Tone::Muted,
        SettingKind::Mapping,
    );
    entry(
        rows,
        kinds,
        "model env",
        tool.model_env.clone().unwrap_or_else(|| "(none)".into()),
        Tone::Muted,
        SettingKind::Mapping,
    );
    entry(
        rows,
        kinds,
        "URL suffix",
        if tool.base_suffix.is_empty() {
            "(none)".into()
        } else {
            tool.base_suffix.clone()
        },
        Tone::Muted,
        SettingKind::Mapping,
    );

    if id == "claude" {
        section(rows, kinds, "Claude aliases");
        for (slot_label, slot) in [
            ("haiku", "haiku"),
            ("sonnet", "sonnet"),
            ("opus", "opus"),
            ("fable", "fable"),
        ] {
            let pinned = match slot {
                "haiku" => nonempty_slot(&profile.and_then(|p| p.claude_haiku.clone())),
                "sonnet" => nonempty_slot(&profile.and_then(|p| p.claude_sonnet.clone())),
                "opus" => nonempty_slot(&profile.and_then(|p| p.claude_opus.clone())),
                _ => nonempty_slot(&profile.and_then(|p| p.claude_fable.clone())),
            };
            let value = match &pinned {
                Some(mid) => catalog_model_id(mid),
                None => format!("{} · default", slot_current_opt(profile, slot)),
            };
            let tone = if pinned.is_some() {
                Tone::Model
            } else {
                Tone::Muted
            };
            let slot_static: &'static str = match slot {
                "haiku" => "haiku",
                "sonnet" => "sonnet",
                "opus" => "opus",
                _ => "fable",
            };
            entry(
                rows,
                kinds,
                slot_label,
                value,
                tone,
                SettingKind::Model(slot_static),
            );
        }
        entry(
            rows,
            kinds,
            "gateway discovery",
            if tool.enable_gateway_model_discovery {
                "on".into()
            } else {
                "off".into()
            },
            if tool.enable_gateway_model_discovery {
                Tone::Good
            } else {
                Tone::Muted
            },
            SettingKind::GatewayDiscovery,
        );
    }
}

#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub(crate) fn nonempty_slot(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub(crate) fn slot_current_opt(profile: Option<&Profile>, slot: &str) -> String {
    slot_current(profile.unwrap_or(&Profile::default()), slot).to_string()
}

#[cfg(feature = "native")]
pub(crate) fn config_settings_loop(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
) -> Result<i32, String> {
    let mut cache = CreditsCache::fresh();
    let mut tab = 0usize;
    let n_tabs = settings_tab_names().len().max(1);
    loop {
        let (state, kinds) = config_settings_frame(parsed, env, path, true, &mut cache, tab);
        let Some(outcome) = tui_settings_select(state)? else {
            return Ok(0);
        };
        let result = match outcome {
            crate::tui::SettingsOutcome::Edit(idx) => kinds
                .get(idx)
                .copied()
                .flatten()
                .map(|kind| config_edit_row(parsed, env, path, kind)),
            crate::tui::SettingsOutcome::Reset(idx) => kinds
                .get(idx)
                .copied()
                .flatten()
                .map(|kind| config_reset_row(path, kind)),
            crate::tui::SettingsOutcome::NextTab => {
                tab = (tab + 1) % n_tabs;
                None
            }
            crate::tui::SettingsOutcome::PrevTab => {
                tab = (tab + n_tabs - 1) % n_tabs;
                None
            }
            crate::tui::SettingsOutcome::GotoTab(i) => {
                tab = i.min(n_tabs.saturating_sub(1));
                None
            }
            crate::tui::SettingsOutcome::Close | crate::tui::SettingsOutcome::Stay => None,
        };
        if let Some(Err(err)) = result {
            if err != "Cancelled." {
                eprintln!("{}", term::err(&err));
            }
        }
    }
}

#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub(crate) fn config_edit_row(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
    kind: SettingKind,
) -> Result<i32, String> {
    match kind {
        SettingKind::Account => config_account_actions(parsed, env),
        SettingKind::ApiKey => {
            print_new_key_hint();
            Ok(0)
        }
        SettingKind::Model(slot) => {
            let existing = load_config_if_present(path);
            let profile = existing
                .as_ref()
                .and_then(|c| c.profiles.get(&c.active_profile));
            let key = resolve_api_key(&parsed.flags, env, profile);
            let base = resolve_base_url(&parsed.flags, profile);
            let models = models_for_picker(path.parent(), &base, key.as_deref(), env);
            let current = profile.map(|p| slot_current(p, slot).to_string());
            let id = pick_model(&models, current.as_deref(), slot_title(slot))?;
            save_model_slot(existing, path, slot, &id)
        }
        SettingKind::Agent => {
            let last = launcher_last_tool(path, parsed, env);
            let labels: Vec<String> = KNOWN_AGENTS
                .iter()
                .map(|(id, label)| format!("{id}  —  {label}"))
                .collect();
            let current = KNOWN_AGENTS.iter().position(|(id, _)| *id == last.as_str());
            let idx = term::pick("Coding agent", &labels, current)?;
            let (tool, label) = KNOWN_AGENTS[idx];
            let mut cfg = load_config_if_present(path).unwrap_or_default();
            if let Some(p) = cfg.profiles.get_mut(&cfg.active_profile) {
                p.default_tool = Some(tool.into());
            }
            write_config(&cfg, path)?;
            println!(
                "{}  coding agent  {}",
                term::ok("Saved"),
                term::paint(term::tool_color(tool), label)
            );
            Ok(0)
        }
        SettingKind::AutoUpdate => {
            let mut cfg = load_config_if_present(path).unwrap_or_default();
            cfg.auto_update = Some(!cfg.auto_update());
            write_config(&cfg, path)?;
            println!(
                "{}  auto-update {}",
                term::ok("Saved"),
                if cfg.auto_update() {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            Ok(0)
        }
        SettingKind::Install(id) => {
            let command = tool_command_for(path, id);
            match ensure_tool_installed(id, &command, Install::Yes) {
                Ok(resolved) => {
                    persist_tool_command(path, id, &resolved)?;
                    println!(
                        "{}  {}  {}",
                        term::ok("Installed"),
                        id,
                        term::dim(&resolved)
                    );
                    Ok(0)
                }
                Err(err) => Err(err),
            }
        }
        SettingKind::ToolCommand(id) => {
            let current = tool_command_for(path, id);
            let next = term::prompt(&format!("Command path for {id} (Enter keeps {current}): "))?;
            let next = next.trim();
            if next.is_empty() {
                return Ok(0);
            }
            let mut cfg = load_config_if_present(path).unwrap_or_default();
            let mut tool = resolve_tool(Some(&cfg), id)?;
            tool.command = next.to_string();
            cfg.tools.insert(id.to_string(), tool);
            write_config(&cfg, path)?;
            println!("{}  {id} command  {next}", term::ok("Saved"));
            Ok(0)
        }
        SettingKind::GatewayDiscovery => {
            let mut cfg = load_config_if_present(path).unwrap_or_default();
            let mut tool = resolve_tool(Some(&cfg), "claude")?;
            tool.enable_gateway_model_discovery = !tool.enable_gateway_model_discovery;
            let on = tool.enable_gateway_model_discovery;
            cfg.tools.insert("claude".into(), tool);
            write_config(&cfg, path)?;
            println!(
                "{}  gateway discovery  {}",
                term::ok("Saved"),
                if on { "on" } else { "off" }
            );
            Ok(0)
        }
        SettingKind::Mapping => Ok(0),
        SettingKind::AgentAccount(id) => {
            let mut next = parsed.clone();
            next.flags
                .insert("agent".into(), FlagValue::Value(id.to_string()));
            config_account_actions(&next, env)
        }
        SettingKind::AgentKey(_) => {
            print_new_key_hint();
            Ok(0)
        }
        SettingKind::AgentModel(id) => {
            let mut next = parsed.clone();
            next.command = "models".into();
            next.flags.insert("pick".into(), FlagValue::Bool(true));
            next.flags
                .insert("agent".into(), FlagValue::Value(id.to_string()));
            match run_models(&next, env) {
                Err(err) if err == "Cancelled." => Ok(0),
                other => other,
            }
        }
        SettingKind::AgentRouting(id, field) => toggle_agent_routing_field(path, id, field),
        SettingKind::Channel => {
            let choices = ["stable", "beta"];
            let current = choices.iter().position(|c| *c == cfg_channel(path));
            let idx = term::pick(
                "Update channel",
                &choices.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
                current,
            )?;
            let mut cfg = load_config_if_present(path).unwrap_or_default();
            cfg.channel = Some(choices[idx].into());
            write_config(&cfg, path)?;
            println!("{}  channel  {}", term::ok("Saved"), choices[idx]);
            Ok(0)
        }
    }
}

#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub(crate) fn cfg_channel(path: &std::path::Path) -> String {
    load_config_if_present(path)
        .map(|c| c.channel().to_string())
        .unwrap_or_else(|| "stable".into())
}

#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub(crate) fn config_account_actions(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let path = config_path(parsed, env);
    let cfg = load_config_if_present(&path).unwrap_or_default();
    let mut names: Vec<String> = cfg.profiles.keys().cloned().collect();
    names.sort();
    let mut labels: Vec<String> = names
        .iter()
        .map(|n| {
            let p = cfg.profiles.get(n);
            let key = mask_api_key(p.and_then(|p| p.api_key.as_deref()));
            let mark = if n == &cfg.active_profile {
                "  ●"
            } else {
                ""
            };
            format!("{n}  ·  {key}{mark}")
        })
        .collect();
    let add_idx = labels.len();
    labels.push("＋  Add account".into());
    labels.push("Re-authenticate (login)".into());
    labels.push("Log out".into());
    let current = if let Some(agent) = flag_agent(parsed) {
        cfg.agent_binding(&agent)
            .and_then(|b| b.profile.as_ref())
            .and_then(|n| names.iter().position(|x| x == n))
            .or_else(|| names.iter().position(|n| n == &cfg.active_profile))
    } else {
        names.iter().position(|n| n == &cfg.active_profile)
    };
    let title = if let Some(agent) = flag_agent(parsed) {
        format!("{agent} account")
    } else {
        "Account".into()
    };
    let idx = pick_list(
        &title,
        &["enter to switch    newest profiles are listed too".into()],
        &labels,
        current,
    )?;
    if idx < names.len() {
        return run_account_use(parsed, env, &names[idx]);
    }
    if idx == add_idx {
        let name = term::prompt("New account name (Enter for \"default\"): ")?;
        let name = name.trim();
        let name = if name.is_empty() { "default" } else { name };
        if !valid_account_name(name) {
            return Err(format!(
                "Invalid account name \"{name}\". Use letters, digits, \".\", \"_\", \"-\"."
            ));
        }
        let mut flags = parsed.flags.clone();
        flags.insert("profile".into(), FlagValue::Value(name.into()));
        let next = ParsedArgs {
            command: "login".into(),
            flags,
            passthrough: Vec::new(),
            raw_from: None,
        };
        return run_login(&next, env);
    }
    if idx == add_idx + 1 {
        return run_login(parsed, env);
    }
    run_logout(parsed, env)
}

#[cfg_attr(not(feature = "native"), allow(dead_code))]
pub(crate) fn config_reset_row(path: &std::path::Path, kind: SettingKind) -> Result<i32, String> {
    let mut cfg = load_config_if_present(path).unwrap_or_default();
    match kind {
        SettingKind::AutoUpdate => {
            if cfg.auto_update.is_none() {
                println!("{}", term::dim("auto-update already at default (enabled)"));
                return Ok(0);
            }
            cfg.auto_update = None;
            write_config(&cfg, path)?;
            println!(
                "{}  auto-update reset to default (enabled)",
                term::ok("Saved")
            );
            Ok(0)
        }
        SettingKind::Channel => {
            if cfg.channel.is_none() {
                println!("{}", term::dim("channel already at default (stable)"));
                return Ok(0);
            }
            cfg.channel = None;
            write_config(&cfg, path)?;
            println!("{}  channel reset to default (stable)", term::ok("Saved"));
            Ok(0)
        }
        SettingKind::Account
        | SettingKind::ApiKey
        | SettingKind::Install(_)
        | SettingKind::ToolCommand(_)
        | SettingKind::Mapping => Ok(0),
        SettingKind::AgentAccount(id) | SettingKind::AgentKey(id) | SettingKind::AgentModel(id) => {
            let (label, was_set) = {
                let Some(b) = cfg.agents.get_mut(id) else {
                    println!("{}", term::dim(&format!("{id} already at session default")));
                    return Ok(0);
                };
                match kind {
                    SettingKind::AgentAccount(_) => ("account", b.profile.take().is_some()),
                    SettingKind::AgentKey(_) => ("key", b.api_key.take().is_some()),
                    _ => ("model", b.default_model.take().is_some()),
                }
            };
            cfg.prune_empty_agents();
            if !was_set {
                println!(
                    "{}",
                    term::dim(&format!("{id} {label} already at session default"))
                );
                return Ok(0);
            }
            write_config(&cfg, path)?;
            println!(
                "{}  {id} {label} reset to session default",
                term::ok("Saved")
            );
            Ok(0)
        }
        SettingKind::AgentRouting(id, field) => {
            let was_set = {
                let Some(b) = cfg.agents.get_mut(id) else {
                    println!(
                        "{}",
                        term::dim(&format!("{id} {} already off", field.label()))
                    );
                    return Ok(0);
                };
                match field {
                    RoutingField::Exacto => {
                        let on = b.routing.wants_exacto();
                        b.routing.set_exacto(false);
                        on
                    }
                    RoutingField::Tools => {
                        let on = b.routing.requires_tools();
                        b.routing.set_require_tools(false);
                        on
                    }
                    RoutingField::MinContext => {
                        let on = b.routing.requires_1m_context();
                        b.routing.set_require_1m(false);
                        on
                    }
                }
            };
            cfg.prune_empty_agents();
            if !was_set {
                println!(
                    "{}",
                    term::dim(&format!("{id} {} already off", field.label()))
                );
                return Ok(0);
            }
            write_config(&cfg, path)?;
            println!("{}  {id} {} off", term::ok("Saved"), field.label());
            Ok(0)
        }
        SettingKind::GatewayDiscovery => {
            let mut cfg = load_config_if_present(path).unwrap_or_default();
            if let Some(t) = cfg.tools.get_mut("claude") {
                t.enable_gateway_model_discovery = true;
            }
            write_config(&cfg, path)?;
            println!("{}  gateway discovery reset to on", term::ok("Saved"));
            Ok(0)
        }
        SettingKind::Model(_) | SettingKind::Agent => {
            let name = cfg.active_profile.clone();
            let Some(p) = cfg.profiles.get_mut(&name) else {
                return Err(no_key_error());
            };
            let (label, was_set) = match kind {
                SettingKind::Model("haiku") => ("haiku", p.claude_haiku.take().is_some()),
                SettingKind::Model("sonnet") => ("sonnet", p.claude_sonnet.take().is_some()),
                SettingKind::Model("opus") => ("opus", p.claude_opus.take().is_some()),
                SettingKind::Model("fable") => ("fable", p.claude_fable.take().is_some()),
                SettingKind::Model(_) => ("default model", p.default_model.take().is_some()),
                _ => ("coding agent", p.default_tool.take().is_some()),
            };
            if !was_set {
                println!("{}", term::dim(&format!("{label} already at default")));
                return Ok(0);
            }
            write_config(&cfg, path)?;
            if label == "default model" {
                println!(
                    "{}  default model reset to anyrouter/auto",
                    term::ok("Saved")
                );
            } else {
                println!("{}  {} reset to default", term::ok("Saved"), label);
            }
            Ok(0)
        }
    }
}

#[cfg(not(feature = "native"))]
pub(crate) fn config_menu_loop_legacy(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
) -> Result<i32, String> {
    let items = vec![
        "Switch account".into(),
        "Switch model".into(),
        "Credits".into(),
        "Sign in".into(),
        "Log out".into(),
        "Done".into(),
    ];
    loop {
        let header = config_tui_header(path);
        let Some(idx) = tui_menu_select("Config", header, items.clone())? else {
            return Ok(0);
        };
        let result = match idx {
            0 => {
                let mut next = parsed.clone();
                next.passthrough = Vec::new();
                run_auth_switch(&next, env)
            }
            1 => {
                let mut next = parsed.clone();
                next.command = "models".into();
                next.flags.insert("pick".into(), FlagValue::Bool(true));
                next.passthrough = Vec::new();
                run_models(&next, env)
            }
            2 => run_usage(parsed, env),
            3 => run_login(parsed, env),
            4 => run_logout(parsed, env),
            _ => return Ok(0),
        };
        if let Err(err) = result {
            eprintln!("{}", term::err(&err));
        }
    }
}

#[cfg(not(feature = "native"))]
pub(crate) fn config_tui_header(path: &std::path::Path) -> Vec<String> {
    let cfg = load_config_if_present(path).unwrap_or_default();
    let profile = cfg.profiles.get(&cfg.active_profile);
    vec![
        format!(
            "account  {}  {}",
            cfg.active_profile,
            mask_api_key(profile.and_then(|p| p.api_key.as_deref()))
        ),
        format!(
            "model    {}",
            session_model_label(
                profile
                    .map(|p| p.default_model())
                    .unwrap_or(crate::config::DEFAULT_MODEL),
            )
        ),
        format!("file     {}", path.display()),
    ]
}

pub(crate) fn run_config(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let path = config_path(parsed, env);
    let sub = parsed.passthrough.first().map(String::as_str);
    match sub {
        Some("path") => {
            if parsed.flag_true("json") {
                println!("{}", serde_json::json!({ "path": path.display().to_string() }));
            } else {
                println!("{}", path.display());
            }
            Ok(0)
        }
        Some("use") => {
            let name = parsed
                .passthrough
                .get(1)
                .cloned()
                .ok_or_else(|| hint("Usage: {bin} config use <account>"))?;
            run_account_use(parsed, env, &name)
        }
        Some(verb @ ("set" | "unset")) => crate::cmd::settings::run(verb, parsed, env),
        Some("get") if parsed.passthrough.len() > 1 => {
            crate::cmd::settings::run("get", parsed, env)
        }
        Some("get") => {
            if parsed.flag_true("json") {
                let cfg = load_config_if_present(&path).unwrap_or_default();
                let profile = cfg.profiles.get(&cfg.active_profile);
                let key = resolve_api_key(&parsed.flags, env, profile);
                let (api_key, api_key_source) = match key.as_deref() {
                    None => (serde_json::Value::Null, "none"),
                    Some(k) => {
                        let source = if get_string_flag(&parsed.flags, "key").is_some_and(|f| f.trim() == k) {
                            "flag"
                        } else if profile.and_then(|p| p.api_key.as_deref()).map(str::trim) == Some(k)
                            && env.get("ANYROUTER_API_KEY").map(|e| e.trim()) != Some(k)
                        {
                            "config"
                        } else {
                            "env"
                        };
                        (serde_json::json!(mask_api_key(Some(k))), source)
                    }
                };
                let payload = serde_json::json!({
                    "path": path.display().to_string(),
                    "active_profile": cfg.active_profile,
                    "api_key": api_key,
                    "api_key_source": api_key_source,
                    "default_model": profile.map(|p| display_model_id(p.default_model())),
                    "launcher": crate::cmd::settings::launcher_json(&cfg),
                    "accounts": cfg.profiles.keys().cloned().collect::<Vec<_>>(),
                    "auto_update": cfg.auto_update(),
                    "channel": cfg.channel(),
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".into())
                );
                Ok(0)
            } else {
                print_config_status(parsed, env, &path)?;
                Ok(0)
            }
        }
        None if parsed.flag_true("json") => {
            let mut next = parsed.clone();
            next.passthrough = vec!["get".into()];
            run_config(&next, env)
        }
        None if tui_wants_dump(parsed, env) || parsed.flag_true("pick") => {
            run_config_tui(parsed, env)
        }
        None => {
            print_config_status(parsed, env, &path)?;
            Ok(0)
        }
        Some(other) => Err(hint(&format!(
            "Unknown config command \"{other}\". Try: {{bin}} config · {{bin}} config get|set|unset <key> · {{bin}} config path · {{bin}} config use <account>"
        ))),
    }
}

#[cfg(test)]
mod settings_tab_tests {
    use super::settings_tab_index;
    use std::collections::BTreeMap;

    #[test]
    fn settings_tab_index_reads_name_and_number() {
        let mut env = BTreeMap::new();
        assert_eq!(settings_tab_index(&env), 0);
        env.insert("ANYR_TUI_TAB".into(), "claude".into());
        assert_eq!(settings_tab_index(&env), 1);
        env.insert("ANYR_TUI_TAB".into(), "2".into());
        assert_eq!(settings_tab_index(&env), 2);
    }
}
