use std::collections::BTreeMap;

use crate::auth::acquire_api_key;
use crate::config::{write_config, DEFAULT_PROFILE};
use crate::install::{ensure_tool_installed, Install};
use crate::key::{
    load_config_if_present, profile_for_agent, resolve_base_url, resolve_launch_key,
    resolve_launch_model,
};
use crate::parse::{get_string_flag, ParsedArgs};
use crate::spawn::{
    apply_model_id_routing, apply_routing_env, build_tool_env, catalog_context_window,
    catalog_model_id, crush_user_dir, default_profile_for_env, display_model_id, env_command_path,
    fx_connection, fx_settings_path, grok_user_home, is_auto_model, is_user_preset,
    is_virtual_preset, model_args_for, normalize_effort, opencode_standalone_args,
    prepare_crush_config, prepare_grok_home, prepare_pi_wrapper, provider_args_for, render_dry_run,
    resolve_tool, spawn_child, tool_base_url, BuildToolEnvInput, GROK_UNSET_ENV,
};
use crate::spawn::{check_fx_version, fx_version, prepare_fx_settings};
use crate::term;

use crate::cmd::decision::{chat_model_error, is_systemone_model};
use crate::cmd::dispatch::{catalog_lookup_enabled, config_path};
use crate::cmd::login::persist_login;
use crate::cmd::models::apply_claude_alias_flags;

pub(crate) struct ResolvedModel {
    id: String,
    context_window: Option<i64>,
    /// The catalog read to resolve `id`, kept so pi does not read it again.
    catalog: Option<Vec<crate::http::CatalogModel>>,
    refresh: crate::launch_cache::Refresh,
}

/// Where the launch caches live: next to the config file.
pub(crate) struct CacheSpot<'a> {
    pub(crate) dir: Option<&'a std::path::Path>,
    pub(crate) dry_run: bool,
}

/// The catalog the model resolution already read, or the cached one, so an
/// agent that writes its own model list (pi, crush) never fetches it twice.
///
/// With no cached copy it waits longer than a normal launch and says so on
/// stderr unless `quiet`. None means the catalog could not be loaded, so the
/// caller must not write a list as if it were complete.
fn take_catalog(
    resolved: &mut ResolvedModel,
    spot: &CacheSpot<'_>,
    base: &str,
    key: &str,
    quiet: bool,
) -> Option<Vec<crate::http::CatalogModel>> {
    let models = match resolved.catalog.take() {
        Some(models) => Some(models),
        None => {
            let launch = crate::launch_cache::catalog_for_list(
                spot.dir,
                base,
                Some(key),
                spot.dry_run,
                &|| {
                    if !quiet {
                        eprintln!("{} loading the model list", term::dim("anyr:"));
                    }
                },
            );
            resolved.refresh = resolved.refresh.take().or(launch.refresh);
            launch.models
        }
    };
    models.filter(|m| !m.is_empty())
}

/// `take_catalog` for an agent that rebuilds its list on every launch: a
/// failed load leaves only the selected model, and the launch says so.
fn catalog_or_selected(
    resolved: &mut ResolvedModel,
    spot: &CacheSpot<'_>,
    base: &str,
    key: &str,
    quiet: bool,
    agent: &str,
) -> Vec<crate::http::CatalogModel> {
    take_catalog(resolved, spot, base, key, quiet).unwrap_or_else(|| {
        if !quiet && !spot.dry_run {
            eprintln!(
                "{} the model list is unavailable; {agent} lists only the selected model.",
                term::dim("anyr:")
            );
        }
        Vec::new()
    })
}

