use std::collections::BTreeMap;

use crate::auth::acquire_api_key;
use crate::config::{write_config, DEFAULT_PROFILE};
use crate::install::{ensure_tool_installed, Install};
use crate::key::{
    load_config_if_present, mask_api_key, mask_secrets, profile_for_agent, resolve_base_url,
    resolve_launch_key, resolve_launch_model, KeySource,
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
    #[cfg(feature = "native")]
    crate::counters::spawning(tool_name);
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

/// `--json` (or `ANYR_OUTPUT=json`) was asked for, so `--dry-run` must print
/// one JSON object instead of the text form. `commands::run` already folds
/// the env var into the parsed `json` flag, but direct callers may not.
fn dry_run_json_requested(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> bool {
    parsed.flag_true("json")
        || env
            .get("ANYR_OUTPUT")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("json"))
}

/// Env for the JSON plan: every value goes through the text dry-run's
/// `redact_value` (name-based) and then `mask_secrets` (catches a full key
/// embedded in a larger string, e.g. a JSON config value) so nothing leaks.
fn redacted_env(env_map: &BTreeMap<String, String>) -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    for (k, v) in env_map {
        out.insert(
            k.clone(),
            serde_json::Value::String(mask_secrets(&crate::spawn::redact_value(k, v))),
        );
    }
    out
}

/// What, if anything, keeps a plan from being executed. These are plan-level
/// decisions; the process exit table in `crate::exit` is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Need {
    Ready,
    KeySetup,
    Install,
    Auth,
    Unsupported,
}

impl Need {
    /// The code `settle` reports for this need: 0 launched, 2 key setup,
    /// 3 install, 4 auth, 5 unsupported.
    pub(crate) fn code(self) -> i32 {
        match self {
            Need::Ready => crate::exit::OK,
            Need::KeySetup => 2,
            Need::Install => 3,
            Need::Auth => crate::exit::AUTH,
            Need::Unsupported => 5,
        }
    }
}

/// A launch, fully resolved and safe to inspect before anything is spawned.
/// `plan()` is pure; `settle()` reads this and decides what still needs doing;
/// the dry-run JSON renders from the same object, so the preview and the real
/// spawn cannot drift apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LaunchPlan {
    pub(crate) tool_name: String,
    pub(crate) profile: String,
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) mcp_decision: crate::mcp_wiring::Decision,
    pub(crate) agent_mode: bool,
    pub(crate) effort: Option<String>,
    pub(crate) context_window: Option<i64>,
    pub(crate) unset: Vec<String>,
    // Facts used for rendering and settling, never printed raw.
    key_masked: String,
    mcp_summary: String,
    fx_preview: Option<String>,
    dry_run: bool,
    has_key: bool,
    auth_failed: bool,
}

/// Everything `plan()` needs, already gathered by `launch()`. Every field is
/// an input the planner actually uses; no hidden I/O or prompts happen here.
#[derive(Clone, Copy)]
pub(crate) struct PlanInput<'a> {
    pub(crate) tool_name: &'a str,
    pub(crate) profile: &'a str,
    pub(crate) command: &'a str,
    pub(crate) args: &'a [String],
    pub(crate) env: &'a BTreeMap<String, String>,
    pub(crate) model: &'a str,
    pub(crate) base_url: &'a str,
    pub(crate) key: &'a str,
    pub(crate) effort: Option<&'a str>,
    pub(crate) context_window: Option<i64>,
    pub(crate) unset: &'a [&'a str],
    pub(crate) mcp_decision: &'a crate::mcp_wiring::Decision,
    pub(crate) mcp_summary: &'a str,
    pub(crate) fx_preview: Option<&'a str>,
    pub(crate) agent_mode: bool,
    pub(crate) dry_run: bool,
    pub(crate) auth_failed: bool,
}

/// Pure gather → plan step. Resolves flags/env/config/profile into one value
/// that can be unit-tested without a TTY, network or config file.
pub(crate) fn plan(input: PlanInput<'_>) -> LaunchPlan {
    LaunchPlan {
        tool_name: input.tool_name.to_string(),
        profile: input.profile.to_string(),
        command: input.command.to_string(),
        args: input.args.to_vec(),
        env: input.env.clone(),
        model: input.model.to_string(),
        base_url: input.base_url.to_string(),
        mcp_decision: input.mcp_decision.clone(),
        agent_mode: input.agent_mode,
        effort: input.effort.map(str::to_string),
        context_window: input.context_window,
        unset: input.unset.iter().map(|s| (*s).to_string()).collect(),
        key_masked: mask_api_key(Some(input.key)),
        mcp_summary: input.mcp_summary.to_string(),
        fx_preview: input.fx_preview.map(str::to_string),
        dry_run: input.dry_run,
        has_key: !input.key.trim().is_empty(),
        auth_failed: input.auth_failed,
    }
}

