use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::Path;

use crate::config::{write_config, Profile};
use crate::http::{
    fetch_credits, fetch_intelligence, fetch_models, format_models_list, CatalogModel,
};
use crate::install::KNOWN_AGENTS;
use crate::key::{load_config_if_present, no_key_error, resolve_api_key, resolve_base_url};
use crate::launch_cache::{catalog_with, now_secs, scores_with, Fetch, ScoresFetch};
use crate::parse::{get_string_flag, ParsedArgs};
use crate::spawn::{
    canonical_tool, catalog_model_id, display_model_id, is_auto_model, session_model_label,
};
use crate::term;

use crate::cmd::dispatch::{catalog_lookup_enabled, config_path, hint, tui_wants_dump};

pub(crate) fn pick_ids(models: &[CatalogModel]) -> Vec<String> {
    let mut ids = vec!["anyrouter/auto".into()];
    let mut catalog: Vec<String> = models
        .iter()
        .map(|m| catalog_model_id(&m.id))
        .filter(|id| !id.is_empty() && !is_auto_model(id))
        .collect();
    catalog.sort();
    catalog.dedup();
    for id in catalog {
        if !ids.iter().any(|existing| existing == &id) {
            ids.push(id);
        }
    }
    ids
}

/// What the model picker shows: the catalog plus intelligence scores by id.
#[derive(Debug, Clone, Default)]
pub(crate) struct PickerCatalog {
    pub(crate) models: Vec<CatalogModel>,
    /// Read only by the native table; the browser demo lists ids.
    #[cfg_attr(not(feature = "native"), allow(dead_code))]
    pub(crate) smart: BTreeMap<String, f64>,
}

/// The launcher's background load of the picker catalog, joined by the first
/// picker that opens while it still runs.
#[cfg(feature = "native")]
static WARMING: std::sync::Mutex<Option<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(None);

impl PickerCatalog {
    /// The catalog plus scores from the public benchmarks feed, when lookups are on.
    pub(crate) fn new(
        dir: Option<&Path>,
        base: &str,
        mut models: Vec<CatalogModel>,
        env: &BTreeMap<String, String>,
    ) -> Self {
        models.retain(CatalogModel::is_routable);
        let smart = if catalog_lookup_enabled(env) {
            scores_with(dir, base, now_secs(), &fetch_intelligence)
        } else {
            BTreeMap::new()
        };
        Self { models, smart }
    }

    /// The catalog and scores from the disk cache, each fetched only when
    /// there is no usable copy.
    fn load(
        dir: Option<&Path>,
        base: &str,
        key: Option<&str>,
        now: u64,
        fetch: &'static Fetch,
        scores: &'static ScoresFetch,
    ) -> Self {
        let mut models = catalog_with(dir, base, key, false, now, fetch)
            .models
            .unwrap_or_default();
        models.retain(CatalogModel::is_routable);
        Self {
            models,
            smart: scores_with(dir, base, now, scores),
        }
    }
}

fn load_picker_catalog(dir: Option<&Path>, base: &str, key: Option<&str>) -> PickerCatalog {
    PickerCatalog::load(
        dir,
        base,
        key,
        now_secs(),
        &fetch_models,
        &fetch_intelligence,
    )
}

/// Load the picker catalog on a background thread so a picker opened from
/// the launcher finds it ready.
#[cfg(feature = "native")]
pub(crate) fn warm_picker_catalog(
    dir: Option<&Path>,
    base: String,
    key: Option<String>,
    env: &BTreeMap<String, String>,
) {
    if !catalog_lookup_enabled(env) {
        return;
    }
    let dir = dir.map(Path::to_path_buf);
    let handle = std::thread::Builder::new()
        .name("anyr-picker".into())
        .spawn(move || {
            load_picker_catalog(dir.as_deref(), &base, key.as_deref());
        })
        .ok();
    *WARMING.lock().unwrap_or_else(|p| p.into_inner()) = handle;
}