/// Run `a` and `b` at the same time where threads exist.
#[cfg(feature = "native")]
fn both<A: Send, B: Send>(a: impl FnOnce() -> A + Send, b: impl FnOnce() -> B + Send) -> (A, B) {
    std::thread::scope(|s| {
        let b = s.spawn(b);
        let a = a();
        (a, b.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
    })
}

#[cfg(not(feature = "native"))]
fn both<A, B>(a: impl FnOnce() -> A, b: impl FnOnce() -> B) -> (A, B) {
    (a(), b())
}

pub(crate) fn resolve_session_model(
    requested: &str,
    base: &str,
    key: Option<&str>,
    env: &BTreeMap<String, String>,
    spot: &CacheSpot<'_>,
) -> ResolvedModel {
    let requested = catalog_model_id(requested);
    let id = if is_auto_model(&requested) {
        display_model_id(&requested)
    } else {
        requested
    };
    if is_virtual_preset(&id) || is_user_preset(&id) || !catalog_lookup_enabled(env) {
        // Virtual anyrouter/* stays the preset + floor suffix. Do not remap
        // onto a catalog SKU or inherit auto's 200k listing window.
        return ResolvedModel {
            id,
            context_window: None,
            catalog: None,
            refresh: None,
        };
    }
    let launch = crate::launch_cache::catalog_for_launch(spot.dir, base, key, spot.dry_run);
    ResolvedModel {
        context_window: launch
            .models
            .as_deref()
            .and_then(|models| catalog_context_window(&id, models)),
        id,
        catalog: launch.models,
        refresh: launch.refresh,
    }
}

/// An exit this soon after spawn is a launch failure, not a session ending.
const LAUNCH_FAILURE_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);

/// Spawn the agent. When it fails right away, name a forwarded flag its
/// `--help` does not list: the usual cause is a flag from another agent.
fn run_agent(
    tool_name: &str,
    resolved: &str,
    args: &[String],
    env_map: &BTreeMap<String, String>,
    unset: &[&str],
    forwarded: &[String],
    raw_from: usize,
) -> i32 {
    let started = std::time::Instant::now();
    let code = spawn_child(resolved, args, env_map, unset);
    let forwarded_flags = forwarded.iter().any(|a| a.starts_with("--"));
    if code != 0 && forwarded_flags && started.elapsed() < LAUNCH_FAILURE_WINDOW {
        let hint = crate::spawn::help_text(resolved).and_then(|help| {
            crate::cmd::portable::unknown_flag_hint(tool_name, &help, forwarded, raw_from)
        });
        if let Some(hint) = hint {
            eprintln!("{} {hint}", term::dim("anyr:"));
        }
    }
    code
}

pub(crate) fn run_launch(
    tool_name: &str,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    launch(tool_name, parsed, env, None)
}

/// Headless one-shot run for `anyr delegate`: same gateway env as a normal
/// launch, but the agent gets the task through its non-interactive mode, the
/// saved config is left alone, and `extra_env` is added to the child.
pub(crate) struct Headless {
    pub(crate) args: Vec<String>,
    pub(crate) extra_env: Vec<(String, String)>,
}