/// plan → exit decision. Called once after gather and again (on the settled
/// plan) before spawn; no prompts or installs happen inside.
pub(crate) fn settle(plan: &LaunchPlan) -> Need {
    if plan.auth_failed {
        return Need::Auth;
    }
    if plan.model.is_empty() || plan.base_url.is_empty() {
        return Need::Unsupported;
    }
    if !plan.has_key {
        return Need::KeySetup;
    }
    if plan.command.is_empty() {
        return Need::Install;
    }
    Need::Ready
}

impl LaunchPlan {
    /// The settled plan as one JSON object: command, args, redacted env,
    /// model, base URL, masked key, and the decision fields an agent can act
    /// on. Single line so `jq` gets exactly one document, nothing else.
    pub(crate) fn to_json(&self) -> String {
        let masked_args: Vec<serde_json::Value> = self
            .args
            .iter()
            .map(|a| serde_json::Value::String(mask_secrets(a)))
            .collect();
        let mut obj = serde_json::json!({
            "agent": self.tool_name,
            "command": mask_secrets(&self.command),
            "args": masked_args,
            "env": redacted_env(&self.env),
            "model": mask_secrets(&self.model),
            "base_url": mask_secrets(&self.base_url),
            "key_masked": self.key_masked,
            "effort": self.effort,
            "context_window": self.context_window,
            "unset": self.unset,
            "dry_run": self.dry_run,
            "mcp": mask_secrets(&self.mcp_summary),
            "agent_mode": self.agent_mode,
        });
        if let Some(preview) = &self.fx_preview {
            obj["fx_settings"] = serde_json::Value::String(mask_secrets(preview));
        }
        serde_json::to_string(&obj).unwrap_or_else(|_| "{}".into())
    }
}

/// Headless one-shot run for `anyr delegate`: same gateway env as a normal
/// launch, but the agent gets the task through its non-interactive mode, the
/// saved config is left alone, and `extra_env` is added to the child.
pub(crate) struct Headless {
    pub(crate) args: Vec<String>,
    pub(crate) extra_env: Vec<(String, String)>,
}