/// `dir` holds the catalog cache: the config file's directory.
pub(crate) fn models_for_picker(
    dir: Option<&Path>,
    base: &str,
    key: Option<&str>,
    env: &BTreeMap<String, String>,
) -> PickerCatalog {
    if !catalog_lookup_enabled(env) {
        return PickerCatalog::default();
    }
    #[cfg(feature = "native")]
    {
        let warming = WARMING.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(warming) = warming {
            if !warming.is_finished() && term::is_interactive() {
                eprintln!("{}", term::dim("Loading models…"));
            }
            let _ = warming.join();
        }
    }
    load_picker_catalog(dir, base, key)
}

fn shown_index(ids: &[String], current: Option<&str>) -> Option<usize> {
    let id = catalog_model_id(current?);
    ids.iter()
        .position(|s| s == &id || (is_auto_model(&id) && is_auto_model(s)))
}

pub(crate) fn render_model_picker_dump(
    catalog: &PickerCatalog,
    current: Option<&str>,
    env: &BTreeMap<String, String>,
) -> String {
    let ids = pick_ids(&catalog.models);
    let shown_idx = shown_index(&ids, current);
    #[cfg(feature = "native")]
    {
        let rows = crate::cmd::model_picker::model_rows(&ids, catalog, crate::civil::now_secs());
        crate::tui::dump_pick_table(
            "Default model",
            &ids,
            crate::cmd::model_picker::model_table(&rows),
            shown_idx,
            crate::tui::dump_cols(env),
        )
    }
    #[cfg(not(feature = "native"))]
    {
        let _ = (env, shown_idx);
        format!("{}\n", ids.join("\n"))
    }
}

pub(crate) fn pick_list(
    title: &str,
    header: &[String],
    items: &[String],
    current: Option<usize>,
) -> Result<usize, String> {
    #[cfg(feature = "native")]
    {
        crate::tui::pick_with_header(title, header, items, current)
    }
    #[cfg(not(feature = "native"))]
    {
        let _ = header;
        term::pick(title, items, current)
    }
}

pub(crate) fn pick_model(
    catalog: &PickerCatalog,
    current: Option<&str>,
    title: &str,
) -> Result<String, String> {
    let ids = pick_ids(&catalog.models);
    let shown_idx = shown_index(&ids, current);
    #[cfg(feature = "native")]
    let idx = {
        let rows = crate::cmd::model_picker::model_rows(&ids, catalog, crate::civil::now_secs());
        crate::tui::pick_table(
            title,
            &ids,
            crate::cmd::model_picker::model_table(&rows),
            shown_idx,
        )?
    };
    #[cfg(not(feature = "native"))]
    let idx = { term::pick(title, &ids, shown_idx)? };
    Ok(ids[idx].clone())
}

pub(crate) fn set_model_slot(profile: &mut Profile, slot: &str, id: String) {
    let id = catalog_model_id(&id);
    match slot {
        "haiku" => profile.claude_haiku = Some(id),
        "sonnet" => profile.claude_sonnet = Some(id),
        "opus" => profile.claude_opus = Some(id),
        "fable" => profile.claude_fable = Some(id),
        _ => {
            profile.default_model = if is_auto_model(&id) { None } else { Some(id) };
        }
    }
}

pub(crate) fn pick_claude_slot(profile: &Profile) -> Result<&'static str, String> {
    let items = vec![
        format!(
            "Default  ·  {}",
            session_model_label(profile.default_model())
        ),
        format!("Haiku    ·  {}", profile.claude_haiku()),
        format!("Sonnet   ·  {}", profile.claude_sonnet()),
        format!("Opus     ·  {}", profile.claude_opus()),
        format!("Fable    ·  {}", profile.claude_fable()),
    ];
    Ok(match term::pick("Which Claude model?", &items, Some(0))? {
        1 => "haiku",
        2 => "sonnet",
        3 => "opus",
        4 => "fable",
        _ => "default",
    })
}