pub(crate) fn launch(
    tool_name: &str,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    headless: Option<Headless>,
) -> Result<i32, String> {
    // `--preset <slug>` launches with the model id `@preset/<slug>`.
    let preset_args = crate::cmd::presets::launch_args(parsed)?;
    let preset_slug = preset_args.as_ref().map(|(_, slug)| slug.clone());
    let parsed = preset_args.as_ref().map_or(parsed, |(next, _)| next);
    let path = config_path(parsed, env);
    let existing = load_config_if_present(&path);
    let stored = existing
        .as_ref()
        .and_then(|c| profile_for_agent(c, &parsed.flags, env, tool_name));
    // Reject a known Decisions model before acquiring a key or spawning a chat
    // agent. The explicit flag path is important for non-interactive callers:
    // they should get the routing error, not a login prompt first.
    let guard_profile = stored.cloned().unwrap_or_else(|| {
        default_profile_for_env(Some(&resolve_base_url(&parsed.flags, None)), None)
    });
    let guard_model =
        resolve_launch_model(&parsed.flags, existing.as_ref(), &guard_profile, tool_name);
    if is_systemone_model(&guard_model) {
        return Err(chat_model_error(&guard_model, tool_name));
    }
    let key = if let Some((key, _)) =
        resolve_launch_key(&parsed.flags, env, existing.as_ref(), tool_name)
    {
        key
    } else {
        let base = resolve_base_url(&parsed.flags, stored);
        let acquired = acquire_api_key(&parsed.flags, env, &base, Some(tool_name))?;
        persist_login(parsed, env, &acquired.api_key, &acquired.source)?;
        acquired.api_key
    };
    let existing = load_config_if_present(&path);
    let stored = existing
        .as_ref()
        .and_then(|c| profile_for_agent(c, &parsed.flags, env, tool_name));
    let base = resolve_base_url(&parsed.flags, stored);
    if let Some(slug) = &preset_slug {
        crate::cmd::presets::verify_for_launch(parsed, env, &base, &key, slug)?;
    }
    let mut profile = stored
        .cloned()
        .unwrap_or_else(|| default_profile_for_env(Some(&base), Some(&key)));
    profile.base_url = Some(base.clone());
    let aliases_changed = apply_claude_alias_flags(&mut profile, parsed);
    let tool = resolve_tool(existing.as_ref(), tool_name)?;
    let mut routing = existing
        .as_ref()
        .and_then(|c| c.agent_binding(tool_name))
        .map(|b| b.routing.clone())
        .unwrap_or_default();
    let requested = apply_model_id_routing(
        &resolve_launch_model(&parsed.flags, existing.as_ref(), &profile, tool_name),
        &mut routing,
    );
    let quiet = term::machine_output(parsed, env);
    let spot = CacheSpot {
        dir: path.parent(),
        dry_run: parsed.flag_true("dry-run"),
    };
    // A cold launch asks for the catalog and the MCP setting at the same time.
    let (mut resolved, mcp_decision) = both(
        || resolve_session_model(&requested, &base, Some(&key), env, &spot),
        || {
            crate::mcp_wiring::decide(&crate::mcp_wiring::DecideInput {
                tool: tool_name,
                no_mcp_flag: parsed.flag_true("no-mcp"),
                config: existing.as_ref(),
                base: &base,
                key: &key,
                cache_dir: path.parent(),
                dry_run: parsed.flag_true("dry-run"),
            })
        },
    );
    let model = std::mem::take(&mut resolved.id);
    if is_systemone_model(&model) {
        return Err(chat_model_error(&model, tool_name));
    }
    let effort = normalize_effort(get_string_flag(&parsed.flags, "effort").as_deref())?;
    let model_mode = if is_auto_model(&model) {
        "auto"
    } else {
        "concrete"
    };
    let mut env_map = build_tool_env(BuildToolEnvInput {
        tool_name,
        tool: &tool,
        profile: &profile,
        api_key: &key,
        model: &model,
        effort: effort.as_deref(),
        context_window: resolved.context_window,
        min_context: routing.min_context,
        model_map: None,
    });
    apply_routing_env(&mut env_map, &routing, tool_name);
    // An `anyr` the agent runs itself must not update or print mid-session.
    env_map.insert(crate::autoupdate::ENV_LAUNCHER_CHILD.into(), "1".into());
    if tool_name == "pi" {
        let catalog = catalog_or_selected(&mut resolved, &spot, &base, &key, quiet, "pi");
        prepare_pi_wrapper(&mut env_map, &path, &profile, &tool, &model, &catalog)?;
    }
    if tool_name == "grok" {
        let user_home = grok_user_home(&|k| env.get(k).cloned());
        prepare_grok_home(&mut env_map, &path, &key, user_home.as_deref())?;
    }
    let mut args = Vec::new();
    args.extend(provider_args_for(tool_name, &profile));
    let user_picked_model = tool_name == "kilo"
        && parsed
            .passthrough
            .iter()
            .any(|a| a == "-m" || a == "--model" || a.starts_with("--model="));
    if !user_picked_model {
        args.extend(model_args_for(
            tool_name,
            &model,
            model_mode,
            resolved.context_window,
        ));
    }
    // Wire the AnyRouter MCP gateway in when the workspace asks for it.
    let mcp = crate::mcp_wiring::apply(
        &crate::mcp_wiring::ApplyInput {
            tool: tool_name,
            decision: &mcp_decision,
            key: &key,
            env,
            config_path: &path,
            dry_run: parsed.flag_true("dry-run"),
        },
        &mut args,
        &mut env_map,
    );
    if tool_name == "crush" {
        let catalog = if catalog_lookup_enabled(env) {
            catalog_or_selected(&mut resolved, &spot, &base, &key, quiet, "crush")
        } else {
            Vec::new()
        };
        let user_dir = crush_user_dir(&|k| env.get(k).cloned());
        let mcp_url = match &mcp_decision {
            crate::mcp_wiring::Decision::On { url } => Some(url.as_str()),
            _ => None,
        };
        prepare_crush_config(
            &mut env_map,
            &path,
            &profile,
            &tool,
            &model,
            &catalog,
            user_dir.as_deref(),
            mcp_url,
        )?;
    }
    let mut agent_args = Vec::new();
    // Saved `launcher.<agent>.args` come after the mapped portable switches and
    // before headless and user args, so the user's own flags come last and win.
    if let Some(binding) = existing.as_ref().and_then(|c| c.agent_binding(tool_name)) {
        agent_args.extend(binding.args.iter().cloned());
    }
    // Written only once the binary is found, so a missing fx leaves ~/.fx alone.
    let fx_settings = if tool_name == "fx" {
        let settings = fx_settings_path(&|k| env.get(k).cloned())
            .ok_or("fx keeps its settings in $HOME/.fx; set HOME.")?;
        let loaded = if catalog_lookup_enabled(env) {
            take_catalog(&mut resolved, &spot, &base, &key, quiet)
        } else {
            Some(Vec::new())
        };
        let mut connection = fx_connection(
            &tool_base_url(&profile, &tool),
            &tool.auth_env,
            &model,
            loaded.as_deref().unwrap_or_default(),
        );
        if loaded.is_none() {
            // Keep what an earlier launch listed rather than shrink `/model`.
            let previous = crate::spawn::fx_listed_models(&settings);
            if !quiet && !spot.dry_run {
                let from = if previous.is_empty() {
                    "the model list is unavailable; fx lists only the selected model."
                } else {
                    "the model list is unavailable; fx keeps the list from the last launch."
                };
                eprintln!("{} {from}", term::dim("anyr:"));
            }
            crate::spawn::fx_keep_listed_models(&mut connection, previous);
        }
        Some((settings, connection))
    } else {
        None
    };
    let fx_ready = |resolved: &str| -> Result<(), String> {
        let Some((settings, connection)) = &fx_settings else {
            return Ok(());
        };
        check_fx_version(fx_version(resolved).as_deref())?;
        prepare_fx_settings(settings, connection, false).map(|_| ())
    };
    if let Some(h) = &headless {
        agent_args.extend(h.args.clone());
        env_map.extend(h.extra_env.iter().cloned());
    }
    let raw_from = agent_args.len() + parsed.raw_from.unwrap_or(parsed.passthrough.len());
    agent_args.extend(parsed.passthrough.clone());
    let mut lead = Vec::new();
    if let Some(effort) = &effort {
        lead.push(("effort", Some(effort.clone())));
    }
    if parsed.flag_true("yolo") || tool.extra_flag("yolo") {
        lead.push(("yolo", None));
    }
    for switch in ["skills-off", "tools-off"] {
        if parsed.flag_true(switch) {
            lead.push((switch, None));
        }
    }
    let mapped = crate::cmd::portable::apply(tool_name, &lead, &agent_args, raw_from)?;
    if !term::machine_output(parsed, env) {
        for notice in &mapped.notices {
            eprintln!("{} {notice}", term::dim("note:"));
        }
    }
    args.extend(mapped.args);
    env_map.extend(
        mapped
            .env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string())),
    );
    let unset: &[&str] = if tool_name == "grok" {
        GROK_UNSET_ENV
    } else {
        &[]
    };
    let command = get_string_flag(&parsed.flags, "command-path")
        .or_else(|| env_command_path(tool_name, env))
        .unwrap_or_else(|| tool.command.clone());
    if parsed.flag_true("dry-run") {
        if tool_name == "opencode" {
            if let Some(path) = crate::install::resolve_executable(&command) {
                args = opencode_standalone_args(
                    &args,
                    crate::launch_cache::opencode_major(spot.dir, &path),
                );
            }
        }
        println!("{}", render_dry_run(&command, &args, &env_map));
        println!("{}", mcp.summary);
        if let Some((settings, connection)) = &fx_settings {
            println!("{}", prepare_fx_settings(settings, connection, true)?);
        }
        return Ok(0);
    }
    let scripted = term::agent_mode(parsed, env);
    if headless.is_none() && !scripted {
        if let Some(hint) = crate::mcp_wiring::unsupported_hint(tool_name, &mcp_decision) {
            eprintln!("{hint}");
        }
    }
    let install = if parsed.flag_true("install") {
        Install::Yes
    } else if scripted {
        Install::Never
    } else {
        Install::Ask
    };
    if headless.is_some() {
        crate::api::confirm(
            parsed,
            &format!(
                "Run {tool_name} headlessly with model {model}? An agent run can make many model calls."
            ),
        )?;
        let resolved = ensure_tool_installed(tool_name, &command, install)?;
        fx_ready(&resolved)?;
        if tool_name == "opencode" {
            args = opencode_standalone_args(
                &args,
                crate::launch_cache::opencode_major(spot.dir, &resolved),
            );
        }
        return Ok(run_agent(
            tool_name,
            &resolved,
            &args,
            &env_map,
            unset,
            &parsed.passthrough,
            parsed.raw_from.unwrap_or(parsed.passthrough.len()),
        ));
    }
    let resolved = ensure_tool_installed(tool_name, &command, install)?;
    fx_ready(&resolved)?;
    // Persist on top of the existing config when there is one; a fresh setup
    // (no config yet) gets one so last_tool and the model are remembered.
    let mut cfg = existing.clone().unwrap_or_else(|| crate::config::Config {
        active_profile: DEFAULT_PROFILE.into(),
        ..crate::config::Config::default()
    });
    // First launch on this machine: no stored profile yet — seed one from
    // what this launch resolved so the key, base URL, and model stick.
    cfg.profiles
        .entry(cfg.active_profile.clone())
        .or_insert_with(|| profile.clone());
    cfg.last_tool = Some(tool_name.to_string());
    if aliases_changed {
        if let Some(p) = cfg.profiles.get_mut(&cfg.active_profile) {
            p.claude_haiku = profile.claude_haiku.clone();
            p.claude_sonnet = profile.claude_sonnet.clone();
            p.claude_opus = profile.claude_opus.clone();
            p.claude_fable = profile.claude_fable.clone();
        }
    }
    // Remember the model this launch used as the session default, so a bare
    // `{bin} claude` next time starts with it. Also pin it on this agent so a
    // later grok launch does not inherit claude's model.
    // A preset launch is one-off: it never becomes the saved default model.
    let flag_model = get_string_flag(&parsed.flags, "model").filter(|_| preset_slug.is_none());
    if let Some(flag_model) = flag_model {
        let id = catalog_model_id(&flag_model);
        let stored = if is_auto_model(&id) { None } else { Some(id) };
        if let Some(p) = cfg.profiles.get_mut(&cfg.active_profile) {
            // Auto stays unset so the next launch keeps the anyrouter/auto preset.
            p.default_model = stored.clone();
        }
        cfg.agent_binding_mut(tool_name).default_model = stored;
        cfg.prune_empty_agents();
    }
    let _ = write_config(&cfg, &path);
    if tool_name == "opencode" {
        args = opencode_standalone_args(
            &args,
            crate::launch_cache::opencode_major(spot.dir, &resolved),
        );
    }
    Ok(run_agent(
        tool_name,
        &resolved,
        &args,
        &env_map,
        unset,
        &parsed.passthrough,
        parsed.raw_from.unwrap_or(parsed.passthrough.len()),
    ))
}