#[allow(clippy::too_many_arguments)]
/// The settle gate just before a spawn: a plan that is not `Ready` is a
/// plan-level need (2/3/4/5), reported with the same tagged-error convention
/// as the rest of the CLI. Real gathers always reach `Ready`; this keeps the
/// plan the single source of truth for "can we launch".
fn check_ready(
    tool_name: &str,
    profile: &str,
    command: &str,
    args: &[String],
    env_map: &BTreeMap<String, String>,
    key: &str,
    model: &str,
    base: &str,
    effort: Option<&str>,
    context_window: Option<i64>,
    unset: &[&str],
    mcp_decision: &crate::mcp_wiring::Decision,
    mcp_summary: &str,
    agent_mode: bool,
) -> Result<(), String> {
    let planned = plan(PlanInput {
        tool_name,
        profile,
        command,
        args,
        env: env_map,
        model,
        base_url: base,
        key,
        effort,
        context_window,
        unset,
        mcp_decision,
        mcp_summary,
        fx_preview: None,
        agent_mode,
        dry_run: false,
        auth_failed: false,
    });
    match settle(&planned) {
        Need::Ready => Ok(()),
        need => Err(crate::exit::tag(
            need.code(),
            format!("cannot launch {tool_name}: {need:?}"),
        )),
    }
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
    // Keys from `--key` or ANYROUTER_API_KEY must never reach config.yaml:
    // they stay in the process env only. `anyr login` is the only path that
    // writes a key to disk (and it does so through `persist_login` below).
    let (key, ephemeral_key) = if let Some((key, source)) =
        resolve_launch_key(&parsed.flags, env, existing.as_ref(), tool_name)
    {
        (key, matches!(source, KeySource::Flag | KeySource::Env))
    } else {
        let base = resolve_base_url(&parsed.flags, stored);
        let acquired = acquire_api_key(&parsed.flags, env, &base, Some(tool_name))?;
        persist_login(parsed, env, &acquired.api_key, &acquired.source)?;
        (acquired.api_key, false)
    };
    let existing = load_config_if_present(&path);
    let stored = existing
        .as_ref()
        .and_then(|c| profile_for_agent(c, &parsed.flags, env, tool_name));
    let base = resolve_base_url(&parsed.flags, stored);
    if let Some(slug) = &preset_slug {
        crate::cmd::presets::verify_for_launch(parsed, env, &base, &key, slug)?;
    }
    let profile_name = get_string_flag(&parsed.flags, "profile")
        .or_else(|| env.get("ANYROUTER_PROFILE").cloned())
        .or_else(|| existing.as_ref().map(|c| c.active_profile.clone()))
        .unwrap_or_else(|| DEFAULT_PROFILE.to_string());
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
    // Kept separately: `resolved` is shadowed by the installed binary path
    // in both spawn branches, but the plan still needs the catalog window.
    let context_window = resolved.context_window;
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
        prepare_grok_home(&mut env_map, &path, &key, user_home.as_deref(), &model)?;
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
            context_window,
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
        if dry_run_json_requested(parsed, env) {
            let fx_preview = if let Some((settings, connection)) = &fx_settings {
                Some(prepare_fx_settings(settings, connection, true)?)
            } else {
                None
            };
            let plan = plan(PlanInput {
                tool_name,
                profile: &profile_name,
                command: &command,
                args: &args,
                env: &env_map,
                model: &model,
                base_url: &base,
                key: &key,
                effort: effort.as_deref(),
                context_window,
                unset,
                mcp_decision: &mcp_decision,
                mcp_summary: &mcp.summary,
                fx_preview: fx_preview.as_deref(),
                agent_mode: term::agent_mode(parsed, env),
                dry_run: true,
                auth_failed: false,
            });
            println!("{}", plan.to_json());
            return Ok(settle(&plan).code());
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
        check_ready(
            tool_name,
            &profile_name,
            &resolved,
            &args,
            &env_map,
            &key,
            &model,
            &base,
            effort.as_deref(),
            context_window,
            unset,
            &mcp_decision,
            &mcp.summary,
            term::agent_mode(parsed, env),
        )?;
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
    // what this launch resolved so base URL, model and last tool stick. An
    // ephemeral (--key / env) key is dropped here on purpose: only the login
    // flow persists secrets to disk.
    cfg.profiles
        .entry(cfg.active_profile.clone())
        .or_insert_with(|| {
            let mut seeded = profile.clone();
            if ephemeral_key {
                seeded.api_key = None;
            }
            seeded
        });
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
    check_ready(
        tool_name,
        &profile_name,
        &resolved,
        &args,
        &env_map,
        &key,
        &model,
        &base,
        effort.as_deref(),
        context_window,
        unset,
        &mcp_decision,
        &mcp.summary,
        term::agent_mode(parsed, env),
    )?;
    // Stage a new build while the agent runs; it applies on the next start,
    // so the running agent is never disturbed. Headless runs skip this: they
    // are short and non-interactive.
    #[cfg(feature = "native")]
    let _check_ticker = crate::autoupdate::spawn_periodic_check(env.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_wiring::Decision;

    const KEY: &str = "sk-ar-v1-testsecret0123456789abcd";

    #[allow(clippy::too_many_arguments)]
    fn facts<'a>(
        tool: &'a str,
        profile: &'a str,
        command: &'a str,
        args: &'a [String],
        env: &'a BTreeMap<String, String>,
        model: &'a str,
        base: &'a str,
        key: &'a str,
        mcp: &'a Decision,
        agent_mode: bool,
        dry_run: bool,
    ) -> PlanInput<'a> {
        PlanInput {
            tool_name: tool,
            profile,
            command,
            args,
            env,
            model,
            base_url: base,
            key,
            effort: Some("high"),
            context_window: Some(200_000),
            unset: &["ANYROUTER_API_KEY"],
            mcp_decision: mcp,
            mcp_summary: "MCP gateway: on",
            fx_preview: None,
            agent_mode,
            dry_run,
            auth_failed: false,
        }
    }

    /// (Person, Script) x (flag, env, config, profile) x (dry-run, real).
    /// `plan()` is pure: the same facts always give the same plan, and no
    /// branch changes the exit decision for a fully resolved launch.
    #[test]
    fn plan_matrix_preserves_gathered_facts_for_every_case() {
        let args = vec!["--model".to_string(), "openai/gpt-5".to_string()];
        let mut env = BTreeMap::new();
        env.insert("ANYROUTER_API_KEY".into(), KEY.into());
        env.insert("HOME".into(), "/home/person".into());
        let mcp_on = Decision::On {
            url: "https://anyrouter.dev/mcp".into(),
        };
        let mcp_off = Decision::Off("no workspace".into());

        for (who, interactive) in [("person", false), ("script", true)] {
            for source in ["flag", "env", "config", "profile"] {
                for (dry_run, mcp) in [(true, &mcp_on), (false, &mcp_off)] {
                    let profile = format!("{who}-{source}");
                    let planned = plan(facts(
                        "claude",
                        &profile,
                        "/usr/bin/claude",
                        &args,
                        &env,
                        "openai/gpt-5",
                        "https://anyrouter.dev/api/v1",
                        KEY,
                        mcp,
                        interactive,
                        dry_run,
                    ));
                    assert_eq!(planned.tool_name, "claude");
                    assert_eq!(planned.profile, profile);
                    assert_eq!(planned.command, "/usr/bin/claude");
                    assert_eq!(planned.args, args);
                    assert_eq!(planned.env, env);
                    assert_eq!(planned.model, "openai/gpt-5");
                    assert_eq!(planned.base_url, "https://anyrouter.dev/api/v1");
                    assert_eq!(planned.mcp_decision, *mcp);
                    assert_eq!(planned.agent_mode, interactive);
                    assert_eq!(planned.effort.as_deref(), Some("high"));
                    assert_eq!(planned.context_window, Some(200_000));
                    assert_eq!(planned.unset, vec!["ANYROUTER_API_KEY"]);
                    assert!(planned.dry_run == dry_run);
                    assert!(!planned.key_masked.contains(KEY));
                    assert_eq!(settle(&planned), Need::Ready);
                    assert_eq!(settle(&planned).code(), 0);
                }
            }
        }
    }

    #[test]
    fn settle_maps_unmet_needs_to_the_documented_codes() {
        let args: Vec<String> = vec![];
        let env = BTreeMap::new();
        let base = facts(
            "claude",
            "default",
            "/usr/bin/claude",
            &args,
            &env,
            "openai/gpt-5",
            "https://anyrouter.dev/api/v1",
            KEY,
            &Decision::Unchecked,
            true,
            false,
        );
        assert_eq!(settle(&plan(base)).code(), 0);

        let mut no_key = base;
        no_key.key = "";
        assert_eq!(settle(&plan(no_key)), Need::KeySetup);
        assert_eq!(settle(&plan(no_key)).code(), 2);

        let no_command = PlanInput {
            command: "",
            ..base
        };
        assert_eq!(settle(&plan(no_command)).code(), 3);

        let auth = PlanInput {
            auth_failed: true,
            ..base
        };
        assert_eq!(settle(&plan(auth)), Need::Auth);
        assert_eq!(settle(&plan(auth)).code(), crate::exit::AUTH);

        let no_model = PlanInput { model: "", ..base };
        assert_eq!(settle(&plan(no_model)), Need::Unsupported);
        assert_eq!(settle(&plan(no_model)).code(), 5);

        let no_base = PlanInput {
            base_url: "",
            ..base
        };
        assert_eq!(settle(&plan(no_base)).code(), 5);
    }

    /// The dry-run output renders from the plan and redacts every secret.
    #[test]
    fn plan_json_matches_the_dry_run_contract_and_redacts_secrets() {
        let args = vec!["--model".to_string(), KEY.to_string()];
        let mut env = BTreeMap::new();
        env.insert("ANYROUTER_API_KEY".into(), KEY.into());
        let mut planned = plan(facts(
            "claude",
            "default",
            "/usr/bin/claude",
            &args,
            &env,
            "openai/gpt-5",
            "https://anyrouter.dev/api/v1",
            KEY,
            &Decision::Unchecked,
            true,
            true,
        ));
        planned.fx_preview = Some(format!("fx config with {KEY}"));
        let json = planned.to_json();
        assert!(!json.contains(KEY), "{json}");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["agent"], "claude");
        assert_eq!(v["command"], "/usr/bin/claude");
        assert_eq!(v["model"], "openai/gpt-5");
        assert_eq!(v["base_url"], "https://anyrouter.dev/api/v1");
        assert_eq!(v["dry_run"], true);
        assert_eq!(v["agent_mode"], true);
        assert_eq!(v["effort"], "high");
        assert_eq!(v["context_window"], 200_000);
        assert_eq!(v["unset"], serde_json::json!(["ANYROUTER_API_KEY"]));
        assert!(v.get("key_masked").is_some());
        assert!(v.get("mcp").is_some());
        assert!(v.get("fx_settings").is_some());
        // Every env value is redacted by name AND by content.
        let env_obj = v["env"].as_object().unwrap();
        assert!(env_obj.contains_key("ANYROUTER_API_KEY"));
        assert!(!format!("{env_obj:?}").contains(KEY));
    }
}