pub(crate) fn slot_title(slot: &str) -> &'static str {
    match slot {
        "haiku" => "Haiku model",
        "sonnet" => "Sonnet model",
        "opus" => "Opus model",
        "fable" => "Fable model",
        _ => "Default model",
    }
}

pub(crate) fn slot_current<'a>(profile: &'a Profile, slot: &str) -> &'a str {
    match slot {
        "haiku" => profile.claude_haiku(),
        "sonnet" => profile.claude_sonnet(),
        "opus" => profile.claude_opus(),
        "fable" => profile.claude_fable(),
        _ => profile.default_model(),
    }
}

pub(crate) fn apply_claude_alias_flags(profile: &mut Profile, parsed: &ParsedArgs) -> bool {
    let mut changed = false;
    if let Some(v) = get_string_flag(&parsed.flags, "haiku") {
        profile.claude_haiku = Some(v);
        changed = true;
    }
    if let Some(v) = get_string_flag(&parsed.flags, "sonnet") {
        profile.claude_sonnet = Some(v);
        changed = true;
    }
    if let Some(v) = get_string_flag(&parsed.flags, "opus") {
        profile.claude_opus = Some(v);
        changed = true;
    }
    if let Some(v) = get_string_flag(&parsed.flags, "fable") {
        profile.claude_fable = Some(v);
        changed = true;
    }
    changed
}

pub(crate) fn known_model_id(models: &[CatalogModel], id: &str) -> bool {
    let id = catalog_model_id(id);
    is_auto_model(&id) || models.iter().any(|m| catalog_model_id(&m.id) == id)
}

pub(crate) fn save_model_slot(
    existing: Option<crate::config::Config>,
    path: &Path,
    slot: &str,
    id: &str,
) -> Result<i32, String> {
    let name = existing.as_ref().map(|c| c.active_profile.clone());
    save_profile_model_slot(existing, path, name.as_deref(), slot, id)
}

/// Save a model slot into the named profile (`None`: not signed in).
pub(crate) fn save_profile_model_slot(
    existing: Option<crate::config::Config>,
    path: &Path,
    profile: Option<&str>,
    slot: &str,
    id: &str,
) -> Result<i32, String> {
    let mut cfg = existing.ok_or_else(no_key_error)?;
    let name = profile.ok_or_else(no_key_error)?;
    let Some(p) = cfg.profiles.get_mut(name) else {
        return Err(no_key_error());
    };
    set_model_slot(p, slot, id.to_string());
    write_config(&cfg, path)?;
    let label = match slot {
        "haiku" => "haiku",
        "sonnet" => "sonnet",
        "opus" => "opus",
        "fable" => "fable",
        _ => "default model",
    };
    println!(
        "{}  {}  {}",
        term::ok("Saved"),
        label,
        term::model_id(&session_model_label(id))
    );
    Ok(0)
}

pub(crate) fn flag_agent(parsed: &ParsedArgs) -> Option<String> {
    get_string_flag(&parsed.flags, "agent").map(|s| canonical_tool(&s).to_string())
}

pub(crate) fn known_agent(name: &str) -> Result<String, String> {
    let id = canonical_tool(name);
    if KNOWN_AGENTS.iter().any(|(k, _)| *k == id) {
        Ok(id.to_string())
    } else {
        let known: Vec<&str> = KNOWN_AGENTS.iter().map(|(k, _)| *k).collect();
        Err(format!(
            "Unknown coding agent \"{name}\". Known: {}.",
            known.join(", ")
        ))
    }
}

pub(crate) fn save_agent_model(path: &Path, agent: &str, id: &str) -> Result<i32, String> {
    let agent = known_agent(agent)?;
    let mut cfg = load_config_if_present(path).ok_or_else(no_key_error)?;
    let id = catalog_model_id(id);
    let stored = if is_auto_model(&id) {
        None
    } else {
        Some(id.clone())
    };
    cfg.agent_binding_mut(&agent).default_model = stored;
    cfg.prune_empty_agents();
    write_config(&cfg, path)?;
    println!(
        "{}  {agent} model  {}",
        term::ok("Saved"),
        term::model_id(&session_model_label(&id))
    );
    Ok(0)
}

