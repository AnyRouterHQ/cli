use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::install::{available_agents, ensure_tool_installed, missing_agents, Install};
use crate::key::{
    load_config_if_present, mask_api_key, no_key_error, resolve_api_key, resolve_base_url,
};
use crate::parse::{get_string_flag, FlagValue, ParsedArgs};
use crate::spawn::session_model_label;
use crate::term;

use crate::cmd::config_tui::{
    config_account_actions, config_edit_row, print_config_status, SettingKind,
};
#[cfg(feature = "native")]
use crate::cmd::dispatch::kick_credits_refresh;
use crate::cmd::dispatch::{
    catalog_lookup_enabled, config_path, launcher_last_tool, launcher_uses_palette,
    persist_tool_command, pick_palette_action, tool_command_for, tui_dump_palette, tui_wants_dump,
    CreditsCache,
};

#[cfg(not(feature = "native"))]
use crate::cmd::dispatch::InlineEntry;
use crate::cmd::keys::stored_api_key;
use crate::cmd::launch::run_launch;
use crate::cmd::login::run_login;
use crate::cmd::models::{
    models_for_picker, palette_bind_detail, pick_list, pick_model, routing_toggle_detail,
    run_models, save_agent_account, save_agent_model, toggle_agent_routing_field, RoutingField,
};
use crate::cmd::usage::run_usage;

/// `…6789` from a masked key: enough to tell keys apart in a one-line row.
fn key_last4(masked: &str) -> String {
    match masked.rsplit_once('…') {
        Some((_, tail)) if !tail.is_empty() => format!("…{tail}"),
        _ => masked.to_string(),
    }
}