pub(crate) fn save_agent_account(path: &Path, agent: &str, profile: &str) -> Result<i32, String> {
    let agent = known_agent(agent)?;
    let mut cfg = load_config_if_present(path).ok_or_else(no_key_error)?;
    if !cfg.profiles.contains_key(profile) {
        return Err(format!("Account \"{profile}\" was not found."));
    }
    cfg.agent_binding_mut(&agent).profile = Some(profile.to_string());
    write_config(&cfg, path)?;
    println!(
        "{}  {agent} account  {}",
        term::ok("Switched"),
        term::accent(profile)
    );
    Ok(0)
}

pub(crate) fn clear_agent_account(path: &Path, agent: &str) -> Result<i32, String> {
    let agent = known_agent(agent)?;
    let mut cfg = load_config_if_present(path).ok_or_else(no_key_error)?;
    if let Some(binding) = cfg.agents.get_mut(&agent) {
        binding.profile = None;
    }
    cfg.prune_empty_agents();
    write_config(&cfg, path)?;
    println!(
        "{}  {agent} uses the active account  {}",
        term::ok("Cleared"),
        term::accent(&cfg.active_profile)
    );
    if cfg
        .agent_binding(&agent)
        .is_some_and(|b| b.api_key.as_deref().is_some_and(|k| !k.trim().is_empty()))
    {
        println!(
            "{}",
            term::dim(&crate::help::with_bin(&format!(
                "{agent} still has its own key, which wins. Remove it: {{bin}} keys unset --agent {agent}"
            )))
        );
    }
    Ok(0)
}

pub(crate) fn palette_bind_detail(agent: &str) -> String {
    format!("for {agent}")
}

pub(crate) fn routing_toggle_detail(on: bool, agent: &str) -> String {
    format!("{} · for {agent}", if on { "on" } else { "off" })
}

pub(crate) fn toggle_agent_routing_field(
    path: &std::path::Path,
    agent: &str,
    field: RoutingField,
) -> Result<i32, String> {
    let agent = known_agent(agent)?;
    let mut cfg = load_config_if_present(path).unwrap_or_default();
    let routing = &mut cfg.agent_binding_mut(&agent).routing;
    let on = match field {
        RoutingField::Exacto => {
            routing.set_exacto(!routing.wants_exacto());
            routing.wants_exacto()
        }
        RoutingField::Tools => {
            routing.set_require_tools(!routing.requires_tools());
            routing.requires_tools()
        }
        RoutingField::MinContext => {
            routing.set_require_1m(!routing.requires_1m_context());
            routing.requires_1m_context()
        }
    };
    cfg.prune_empty_agents();
    write_config(&cfg, path)?;
    println!(
        "{}  {agent} {} {}",
        term::ok("Saved"),
        field.label(),
        if on { "on" } else { "off" }
    );
    Ok(0)
}

/// Dispatch entry: usage errors exit 2, matching `anyr api`.
pub(crate) fn run_models_cli(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    crate::cmd::dispatch::usage_exit(run_models(parsed, env))
}

pub(crate) fn run_models(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let path = config_path(parsed, env);
    let existing = load_config_if_present(&path);
    let profile = existing
        .as_ref()
        .and_then(|c| c.profiles.get(&c.active_profile));
    let sub = parsed.passthrough.first().map(String::as_str);
    if let Some(verb) = sub.filter(|v| !matches!(*v, "list" | "ls" | "use" | "show")) {
        return Err(hint(&format!(
            "Unknown models command \"{verb}\". Use: {{bin}} models [list|ls|show <id>|use <id>]"
        )));
    }
    if sub == Some("show") {
        return crate::cmd::catalog::run_show(parsed, env);
    }
    if sub != Some("use") && crate::cmd::catalog::Filter::is_set(parsed) {
        return crate::cmd::catalog::run_list(parsed, env);
    }
    let agent = flag_agent(parsed);
    // Pinning a caller-supplied id to an agent does not need the catalog —
    // do not invent ids; the user (or TUI picker) already chose one.
    if let Some(agent) = agent.as_deref() {
        let positional = parsed.passthrough.get(1).cloned().filter(|s| !s.is_empty());
        if sub == Some("use") {
            if let Some(id) = positional {
                return save_agent_model(&path, agent, &id);
            }
        }
    }
    let key = resolve_api_key(&parsed.flags, env, profile);
    let base = resolve_base_url(&parsed.flags, profile);
    let flag_slots = [
        ("haiku", get_string_flag(&parsed.flags, "haiku")),
        ("sonnet", get_string_flag(&parsed.flags, "sonnet")),
        ("opus", get_string_flag(&parsed.flags, "opus")),
        ("fable", get_string_flag(&parsed.flags, "fable")),
    ];
    let has_alias_flags = flag_slots.iter().any(|(_, v)| v.is_some());
    if tui_wants_dump(parsed, env) {
        let models = models_for_picker(path.parent(), &base, key.as_deref(), env);
        let current = profile.map(|p| slot_current(p, "default").to_string());
        print!(
            "{}",
            render_model_picker_dump(&models, current.as_deref(), env)
        );
        return Ok(0);
    }
    // `anyrouter/auto` is a first-class preset — persist it without a catalog fetch.
    if sub == Some("use") && !has_alias_flags {
        let positional = parsed.passthrough.get(1).cloned().filter(|s| !s.is_empty());
        if let Some(id) = positional {
            if is_auto_model(&id) {
                return save_model_slot(existing, &path, "default", &display_model_id(&id));
            }
        }
    }
    if parsed.flag_true("pick") && term::is_interactive() {
        let models = models_for_picker(path.parent(), &base, key.as_deref(), env);
        if let Some(agent) = agent.as_deref() {
            let current = existing
                .as_ref()
                .and_then(|c| c.agent_binding(agent))
                .and_then(|b| b.default_model.clone())
                .or_else(|| profile.map(|p| p.default_model().to_string()));
            let id = pick_model(&models, current.as_deref(), &format!("{agent} model"))?;
            return save_agent_model(&path, agent, &id);
        }
        let slot = profile
            .map(pick_claude_slot)
            .transpose()?
            .unwrap_or("default");
        let current = profile.map(|p| slot_current(p, slot).to_string());
        let id = pick_model(&models, current.as_deref(), slot_title(slot))?;
        return save_model_slot(existing, &path, slot, &catalog_model_id(&id));
    }
    // `models use --agent` with no id: same picker as `--pick`, no catalog required.
    if sub == Some("use") && !has_alias_flags {
        if let Some(agent) = agent.as_deref() {
            if term::is_interactive() {
                let models = models_for_picker(path.parent(), &base, key.as_deref(), env);
                let current = existing
                    .as_ref()
                    .and_then(|c| c.agent_binding(agent))
                    .and_then(|b| b.default_model.clone())
                    .or_else(|| profile.map(|p| p.default_model().to_string()));
                let id = pick_model(&models, current.as_deref(), &format!("{agent} model"))?;
                return save_agent_model(&path, agent, &id);
            }
        }
    }
    let models = fetch_models(&base, key.as_deref())?;
    if sub == Some("use") || has_alias_flags {
        if agent.is_some() {
            return Err(hint(
                "Usage: {bin} models use <id> --agent <claude|codex|grok|opencode|pi|pool>",
            ));
        }
        let mut assigned = false;
        for (slot, value) in &flag_slots {
            let Some(id) = value.clone() else {
                continue;
            };
            if !known_model_id(&models, &id) {
                return Err(hint(&format!(
                    "Unknown model \"{id}\". Run: {{bin}} models"
                )));
            }
            save_model_slot(existing.clone(), &path, slot, &catalog_model_id(&id))?;
            assigned = true;
        }
        let positional = parsed.passthrough.get(1).cloned().filter(|s| !s.is_empty());
        if let Some(id) = positional {
            if !known_model_id(&models, &id) {
                return Err(hint(&format!(
                    "Unknown model \"{id}\". Run: {{bin}} models"
                )));
            }
            save_model_slot(
                load_config_if_present(&path),
                &path,
                "default",
                &catalog_model_id(&id),
            )?;
            assigned = true;
        }
        if assigned {
            return Ok(0);
        }
        let id = if term::is_interactive() {
            let slot = profile
                .map(pick_claude_slot)
                .transpose()?
                .unwrap_or("default");
            let current = profile.map(|p| slot_current(p, slot).to_string());
            let catalog = PickerCatalog::new(path.parent(), &base, models.clone(), env);
            pick_model(&catalog, current.as_deref(), slot_title(slot)).map(|id| (slot, id))
        } else {
            Err(hint(
                "Usage: {bin} models use <id>   or   {bin} models use --haiku|--sonnet|--opus <id>",
            ))
        }?;
        if !known_model_id(&models, &id.1) {
            return Err(hint(&format!(
                "Unknown model \"{}\". Run: {{bin}} models",
                id.1
            )));
        }
        return save_model_slot(
            load_config_if_present(&path),
            &path,
            id.0,
            &catalog_model_id(&id.1),
        );
    }
    let pinned = profile
        .map(|p| display_model_id(p.default_model()))
        .into_iter()
        .collect::<Vec<_>>();
    let preset = profile.map(|p| p.pinned_preset().to_string());
    // Piped: bare TSV rows like `anyr api`, no header or footer hints.
    if !parsed.flag_true("json") && !std::io::stdout().is_terminal() {
        print!("{}", crate::http::format_models_tsv(&models, &pinned));
        return Ok(0);
    }
    let (stdout, _) = format_models_list(
        &models,
        &pinned,
        preset.as_deref(),
        parsed.flag_true("json"),
    );
    print!("{stdout}");
    if !parsed.flag_true("json") {
        let bin = crate::help::invoked_bin();
        if let Some(id) = pinned.first() {
            println!("{}  {id}", term::dim(&format!("$ {bin} models use")));
        }
        if let Some(key) = key.as_deref() {
            if let Ok(credits) = fetch_credits(&base, key) {
                print!("{}", format_usage_compact(&credits, &bin));
            }
        }
    }
    Ok(0)
}