pub(crate) fn agent_binding_detail(
    cfg: &crate::config::Config,
    id: &str,
    signed_in: bool,
) -> String {
    let profile = cfg.profiles.get(&cfg.active_profile);
    let binding = cfg.agent_binding(id);
    let model = binding
        .and_then(|b| b.default_model.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(session_model_label)
        .unwrap_or_else(|| {
            session_model_label(
                profile
                    .map(|p| p.default_model())
                    .unwrap_or(crate::config::DEFAULT_MODEL),
            )
        });
    let account = binding
        .and_then(|b| b.profile.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(cfg.active_profile.as_str());
    let key = if signed_in {
        let k = binding
            .and_then(|b| b.api_key.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| profile.and_then(|p| p.api_key.as_deref()));
        key_last4(&mask_api_key(k))
    } else {
        "(not signed in)".into()
    };
    // The account shows only when this agent uses another one than the
    // active profile; the account row above already names that one.
    let mut parts = vec![model];
    if account != cfg.active_profile {
        parts.push(account.to_string());
    }
    parts.push(key);
    if let Some(r) = binding.map(|b| &b.routing) {
        if r.wants_exacto() {
            parts.push("exacto".into());
        }
        if r.requires_tools() {
            parts.push("tools".into());
        }
        if r.requires_1m_context() {
            parts.push("1M".into());
        }
    }
    parts.join("  ·  ")
}

#[cfg(feature = "native")]
pub(crate) fn launcher_palette(
    path: &Path,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    credits: &mut CreditsCache,
) -> (Vec<String>, Vec<crate::tui::PaletteEntry>) {
    let cfg = load_config_if_present(path).unwrap_or_default();
    let profile = cfg.profiles.get(&cfg.active_profile);
    let signed_in = resolve_api_key(&parsed.flags, env, profile).is_some();
    let command_for = |id: &str| {
        crate::spawn::resolve_tool(Some(&cfg), id)
            .map(|t| t.command)
            .unwrap_or_else(|_| id.to_string())
    };
    let present = available_agents(env, command_for);
    let last = cfg
        .preferred_agent(get_string_flag(&parsed.flags, "tool").as_deref())
        .or_else(|| present.first().map(|(id, _)| (*id).to_string()))
        .unwrap_or_else(|| "claude".into());

    // Compact status: bindings live on each agent row, not a 5-line header.
    let dump_or_pipe = tui_wants_dump(parsed, env) || !term::is_interactive();
    let credits_line = format!("credits  {}", credits.peek_credits());
    let account_line = if dump_or_pipe {
        format!("account  {}", cfg.active_profile)
    } else if let Some(label) = credits.peek_identity().map(|me| me.display_label()) {
        format!("account  {label}")
    } else {
        format!("account  {}", cfg.active_profile)
    };
    let header = vec![account_line, credits_line];

    #[cfg(feature = "native")]
    use crate::tui::PaletteEntry;
    let mut entries = Vec::new();
    if signed_in {
        push_launch_entries(&mut entries, &present, &last, |id| {
            agent_binding_detail(&cfg, id, signed_in)
        });
    } else {
        entries.push(PaletteEntry::new(
            "login",
            "sign in / add key",
            "account",
            "Login / sign in",
        ));
    }
    let hub = if present.iter().any(|(id, _)| *id == last.as_str()) {
        last.clone()
    } else {
        present
            .first()
            .map(|(id, _)| (*id).to_string())
            .unwrap_or(last.clone())
    };
    push_agent_configure_entries(&mut entries, &hub, &cfg);
    entries.push(PaletteEntry::new(
        "install…",
        "install a coding agent",
        "more",
        "Install agent",
    ));
    entries.push(PaletteEntry::new(
        "config…",
        "accounts · keys · agent",
        "more",
        "Config",
    ));
    entries.push(PaletteEntry::new("quit", "esc works too", "more", "Quit"));
    (header, entries)
}

#[cfg(not(feature = "native"))]
pub(crate) fn launcher_palette(
    path: &Path,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    _credits: &mut CreditsCache,
) -> (Vec<String>, Vec<InlineEntry>) {
    let cfg = load_config_if_present(path).unwrap_or_default();
    let profile = cfg.profiles.get(&cfg.active_profile);
    let signed_in = launcher_signed_in(path, parsed, env);
    let last = launcher_last_tool(path, parsed, env);
    let header = vec![
        format!("account  {}", cfg.active_profile),
        "credits  -".to_string(),
    ];
    let mut entries = Vec::new();
    let present = available_agents(env, |id| tool_command_for(path, id));
    if signed_in {
        if present.is_empty() {
            entries.push(InlineEntry::new(
                "install an agent…",
                "none detected on PATH",
                "launch",
                "Install agent",
            ));
        } else {
            let last = if present.iter().any(|(id, _)| *id == last) {
                last.clone()
            } else {
                present[0].0.to_string()
            };
            let detail = agent_binding_detail(&cfg, &last, signed_in);
            entries.push(InlineEntry::new(
                last.clone(),
                detail,
                "launch",
                format!("Launch {last}"),
            ));
            for (id, _) in present
                .iter()
                .copied()
                .filter(|(id, _)| *id != last.as_str())
            {
                entries.push(InlineEntry::new(
                    id,
                    agent_binding_detail(&cfg, id, signed_in),
                    "launch",
                    format!("Launch {id}"),
                ));
            }
            push_inline_configure(&mut entries, &last, &cfg);
        }
    } else {
        entries.push(InlineEntry::new(
            "login",
            "sign in / add key",
            "account",
            "Login / sign in",
        ));
        push_inline_configure(&mut entries, &last, &cfg);
    }
    entries.push(InlineEntry::new(
        "install…",
        "install a coding agent",
        "more",
        "Install agent",
    ));
    entries.push(InlineEntry::new(
        "config…",
        "accounts · keys · agent",
        "more",
        "Config",
    ));
    entries.push(InlineEntry::new("quit", "esc works too", "more", "Quit"));
    (header, entries)
}

#[cfg(not(feature = "native"))]
pub(crate) fn push_inline_configure(
    entries: &mut Vec<InlineEntry>,
    agent: &str,
    cfg: &crate::config::Config,
) {
    let routing = cfg
        .agent_binding(agent)
        .map(|b| b.routing.clone())
        .unwrap_or_default();
    entries.push(InlineEntry::new(
        "model…",
        palette_bind_detail(agent),
        format!("configure · {agent}"),
        format!("Switch model {agent}"),
    ));
    entries.push(InlineEntry::new(
        "account…",
        palette_bind_detail(agent),
        format!("configure · {agent}"),
        format!("Switch account {agent}"),
    ));
    entries.push(InlineEntry::new(
        "exacto",
        routing_toggle_detail(routing.wants_exacto(), agent),
        format!("configure · {agent}"),
        format!("Toggle exacto {agent}"),
    ));
    entries.push(InlineEntry::new(
        "tools",
        routing_toggle_detail(routing.requires_tools(), agent),
        format!("configure · {agent}"),
        format!("Toggle tools {agent}"),
    ));
    entries.push(InlineEntry::new(
        "1M ctx",
        routing_toggle_detail(routing.requires_1m_context(), agent),
        format!("configure · {agent}"),
        format!("Toggle 1m {agent}"),
    ));
}

pub(crate) const HUD_FOOTER: &str = "# no fullscreen. works over SSH, scriptable with --ok";

pub(crate) fn run_menu(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let path = config_path(parsed, env);
    let dumping = tui_wants_dump(parsed, env);

    if dumping {
        if launcher_uses_palette(env) {
            let (header, entries) =
                launcher_palette(&path, parsed, env, &mut CreditsCache::fresh());
            print!("{}", tui_dump_palette(entries, header, env));
            return Ok(0);
        }
        let (status, actions) = launcher_hud(&path, parsed, env, &mut CreditsCache::fresh());
        let labels: Vec<String> = actions.iter().map(|(l, _)| l.clone()).collect();
        println!(
            "{}",
            term::render_inline_menu(&[status], "What do you want to do?", &labels, 0, HUD_FOOTER,)
        );
        return Ok(0);
    }

    if !term::is_interactive() {
        let (status, actions) = launcher_hud(&path, parsed, env, &mut CreditsCache::fresh());
        // Without a TTY the menu can't be driven, but the status line is the
        // whole point of the "preflight" launcher: account · model · agent ·
        // credits. Print it first so a piped `anyr menu` still reports state,
        // then list the actions it would offer.
        println!("{status}");
        println!(
            "{}",
            actions
                .iter()
                .map(|(l, _)| l.clone())
                .collect::<Vec<_>>()
                .join("\n")
        );
        return Ok(0);
    }

    // A long-lived menu keeps the background check fresh for its whole
    // session; staging never swaps, so the running agent is undisturbed.
    #[cfg(feature = "native")]
    let _check_ticker = crate::autoupdate::spawn_periodic_check(env.clone());

    // Fullscreen palette by default. ANYR_TUI=0 forces inline HUD.
    let cache = Arc::new(Mutex::new(CreditsCache::fresh()));
    #[cfg(feature = "native")]
    if term::is_interactive() {
        let cfg = load_config_if_present(&path).unwrap_or_default();
        let profile = cfg.profiles.get(&cfg.active_profile);
        let base = resolve_base_url(&parsed.flags, profile);
        let key = resolve_api_key(&parsed.flags, env, profile);
        crate::cmd::models::warm_picker_catalog(path.parent(), base.clone(), key.clone(), env);
        kick_credits_refresh(&cache, base, key);
    }
    let inline = !launcher_uses_palette(env);
    // The scriptable form of the last action that changed something,
    // printed once on the way out.
    let mut equivalent: Option<String> = None;
    let done = |code: i32, line: &Option<String>| {
        if let Some(line) = line {
            eprintln!("{}", term::dim(line));
        }
        Ok(code)
    };
    loop {
        if inline {
            let (status, actions) = {
                let mut credits = cache.lock().unwrap_or_else(|p| p.into_inner());
                launcher_hud(&path, parsed, env, &mut credits)
            };
            let labels: Vec<String> = actions.iter().map(|(l, _)| l.clone()).collect();
            let idx = match term::pick_inline(
                &[status],
                "What do you want to do?",
                &labels,
                HUD_FOOTER,
                Some(0),
            ) {
                Ok(i) => i,
                Err(err) if err == "Cancelled." => return done(0, &equivalent),
                Err(err) => return Err(err),
            };
            let action = actions[idx].1.clone();
            match dispatch_noting(&action, parsed, env, &path, &mut equivalent)? {
                LauncherNext::Continue => {}
                LauncherNext::Exit(code) => return done(code, &equivalent),
            }
            continue;
        }
        let (header, entries) = {
            let mut credits = cache.lock().unwrap_or_else(|p| p.into_inner());
            launcher_palette(&path, parsed, env, &mut credits)
        };
        let idx = match pick_palette_action(header, entries.clone(), &cache) {
            Ok(Some(i)) => i,
            Ok(None) => return done(0, &equivalent),
            Err(err) => return Err(err),
        };
        let action = entries[idx].action.clone();
        match dispatch_noting(&action, parsed, env, &path, &mut equivalent)? {
            LauncherNext::Continue => {}
            LauncherNext::Exit(code) => return done(code, &equivalent),
        }
    }
}

/// `launcher_dispatch`, noting the command the action stands for.
fn dispatch_noting(
    action: &str,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
    equivalent: &mut Option<String>,
) -> Result<LauncherNext, String> {
    let before = load_config_if_present(path).unwrap_or_default();
    let next = launcher_dispatch(action, parsed, env, path)?;
    let after = load_config_if_present(path).unwrap_or_default();
    if let Some(line) = equivalent_command(action, &before, &after) {
        *equivalent = Some(line);
    }
    Ok(next)
}

/// The command a HUD action stands for, read from what it changed, so the
/// user learns the scriptable form: a launch, or an agent's model or
/// account. Other actions print nothing.
pub(crate) fn equivalent_command(
    action: &str,
    before: &crate::config::Config,
    after: &crate::config::Config,
) -> Option<String> {
    let bin = crate::help::invoked_bin();
    if action.starts_with("Launch ") {
        return after.last_tool.as_ref().map(|t| format!("{bin} {t}"));
    }
    for (id, _) in crate::install::KNOWN_AGENTS {
        let (b, a) = (before.agent_binding(id), after.agent_binding(id));
        let (mut set, mut unset) = (String::new(), String::new());
        for (flag, old, new) in [
            (
                "model",
                b.and_then(|x| x.default_model.clone()),
                a.and_then(|x| x.default_model.clone()),
            ),
            (
                "profile",
                b.and_then(|x| x.profile.clone()),
                a.and_then(|x| x.profile.clone()),
            ),
        ] {
            match new {
                _ if old == new => {}
                Some(v) => set.push_str(&format!(" --{flag} {v}")),
                None => unset.push_str(&format!(" --{flag}")),
            }
        }
        if !set.is_empty() {
            return Some(format!("{bin} agents set {id}{set}"));
        }
        if !unset.is_empty() {
            return Some(format!("{bin} agents unset {id}{unset}"));
        }
    }
    None
}

pub(crate) fn launcher_hud(
    path: &Path,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    credits: &mut CreditsCache,
) -> (String, Vec<(String, String)>) {
    let cfg = load_config_if_present(path).unwrap_or_default();
    let profile = cfg.profiles.get(&cfg.active_profile);
    let signed_in = resolve_api_key(&parsed.flags, env, profile).is_some();
    let present = available_agents(env, |id| tool_command_for(path, id));
    let last = launcher_last_tool(path, parsed, env);
    let account = credits
        .peek_identity()
        .map(|me| me.display_label())
        .unwrap_or_else(|| cfg.active_profile.clone());
    let model = session_model_label(
        profile
            .map(|p| p.default_model())
            .unwrap_or(crate::config::DEFAULT_MODEL),
    );
    let credits_s = credits.peek_credits();
    let dot = if signed_in {
        term::ok("●")
    } else {
        term::warn("○")
    };
    let credits_shown = if credits_s == "-" {
        term::dim("-")
    } else {
        term::ok(&credits_s)
    };
    let status = format!(
        "{}  {dot} {account}  ·  {}  ·  {last}  ·  {credits_shown}",
        term::accent("anyr"),
        term::model_id(&model),
    );
    let mut actions = Vec::new();
    for id in hud_launch_ids(&present, &last, signed_in) {
        actions.push((format!("Launch {id}"), format!("Launch {id}")));
    }
    actions.push(("Config".into(), "Config".into()));
    actions.push(("Models".into(), "Models".into()));
    actions.push(("Quit".into(), "Quit".into()));
    (status, actions)
}

pub(crate) fn hud_launch_ids(
    present: &[(&'static str, &'static str)],
    last: &str,
    signed_in: bool,
) -> Vec<String> {
    if !signed_in {
        return vec!["claude".into()];
    }
    let pin = if last.is_empty() { "claude" } else { last };
    let mut ids = vec![pin.to_string()];
    for (id, _) in present {
        if *id != pin {
            ids.push((*id).to_string());
        }
    }
    ids
}

#[derive(Debug)]
pub(crate) enum LauncherNext {
    Continue,
    Exit(i32),
}

#[cfg(feature = "native")]
pub(crate) fn push_launch_entries(
    entries: &mut Vec<crate::tui::PaletteEntry>,
    present: &[(&'static str, &'static str)],
    last: &str,
    detail_for: impl Fn(&str) -> String,
) {
    use crate::tui::PaletteEntry;
    if present.is_empty() {
        entries.push(PaletteEntry::new(
            "install an agent…",
            "none detected on PATH",
            "launch",
            "Install agent",
        ));
        return;
    }
    let last = if present.iter().any(|(id, _)| *id == last) {
        last.to_string()
    } else {
        present[0].0.to_string()
    };
    entries.push(PaletteEntry::new(
        last.clone(),
        detail_for(&last),
        "launch",
        format!("Launch {last}"),
    ));
    for (id, _) in present
        .iter()
        .copied()
        .filter(|(id, _)| *id != last.as_str())
    {
        entries.push(PaletteEntry::new(
            id,
            detail_for(id),
            "launch",
            format!("Launch {id}"),
        ));
    }
}

#[cfg(feature = "native")]
pub(crate) fn push_agent_configure_entries(
    entries: &mut Vec<crate::tui::PaletteEntry>,
    agent: &str,
    cfg: &crate::config::Config,
) {
    use crate::tui::PaletteEntry;
    let group = format!("configure · {agent}");
    let routing = cfg
        .agent_binding(agent)
        .map(|b| b.routing.clone())
        .unwrap_or_default();
    entries.push(PaletteEntry::new(
        "model…",
        palette_bind_detail(agent),
        group.clone(),
        format!("Switch model {agent}"),
    ));
    entries.push(PaletteEntry::new(
        "account…",
        palette_bind_detail(agent),
        group.clone(),
        format!("Switch account {agent}"),
    ));
    entries.push(PaletteEntry::new(
        "exacto",
        routing_toggle_detail(routing.wants_exacto(), agent),
        group.clone(),
        format!("Toggle exacto {agent}"),
    ));
    entries.push(PaletteEntry::new(
        "tools",
        routing_toggle_detail(routing.requires_tools(), agent),
        group.clone(),
        format!("Toggle tools {agent}"),
    ));
    entries.push(PaletteEntry::new(
        "1M ctx",
        routing_toggle_detail(routing.requires_1m_context(), agent),
        group,
        format!("Toggle 1m {agent}"),
    ));
}

pub(crate) fn install_agent_dialog(
    path: &Path,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let missing = missing_agents(env, |id| tool_command_for(path, id));
    if missing.is_empty() {
        println!(
            "{}",
            term::dim("Every known coding agent is already on PATH.")
        );
        return Ok(0);
    }
    let labels: Vec<String> = missing
        .iter()
        .map(|(id, label)| {
            let cmd = crate::install::tool_hint(id)
                .map(|h| h.install.to_string())
                .unwrap_or_default();
            format!("{id}  —  {label}    {cmd}")
        })
        .collect();
    let idx = pick_list(
        "Install coding agent",
        &["none of these are on PATH · enter to install".into()],
        &labels,
        Some(0),
    )?;
    let (id, _) = missing[idx];
    let command = tool_command_for(path, id);
    let resolved = ensure_tool_installed(id, &command, Install::Yes)?;
    persist_tool_command(path, id, &resolved)?;
    println!("{}  {id}  {}", term::ok("Installed"), term::dim(&resolved));
    Ok(0)
}

pub(crate) fn launcher_signed_in(
    path: &Path,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> bool {
    stored_api_key(parsed, env, path).is_some()
}

pub(crate) fn launcher_dispatch(
    action: &str,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
) -> Result<LauncherNext, String> {
    if action == "Quit" || action.starts_with("Quit") {
        return Ok(LauncherNext::Exit(0));
    }
    if action.starts_with("Launch ") && action != "Launch coding agent…" {
        let tool = action.trim_start_matches("Launch ").trim();
        // Apply a build staged since the menu started, before the agent
        // starts, so this launch is the "next `anyr claude`" that installs it.
        #[cfg(feature = "native")]
        crate::autoupdate::on_startup(tool, parsed, env);
        // Unsigned HUD's first row is Launch claude. run_launch signs in.
        return Ok(LauncherNext::Exit(run_launch(tool, parsed, env)?));
    }
    if action == "Launch coding agent…" {
        return launch_agent_picker(parsed, env, path);
    }
    if action == "Config" {
        let path = config_path(parsed, env);
        print_config_status(parsed, env, &path)?;
        return Ok(LauncherNext::Continue);
    }
    if action == "Models" {
        // The picker, not the plain list: the launcher redraws over scrollback.
        let mut next = parsed.clone();
        next.command = "models".into();
        next.flags.insert("pick".into(), FlagValue::Bool(true));
        match run_models(&next, env) {
            Ok(_) => {}
            Err(err) if err == "Cancelled." => {}
            Err(err) => eprintln!("{}", term::err(&err)),
        }
        return Ok(LauncherNext::Continue);
    }
    if action == "Install agent" {
        match install_agent_dialog(path, env) {
            Ok(_) => {}
            Err(err) if err == "Cancelled." => {}
            Err(err) => eprintln!("{}", term::err(&err)),
        }
        return Ok(LauncherNext::Continue);
    }
    if action == "Switch model" {
        if !launcher_signed_in(path, parsed, env) {
            eprintln!("{}", term::err("Sign in first (Login / sign in)."));
            return Ok(LauncherNext::Continue);
        }
        let agent = launcher_last_tool(path, parsed, env);
        let mut next = parsed.clone();
        next.command = "models".into();
        next.flags.insert("pick".into(), FlagValue::Bool(true));
        next.flags.insert("agent".into(), FlagValue::Value(agent));
        if let Err(err) = run_models(&next, env) {
            eprintln!("{}", term::err(&err));
        }
        return Ok(LauncherNext::Continue);
    }
    if let Some(agent) = action.strip_prefix("Switch model ") {
        return switch_agent_model(parsed, env, path, agent.trim());
    }
    if action == "Switch agent" {
        match config_edit_row(parsed, env, path, SettingKind::Agent) {
            Ok(_) => {}
            Err(err) if err == "Cancelled." => {}
            Err(err) => eprintln!("{}", term::err(&err)),
        }
        return Ok(LauncherNext::Continue);
    }
    if action == "Switch account" {
        let agent = launcher_last_tool(path, parsed, env);
        let mut next = parsed.clone();
        next.flags.insert("agent".into(), FlagValue::Value(agent));
        match config_account_actions(&next, env) {
            Ok(_) => {}
            Err(err) if err == "Cancelled." => {}
            Err(err) => eprintln!("{}", term::err(&err)),
        }
        return Ok(LauncherNext::Continue);
    }
    if let Some(agent) = action.strip_prefix("Switch account ") {
        return switch_agent_account(parsed, env, path, agent.trim());
    }
    if let Some(agent) = action.strip_prefix("Toggle exacto ") {
        toggle_agent_routing_field(path, agent.trim(), RoutingField::Exacto)?;
        return Ok(LauncherNext::Continue);
    }
    if let Some(agent) = action.strip_prefix("Toggle tools ") {
        toggle_agent_routing_field(path, agent.trim(), RoutingField::Tools)?;
        return Ok(LauncherNext::Continue);
    }
    if let Some(agent) = action.strip_prefix("Toggle 1m ") {
        toggle_agent_routing_field(path, agent.trim(), RoutingField::MinContext)?;
        return Ok(LauncherNext::Continue);
    }
    if action == "Credits" {
        if let Err(err) = run_usage(parsed, env) {
            eprintln!("{}", term::err(&err));
        }
        return Ok(LauncherNext::Continue);
    }
    if action == "Agent onboard prompt…" {
        if let Err(err) = crate::onboard::run("onboard", parsed) {
            eprintln!("{}", term::err(&err));
        }
        return Ok(LauncherNext::Continue);
    }
    if action == "Login / add key" || action == "Login / sign in" {
        if let Err(err) = run_login(parsed, env) {
            eprintln!("{}", term::err(&err));
        }
        return Ok(LauncherNext::Continue);
    }
    Ok(LauncherNext::Continue)
}

pub(crate) fn switch_agent_model(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
    agent: &str,
) -> Result<LauncherNext, String> {
    if agent.is_empty() {
        return Ok(LauncherNext::Continue);
    }
    if !launcher_signed_in(path, parsed, env) {
        eprintln!("{}", term::err("Sign in first (Login / sign in)."));
        return Ok(LauncherNext::Continue);
    }
    match bind_agent_model(parsed, env, path, agent) {
        Ok(_) => {}
        Err(err) if err == "Cancelled." => {}
        Err(err) => eprintln!("{}", term::err(&err)),
    }
    Ok(LauncherNext::Continue)
}

pub(crate) fn bind_agent_model(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
    agent: &str,
) -> Result<i32, String> {
    let existing = load_config_if_present(path);
    let profile = existing
        .as_ref()
        .and_then(|c| c.profiles.get(&c.active_profile));
    let key = resolve_api_key(&parsed.flags, env, profile);
    let base = resolve_base_url(&parsed.flags, profile);
    let models = models_for_picker(path.parent(), &base, key.as_deref(), env);
    if models.models.is_empty() && catalog_lookup_enabled(env) {
        return Err(
            "Could not load the model catalog. Check your connection and try again.".into(),
        );
    }
    let current = existing
        .as_ref()
        .and_then(|c| c.agent_binding(agent))
        .and_then(|b| b.default_model.clone())
        .or_else(|| profile.map(|p| p.default_model().to_string()));
    let id = pick_model(&models, current.as_deref(), &format!("{agent} model"))?;
    save_agent_model(path, agent, &id)
}

pub(crate) fn switch_agent_account(
    _parsed: &ParsedArgs,
    _env: &BTreeMap<String, String>,
    path: &Path,
    agent: &str,
) -> Result<LauncherNext, String> {
    if agent.is_empty() {
        return Ok(LauncherNext::Continue);
    }
    match bind_agent_account(path, agent) {
        Ok(_) => {}
        Err(err) if err == "Cancelled." => {}
        Err(err) => eprintln!("{}", term::err(&err)),
    }
    Ok(LauncherNext::Continue)
}

pub(crate) fn bind_agent_account(path: &Path, agent: &str) -> Result<i32, String> {
    let cfg = load_config_if_present(path).unwrap_or_default();
    let mut names: Vec<String> = cfg.profiles.keys().cloned().collect();
    crate::sort::sort(&mut names);
    if names.is_empty() {
        return Err(no_key_error());
    }
    let current_name = cfg
        .agent_binding(agent)
        .and_then(|b| b.profile.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| cfg.active_profile.clone());
    let labels: Vec<String> = names
        .iter()
        .map(|n| {
            let p = cfg.profiles.get(n);
            let key = mask_api_key(p.and_then(|p| p.api_key.as_deref()));
            let mark = if n == &current_name { "  ●" } else { "" };
            format!("{n}  ·  {key}{mark}")
        })
        .collect();
    let current = names.iter().position(|n| n == &current_name);
    let idx = pick_list(
        &format!("Account for {agent}"),
        &["applies only to this agent · launch list stays put".into()],
        &labels,
        current,
    )?;
    let chosen = names[idx].clone();
    save_agent_account(path, agent, &chosen)
}

pub(crate) fn launch_agent_picker(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
) -> Result<LauncherNext, String> {
    if !launcher_signed_in(path, parsed, env) {
        eprintln!(
            "{}",
            term::warn("Not signed in — login first, or pass --key.")
        );
        if let Err(err) = run_login(parsed, env) {
            eprintln!("{}", term::err(&err));
            return Ok(LauncherNext::Continue);
        }
        if !launcher_signed_in(path, parsed, env) {
            return Ok(LauncherNext::Continue);
        }
    }
    let last = launcher_last_tool(path, parsed, env);
    let present = available_agents(env, |id| tool_command_for(path, id));
    if present.is_empty() {
        if let Err(err) = install_agent_dialog(path, env) {
            if err != "Cancelled." {
                eprintln!("{}", term::err(&err));
            }
        }
        return Ok(LauncherNext::Continue);
    }
    let labels: Vec<String> = present
        .iter()
        .map(|(id, label)| format!("{id}  —  {label}"))
        .collect();
    let current = present.iter().position(|(id, _)| *id == last.as_str());
    let idx = match term::pick("Launch coding agent", &labels, current) {
        Ok(i) => i,
        Err(err) if err == "Cancelled." => return Ok(LauncherNext::Continue),
        Err(err) => {
            eprintln!("{}", term::err(&err));
            return Ok(LauncherNext::Continue);
        }
    };
    let tool = present[idx].0;
    // Same apply-at-"next claude" refresh as the "Launch <tool>" entry.
    #[cfg(feature = "native")]
    crate::autoupdate::on_startup(tool, parsed, env);
    Ok(LauncherNext::Exit(run_launch(tool, parsed, env)?))
}

#[cfg(test)]
mod hud_tests {
    use super::{equivalent_command, hud_launch_ids};

    #[test]
    fn each_hud_change_names_its_scriptable_command() {
        // WHY: the HUD teaches the scriptable form; a wrong line would teach
        // a command that does something else.
        let bin = crate::help::invoked_bin();
        let before = crate::config::Config::default();
        let mut after = before.clone();
        after.agent_binding_mut("claude").default_model = Some("z-ai/glm-4.7".into());
        assert_eq!(
            equivalent_command("Switch model claude", &before, &after).as_deref(),
            Some(format!("{bin} agents set claude --model z-ai/glm-4.7").as_str())
        );
        assert_eq!(
            equivalent_command("Switch model claude", &after, &before).as_deref(),
            Some(format!("{bin} agents unset claude --model").as_str())
        );
        let mut launched = before.clone();
        launched.last_tool = Some("codex".into());
        assert_eq!(
            equivalent_command("Launch codex", &before, &launched).as_deref(),
            Some(format!("{bin} codex").as_str())
        );
        assert_eq!(
            equivalent_command("Toggle exacto claude", &before, &before),
            None
        );
    }

    #[test]
    fn first_run_always_offers_launch_claude() {
        // WHY: after install, Enter on the HUD should launch Claude even
        // when unsigned or when no agent is on PATH yet.
        assert_eq!(
            hud_launch_ids(&[], "claude", false),
            vec!["claude".to_string()]
        );
        assert_eq!(hud_launch_ids(&[], "", true), vec!["claude".to_string()]);
        let present = [("codex", "Codex"), ("grok", "Grok")];
        assert_eq!(
            hud_launch_ids(&present, "claude", true),
            vec![
                "claude".to_string(),
                "codex".to_string(),
                "grok".to_string()
            ]
        );
    }
}