pub(crate) fn format_usage_compact(credits: &serde_json::Value, bin: &str) -> String {
    let remaining = credits
        .get("balance")
        .and_then(|v| v.as_f64())
        .map(crate::http::format_usd)
        .unwrap_or_else(|| "-".into());
    let day = credits
        .pointer("/last_24h")
        .or_else(|| credits.pointer("/usage/day"))
        .or_else(|| credits.get("today"))
        .and_then(|v| v.as_f64())
        .map(crate::http::format_usd);
    let top = credits
        .pointer("/most_used_model")
        .or_else(|| credits.pointer("/top_model"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mut line = format!(
        "  {}    remaining  {}",
        term::dim(&format!("$ {bin} usage")),
        term::ok(&remaining)
    );
    if let Some(day) = day {
        line.push_str(&format!("    24h  {day}"));
    }
    if let Some(top) = top {
        line.push_str(&format!("    top  {}", term::model_id(&top)));
    }
    format!("{line}\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoutingField {
    Exacto,
    Tools,
    MinContext,
}

impl RoutingField {
    pub(crate) fn label(self) -> &'static str {
        match self {
            RoutingField::Exacto => "exacto",
            RoutingField::Tools => "tools",
            RoutingField::MinContext => "1M ctx",
        }
    }

    #[allow(dead_code)]
    pub(crate) fn action_kind(self) -> &'static str {
        match self {
            RoutingField::Exacto => "exacto",
            RoutingField::Tools => "tools",
            RoutingField::MinContext => "1m",
        }
    }
}

#[cfg(test)]
mod picker_catalog_tests {
    use super::*;
    use crate::http::CatalogModel;

    #[test]
    fn the_picker_does_not_offer_a_model_the_caller_cannot_route() {
        let blocked = CatalogModel {
            routable: Some(false),
            ..cm("x/blocked", None)
        };
        let env = BTreeMap::from([("ANYR_NO_CATALOG".to_string(), "1".to_string())]);
        let picker = PickerCatalog::new(
            None,
            "https://gw.test/api",
            vec![blocked, cm("x/open", None)],
            &env,
        );
        assert_eq!(
            picker
                .models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["x/open"]
        );
    }

    fn cm(id: &str, context_length: Option<i64>) -> CatalogModel {
        CatalogModel {
            id: id.into(),
            name: None,
            owned_by: None,
            context_length,
            max_completion_tokens: None,
            ..Default::default()
        }
    }

    #[test]
    fn pick_ids_pins_anyrouter_auto_and_does_not_dump_usage_order() {
        // Input order is a usage ranking (most-used first).
        let models = vec![
            cm("stealth/ox-alpha", Some(1_000_000)),
            cm("openai/gpt-5.4-mini", Some(128_000)),
            cm("anthropic/claude-sonnet-4.6", Some(200_000)),
            cm("auto", None),
        ];
        let ids = pick_ids(&models);
        assert_eq!(ids[0], "anyrouter/auto");
        assert_eq!(
            ids,
            vec![
                "anyrouter/auto",
                "anthropic/claude-sonnet-4.6",
                "openai/gpt-5.4-mini",
                "stealth/ox-alpha",
            ]
        );
        assert_ne!(ids[1], "stealth/ox-alpha", "must not lead with most-used");
    }

    #[test]
    fn pick_ids_empty_catalog_still_has_preset() {
        assert_eq!(pick_ids(&[]), vec!["anyrouter/auto"]);
    }

    const KEY: &str = "sk-ar-v1-pickertest";

    fn cache_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("anyr-picker-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn offline_models(_: &str, _: Option<&str>) -> Result<Vec<CatalogModel>, String> {
        panic!("a cached catalog must not touch the network")
    }

    fn one_score(_: &str) -> Result<BTreeMap<String, f64>, String> {
        Ok([("cached/m".to_string(), 42.0)].into())
    }

    fn offline_scores(_: &str) -> Result<BTreeMap<String, f64>, String> {
        panic!("cached scores must not touch the network")
    }

    #[test]
    fn a_warm_picker_reads_catalog_and_scores_from_disk_only() {
        let d = cache_dir("offline");
        let base = "https://picker-offline.test/api";
        crate::launch_cache::CatalogCache::new(&d, base, Some(KEY))
            .save(&[cm("cached/m", Some(8192))], now_secs());
        let first = PickerCatalog::load(
            Some(&d),
            base,
            Some(KEY),
            now_secs(),
            &offline_models,
            &one_score,
        );
        let again = PickerCatalog::load(
            Some(&d),
            base,
            Some(KEY),
            now_secs(),
            &offline_models,
            &offline_scores,
        );
        for catalog in [first, again] {
            assert_eq!(
                pick_ids(&catalog.models),
                vec!["anyrouter/auto", "cached/m"]
            );
            assert_eq!(catalog.smart.get("cached/m"), Some(&42.0));
        }
    }

    #[test]
    fn the_launcher_pickers_read_the_disk_cache() {
        let d = cache_dir("launcher");
        // Nothing listens here, so a live fetch would come back empty.
        let base = "http://127.0.0.1:1/api";
        crate::launch_cache::CatalogCache::new(&d, base, Some(KEY))
            .save(&[cm("cached/m", Some(8192))], now_secs());
        let catalog = models_for_picker(Some(&d), base, Some(KEY), &BTreeMap::new());
        assert_eq!(
            pick_ids(&catalog.models),
            vec!["anyrouter/auto", "cached/m"]
        );
    }
}
