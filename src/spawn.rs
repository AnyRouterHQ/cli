use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
#[cfg(feature = "native")]
use std::process::{Command, Stdio};

use crate::config::{
    Profile, YamlValue, DEFAULT_BASE_URL, DEFAULT_MODEL, DEFAULT_PRESET, DEFAULT_TIMEOUT_MS,
};

pub const PI_DEFAULT_MODEL: &str = "anyrouter/free";
/// Historical 1M-context suffix used by Claude Code. Claude Code used to strip
/// it before the gateway, but third-party catalog ids (e.g.
/// `meituan/longcat-2.0`) are forwarded verbatim to the provider and 404 on
/// `id[1m]`. The CLI no longer appends it; we keep stripping it on ingest as
/// defense against a user-typed or relay-tagged id.
pub const CLAUDE_1M_SUFFIX: &str = "[1m]";
const MIN_1M_CONTEXT: i64 = 1_000_000;

pub const REASONING_LEVELS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max"];
const CLAUDE_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const CLAUDE_EFFORT_TOKENS: &[(&str, i64)] = &[
    ("low", 2048),
    ("medium", 8192),
    ("high", 24000),
    ("xhigh", 32000),
    ("max", 32000),
];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolConfig {
    pub command: String,
    pub base_url_env: String,
    pub auth_env: String,
    pub model_env: Option<String>,
    pub base_suffix: String,
    pub enable_gateway_model_discovery: bool,
    pub shadow_env: Option<String>,
    pub extra: BTreeMap<String, YamlValue>,
}

impl ToolConfig {
    pub fn merge(&mut self, over: &ToolConfig) {
        if !over.command.is_empty() {
            self.command = over.command.clone();
        }
        if !over.base_url_env.is_empty() {
            self.base_url_env = over.base_url_env.clone();
        }
        if !over.auth_env.is_empty() {
            self.auth_env = over.auth_env.clone();
        }
        if over.model_env.is_some() {
            self.model_env = over.model_env.clone();
        }
        if !over.base_suffix.is_empty() {
            self.base_suffix = over.base_suffix.clone();
        }
        // Overlay from `from_yaml` defaults discovery to false when the key
        // is missing — never copy that over a builtin. `apply_yaml` is the
        // path that honors an explicit false.
        if over.enable_gateway_model_discovery {
            self.enable_gateway_model_discovery = true;
        }
        if over.shadow_env.is_some() {
            self.shadow_env = over.shadow_env.clone();
        }
        for (k, v) in &over.extra {
            self.extra.insert(k.clone(), v.clone());
        }
    }

    /// Apply only keys present in `map`. Missing keys keep the current value
    /// so a partial `tools.claude.command:` overlay cannot wipe `/v1` or
    /// gateway discovery.
    pub fn apply_yaml(&mut self, map: &BTreeMap<String, YamlValue>) {
        for (key, value) in map {
            match key.as_str() {
                "command" => self.command = value.as_string_lossy(),
                "base_url_env" => self.base_url_env = value.as_string_lossy(),
                "auth_env" => self.auth_env = value.as_string_lossy(),
                "model_env" => {
                    let s = value.as_string_lossy();
                    self.model_env = if s.is_empty() || s == "null" {
                        None
                    } else {
                        Some(s)
                    };
                }
                "base_suffix" => self.base_suffix = value.as_string_lossy(),
                "enable_gateway_model_discovery" => {
                    self.enable_gateway_model_discovery =
                        matches!(value, YamlValue::Bool(true)) || value.as_string_lossy() == "true"
                }
                "shadow_env" => {
                    let s = value.as_string_lossy();
                    self.shadow_env = if s.is_empty() || s == "null" {
                        None
                    } else {
                        Some(s)
                    };
                }
                _ => {
                    self.extra.insert(key.clone(), value.clone());
                }
            }
        }
    }

    pub fn from_yaml(map: &BTreeMap<String, YamlValue>) -> Self {
        let mut tool = ToolConfig::default();
        tool.apply_yaml(map);
        tool
    }

    pub fn extra_flag(&self, key: &str) -> bool {
        match self.extra.get(key) {
            Some(YamlValue::Bool(true)) => true,
            Some(YamlValue::Int(n)) => *n != 0,
            Some(YamlValue::String(s)) => {
                let t = s.trim();
                t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes")
            }
            _ => false,
        }
    }

    pub fn to_yaml_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("    command: {}", self.command),
            format!("    base_url_env: {}", self.base_url_env),
            format!("    auth_env: {}", self.auth_env),
            format!(
                "    model_env: {}",
                self.model_env.as_deref().unwrap_or("null")
            ),
            format!(
                "    base_suffix: {}",
                if self.base_suffix.is_empty() {
                    "\"\"".into()
                } else {
                    self.base_suffix.clone()
                }
            ),
            format!(
                "    enable_gateway_model_discovery: {}",
                self.enable_gateway_model_discovery
            ),
            format!(
                "    shadow_env: {}",
                self.shadow_env.as_deref().unwrap_or("null")
            ),
        ];
        for (key, value) in &self.extra {
            lines.push(format!(
                "    {key}: {}",
                crate::config::yaml_scalar_value(value)
            ));
        }
        lines
    }
}

fn builtin(id: &str) -> Option<ToolConfig> {
    Some(match id {
        "claude" => ToolConfig {
            command: "claude".into(),
            base_url_env: "ANTHROPIC_BASE_URL".into(),
            auth_env: "ANTHROPIC_AUTH_TOKEN".into(),
            model_env: Some("ANTHROPIC_MODEL".into()),
            base_suffix: String::new(),
            enable_gateway_model_discovery: true,
            shadow_env: Some("ANTHROPIC_API_KEY".into()),
            extra: BTreeMap::new(),
        },
        "codex" => ToolConfig {
            command: "codex".into(),
            base_url_env: "OPENAI_BASE_URL".into(),
            auth_env: "OPENAI_API_KEY".into(),
            model_env: Some("OPENAI_MODEL".into()),
            base_suffix: "/v1".into(),
            enable_gateway_model_discovery: false,
            shadow_env: Some("OPENAI_API_KEY".into()),
            extra: BTreeMap::new(),
        },
        "grok" => ToolConfig {
            command: "grok".into(),
            base_url_env: "GROK_MODELS_BASE_URL".into(),
            auth_env: "GROK_CODE_XAI_API_KEY".into(),
            model_env: None,
            base_suffix: "/v1".into(),
            enable_gateway_model_discovery: false,
            shadow_env: None,
            extra: BTreeMap::new(),
        },
        "opencode" => ToolConfig {
            command: "opencode".into(),
            base_url_env: "OPENAI_BASE_URL".into(),
            auth_env: "OPENAI_API_KEY".into(),
            model_env: Some("OPENAI_MODEL".into()),
            base_suffix: "/v1".into(),
            enable_gateway_model_discovery: false,
            shadow_env: None,
            extra: BTreeMap::new(),
        },
        "pool" => ToolConfig {
            command: "pool".into(),
            base_url_env: "POOLSIDE_STANDALONE_BASE_URL".into(),
            auth_env: "POOLSIDE_API_KEY".into(),
            model_env: Some("POOLSIDE_STANDALONE_MODEL".into()),
            base_suffix: "/v1".into(),
            enable_gateway_model_discovery: false,
            shadow_env: Some("OPENAI_API_KEY".into()),
            extra: BTreeMap::new(),
        },
        "pi" => ToolConfig {
            command: "pi".into(),
            base_url_env: "OPENAI_BASE_URL".into(),
            auth_env: "ANYROUTER_API_KEY".into(),
            model_env: None,
            base_suffix: "/v1".into(),
            enable_gateway_model_discovery: false,
            shadow_env: None,
            extra: BTreeMap::new(),
        },
        // Qwen Code reads OPENAI_* ahead of its own settings.json.
        "qwen" => ToolConfig {
            command: "qwen".into(),
            base_url_env: "OPENAI_BASE_URL".into(),
            auth_env: "OPENAI_API_KEY".into(),
            model_env: Some("OPENAI_MODEL".into()),
            base_suffix: "/v1".into(),
            ..ToolConfig::default()
        },
        // Goose splits the endpoint into OPENAI_HOST + OPENAI_BASE_PATH; see
        // `build_tool_env`. Environment beats its config.yaml.
        "goose" => ToolConfig {
            command: "goose".into(),
            base_url_env: "OPENAI_HOST".into(),
            auth_env: "OPENAI_API_KEY".into(),
            model_env: Some("GOOSE_MODEL".into()),
            ..ToolConfig::default()
        },
        // AIDER_* outranks `.aider.conf.yml`; plain OPENAI_* does not.
        "aider" => ToolConfig {
            command: "aider".into(),
            base_url_env: "AIDER_OPENAI_API_BASE".into(),
            auth_env: "AIDER_OPENAI_API_KEY".into(),
            model_env: Some("AIDER_MODEL".into()),
            base_suffix: "/v1".into(),
            ..ToolConfig::default()
        },
        // Crush gets a private CRUSH_GLOBAL_CONFIG (`prepare_crush_config`)
        // whose provider reads `$ANYROUTER_API_KEY`.
        "crush" => ToolConfig {
            command: "crush".into(),
            base_url_env: "ANYROUTER_BASE_URL".into(),
            auth_env: "ANYROUTER_API_KEY".into(),
            base_suffix: "/v1".into(),
            ..ToolConfig::default()
        },
        // fx only takes a custom endpoint as a named connection in
        // ~/.fx/settings.json (`prepare_fx_settings`); FX_PROVIDER selects it
        // for this process and the connection reads `$ANYROUTER_API_KEY`.
        "fx" => ToolConfig {
            command: "fx".into(),
            base_url_env: "ANYROUTER_BASE_URL".into(),
            auth_env: "ANYROUTER_API_KEY".into(),
            model_env: Some("FX_MODEL".into()),
            base_suffix: "/v1".into(),
            ..ToolConfig::default()
        },
        // Kilo is an OpenCode fork: same inline config, its own variable.
        "kilo" => ToolConfig {
            command: "kilo".into(),
            base_url_env: "OPENAI_BASE_URL".into(),
            auth_env: "ANYROUTER_API_KEY".into(),
            model_env: Some("OPENAI_MODEL".into()),
            base_suffix: "/v1".into(),
            ..ToolConfig::default()
        },
        _ => return None,
    })
}

pub fn create_default_tools() -> BTreeMap<String, ToolConfig> {
    crate::sort::map(
        crate::registry::LAUNCHERS
            .iter()
            .filter_map(|id| builtin(id).map(|t| (id.to_string(), t))),
    )
}

pub fn canonical_tool(name: &str) -> &str {
    match name {
        "cc" => "claude",
        "poolside" => "pool",
        "status" => "whoami",
        other => other,
    }
}

pub fn resolve_tool(
    config: Option<&crate::config::Config>,
    name: &str,
) -> Result<ToolConfig, String> {
    let id = canonical_tool(name);
    let fallback = builtin(id).ok_or_else(|| {
        format!(
            "Unknown tool \"{name}\". Known tools: {}.",
            crate::registry::LAUNCHERS.join(", ")
        )
    })?;
    if let Some(over) = config.and_then(|c| c.tools.get(id)) {
        // Parsed tools are already builtin + apply_yaml. Clone, don't merge a
        // second time (merge would treat missing overlay keys as defaults).
        return Ok(over.clone());
    }
    Ok(fallback)
}

pub fn tool_base_url(profile: &Profile, tool: &ToolConfig) -> String {
    format!(
        "{}{}",
        profile.base_url().trim_end_matches('/'),
        tool.base_suffix
    )
}

pub fn default_profile_for_env(base_url: Option<&str>, api_key: Option<&str>) -> Profile {
    Profile {
        api_key: api_key.map(str::to_string),
        base_url: Some(base_url.unwrap_or(DEFAULT_BASE_URL).to_string()),
        pinned_preset: Some(DEFAULT_PRESET.into()),
        default_model: Some(DEFAULT_MODEL.into()),
        timeout_ms: Some(DEFAULT_TIMEOUT_MS),
        ..Profile::default()
    }
}

pub(crate) fn clamp_level(levels: &[&str], level: &str) -> Option<String> {
    if levels.contains(&level) {
        return Some(level.to_string());
    }
    let wanted = REASONING_LEVELS.iter().position(|l| *l == level);
    let mut best = levels.first().copied().unwrap_or(level);
    if let Some(wanted) = wanted {
        for candidate in levels {
            if let Some(rank) = REASONING_LEVELS.iter().position(|l| l == candidate) {
                if rank <= wanted {
                    best = candidate;
                }
            }
        }
    }
    Some(best.to_string())
}

pub fn normalize_effort(input: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = input else {
        return Ok(None);
    };
    let value = raw.trim().to_ascii_lowercase();
    if !REASONING_LEVELS.contains(&value.as_str()) {
        return Err(format!(
            "Invalid --effort \"{raw}\". Expected one of: {}.",
            REASONING_LEVELS.join(", ")
        ));
    }
    Ok(Some(value))
}

fn harness_effort(tool: &str, effort: Option<&str>) -> Option<String> {
    let effort = effort?;
    match tool {
        "claude" => clamp_level(CLAUDE_LEVELS, effort),
        _ => Some(effort.to_string()),
    }
}

pub struct BuildToolEnvInput<'a> {
    pub tool_name: &'a str,
    pub tool: &'a ToolConfig,
    pub profile: &'a Profile,
    pub api_key: &'a str,
    pub model: &'a str,
    pub effort: Option<&'a str>,
    pub context_window: Option<i64>,
    /// Peeled `[1m]`/`[500k]` or `routing.min_context`. Not catalog `context_length`.
    pub min_context: Option<i64>,
    pub model_map: Option<&'a HashMap<String, String>>,
}

pub fn build_tool_env(input: BuildToolEnvInput<'_>) -> BTreeMap<String, String> {
    let model_mode = if is_auto_model(input.model) {
        "auto"
    } else {
        "concrete"
    };
    let mut env = BTreeMap::new();
    env.insert(
        input.tool.base_url_env.clone(),
        tool_base_url(input.profile, input.tool),
    );
    env.insert(input.tool.auth_env.clone(), input.api_key.to_string());
    // Parent-shell Anthropic/OpenAI keys must not beat the AnyRouter token.
    if let Some(shadow) = &input.tool.shadow_env {
        env.insert(shadow.clone(), input.api_key.to_string());
    }
    env.insert(
        "ANYROUTER_PINNED_PRESET".into(),
        input.profile.pinned_preset().to_string(),
    );
    env.insert("ANYROUTER_MODEL_MODE".into(), model_mode.into());
    env.insert(
        "API_TIMEOUT_MS".into(),
        input.profile.timeout_ms().to_string(),
    );
    if let Some(model_env) = &input.tool.model_env {
        env.insert(
            model_env.clone(),
            model_id_for_tool(input.tool_name, input.model, input.min_context),
        );
    }
    if let Some(effort) = input.effort {
        env.insert("ANYROUTER_EFFORT".into(), effort.to_string());
    }
    if input.tool_name == "pi" {
        // Placeholder until `prepare_pi_wrapper` overwrites with the full catalog.
        let base = tool_base_url(input.profile, input.tool);
        let model_id = pi_resolved_model(input.model);
        let entries = pi_catalog_models(&model_id, &[]);
        if let Ok(api_key) = pi_api_key_ref(&input.tool.auth_env) {
            env.insert(
                "PI_MODELS_JSON".into(),
                serde_json::to_string(&pi_models_config(&base, &api_key, &entries))
                    .unwrap_or_else(|_| "{}".into()),
            );
        }
    }
    if let Some(content_env) = inline_config_env(input.tool_name) {
        env.remove(&input.tool.base_url_env);
        if let Some(m) = &input.tool.model_env {
            env.remove(m);
        }
        let mut provider = serde_json::json!({
            "npm": "@ai-sdk/openai-compatible",
            "name": "AnyRouter",
            "options": {
                "baseURL": tool_base_url(input.profile, input.tool),
                "apiKey": format!("{{env:{}}}", input.tool.auth_env),
            },
            "models": {}
        });
        let mut config = serde_json::json!({
            "provider": { "anyrouter": provider.clone() }
        });
        if input.tool_name == "opencode" {
            config["$schema"] = "https://opencode.ai/config.json".into();
        }
        // Auto still names a gateway model: without a top-level model OpenCode
        // stays on its own default and the empty allowlist hides AnyRouter.
        let catalog = display_model_id(input.model);
        provider["models"][&catalog] = serde_json::json!({ "name": catalog });
        config["provider"]["anyrouter"] = provider;
        config["model"] = serde_json::json!(format!("anyrouter/{catalog}"));
        env.insert(
            content_env.into(),
            serde_json::to_string(&config).unwrap_or_else(|_| "{}".into()),
        );
    }
    if input.tool_name == "goose" {
        // Goose wants the origin in OPENAI_HOST and the rest of the path,
        // without a leading slash, in OPENAI_BASE_PATH.
        let base = format!(
            "{}/v1/chat/completions",
            tool_base_url(input.profile, input.tool)
        );
        let (host, path) = split_origin(&base);
        env.insert("OPENAI_HOST".into(), host.into());
        env.insert(
            "OPENAI_BASE_PATH".into(),
            path.trim_start_matches('/').into(),
        );
        env.insert("GOOSE_PROVIDER".into(), "openai".into());
    }
    if input.tool_name == "fx" {
        env.insert("FX_PROVIDER".into(), FX_CONNECTION.into());
    }
    if input.tool_name == "aider" {
        // Aider routes through LiteLLM, which needs the `openai/` prefix to
        // send an unknown id to the OpenAI-compatible base.
        let id = model_id_for_tool("aider", input.model, input.min_context);
        env.insert("AIDER_MODEL".into(), format!("openai/{id}"));
        // An id missing from LiteLLM's metadata otherwise warns on every start.
        env.insert("AIDER_SHOW_MODEL_WARNINGS".into(), "false".into());
    }
    if input.tool_name == "claude" {
        let anthropic_model = model_id_for_tool("claude", input.model, input.min_context);
        env.insert("ANTHROPIC_MODEL".into(), anthropic_model);
        // Claude Code reads custom headers from ANTHROPIC_CUSTOM_HEADERS
        // (newline-separated "Name: value"). Source is the canonical
        // claude-code app; the title carries the via-anyr-CLI marker so
        // /data/apps counts Claude Code and anyr CLI (#275).
        env.insert(
            "ANTHROPIC_CUSTOM_HEADERS".into(),
            "X-AnyRouter-Source: claude-code\nX-AnyRouter-Title: Claude Code via anyr CLI".into(),
        );
        env.insert(
            "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY".into(),
            // Discovery remaps unknown ids (including virtual `anyrouter/*`)
            // onto a catalog SKU such as Laguna. Keep it off for presets so the
            // gateway still sees the virtual id + extra-body min_context.
            if claude_gateway_discovery_enabled(input.tool, input.model) {
                "1"
            } else {
                "0"
            }
            .into(),
        );
        // A concrete pinned model takes over every Claude Code alias slot
        // (haiku / sonnet / opus / fable and subagents) so nothing — including
        // automatic model fallback, which rides the fable alias on third-party
        // providers — falls back to a different model. Slots set explicitly
        // (--haiku/--sonnet/--opus/--fable or the profile config) still win.
        let pinned = (!is_virtual_preset(input.model)).then_some(input.model);
        let alias = |slot: &Option<String>, default: &str| -> String {
            let explicit = slot.as_deref().map(str::trim).filter(|s| !s.is_empty());
            match (pinned, explicit) {
                (Some(id), None) => model_id_for_tool("claude", id, input.min_context),
                (None, Some(id)) => model_id_for_tool("claude", id, None),
                (Some(_), Some(id)) => model_id_for_tool("claude", id, None),
                (None, None) => default.to_string(),
            }
        };
        let haiku = alias(&input.profile.claude_haiku, input.profile.claude_haiku());
        env.insert("ANTHROPIC_DEFAULT_HAIKU_MODEL".into(), haiku.clone());
        env.insert(
            "ANTHROPIC_DEFAULT_SONNET_MODEL".into(),
            alias(&input.profile.claude_sonnet, input.profile.claude_sonnet()),
        );
        env.insert(
            "ANTHROPIC_DEFAULT_OPUS_MODEL".into(),
            alias(&input.profile.claude_opus, input.profile.claude_opus()),
        );
        env.insert(
            "ANTHROPIC_DEFAULT_FABLE_MODEL".into(),
            alias(&input.profile.claude_fable, input.profile.claude_fable()),
        );
        env.insert("CLAUDE_CODE_SUBAGENT_MODEL".into(), haiku);
        if claude_wants_auto_compact(input.model, input.min_context, input.context_window) {
            env.insert("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), "1000000".into());
        }
        // Claude Code sizes a session from its own model table, which only knows
        // Anthropic ids, so a third-party id lands on the 200k default. Declare
        // the window the catalog resolved. A virtual preset picks its hop per
        // request and carries its own `[1m]` suffix, so it keeps its own window.
        if !is_virtual_preset(input.model) {
            if let Some(window) = input.context_window.filter(|n| *n > 0) {
                env.insert("CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(), window.to_string());
            }
        }
        // Label each picker entry with its role; otherwise four identical IDs
        // all render as "Custom <Alias> model".
        for (key, value) in [
            (
                "ANTHROPIC_DEFAULT_HAIKU_MODEL_DESCRIPTION",
                "Background & subagents",
            ),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL_DESCRIPTION", "Sonnet alias"),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL_DESCRIPTION", "Opus alias"),
            (
                "ANTHROPIC_DEFAULT_FABLE_MODEL_DESCRIPTION",
                "Fable alias + fallback",
            ),
        ] {
            env.insert(key.into(), value.into());
        }
        if let Some(effort) = harness_effort("claude", input.effort) {
            if let Some((_, tokens)) = CLAUDE_EFFORT_TOKENS.iter().find(|(k, _)| *k == effort) {
                env.insert("MAX_THINKING_TOKENS".into(), tokens.to_string());
            }
        }
    }
    let _ = input.model_map;
    env
}

/// Merge AnyRouter preset routing fields into the child env so launch
/// actually sends them. Claude Code reads `CLAUDE_CODE_EXTRA_BODY`.
pub fn apply_routing_env(
    env: &mut BTreeMap<String, String>,
    routing: &crate::config::RoutingConstraints,
    tool_name: &str,
) {
    let Some(body) = routing.extra_body_json() else {
        return;
    };
    env.insert("ANYROUTER_EXTRA_BODY".into(), body.clone());
    if tool_name == "claude" {
        env.insert("CLAUDE_CODE_EXTRA_BODY".into(), body);
    }
}

/// First-party virtual presets (`anyrouter/auto`, `anyrouter/free`, …).
/// Keep in lockstep with gateway `ANYROUTER_VIRTUAL_MODELS`.
const VIRTUAL_PRESETS: &[&str] = &[
    "anyrouter/auto",
    "anyrouter/free",
    "anyrouter/byok",
    "anyrouter/coding",
    "anyrouter/agent",
    "anyrouter/hermes",
    "anyrouter/cowork",
    "anyrouter/latest",
];

pub fn is_auto_model(model: &str) -> bool {
    let value = catalog_model_id(model);
    let value = crate::config::strip_context_window_suffix(&value);
    value.is_empty() || value == "auto" || value == "anyrouter/auto"
}

/// A saved preset used as a model id (`@preset/<slug>`). The gateway picks the
/// hop per request, so there is no catalog entry, window or discovery remap.
pub fn is_user_preset(model: &str) -> bool {
    catalog_model_id(model).starts_with("@preset/")
}

/// Virtual routing presets. `[1m]` / `[500k]` are min_context floors, not listing ids.
pub fn is_virtual_preset(model: &str) -> bool {
    if is_auto_model(model) {
        return true;
    }
    let id = catalog_model_id(model);
    let id = crate::config::strip_context_window_suffix(&id);
    VIRTUAL_PRESETS.contains(&id)
}

/// Catalog id for display and config. Auto is `anyrouter/auto`.
pub fn display_model_id(model: &str) -> String {
    let id = catalog_model_id(model);
    if is_auto_model(&id) {
        "anyrouter/auto".into()
    } else {
        id
    }
}

/// Launcher / settings label. Auto is the selectable preset `anyrouter/auto`.
pub fn session_model_label(model: &str) -> String {
    display_model_id(model)
}

pub fn claude_wants_1m(context_window: Option<i64>) -> bool {
    match context_window {
        Some(n) => n >= MIN_1M_CONTEXT,
        // Unknown concrete window: still enable compact so a 1M session is not
        // truncated at Claude's 200k default. Virtual presets use the floor.
        None => true,
    }
}

fn merged_floor(model: &str, min_context: Option<i64>) -> Option<i64> {
    match (peel_context_window_suffixes(model).1, min_context) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Compact when the routing floor is ≥ 1M. Concrete ids without a floor still
/// use catalog `context_length` (unknown counts as yes).
pub fn claude_wants_auto_compact(
    model: &str,
    min_context: Option<i64>,
    context_window: Option<i64>,
) -> bool {
    if merged_floor(model, min_context).is_some_and(|n| n >= MIN_1M_CONTEXT) {
        return true;
    }
    if is_virtual_preset(model) || is_user_preset(model) {
        return false;
    }
    claude_wants_1m(context_window)
}

pub fn claude_gateway_discovery_enabled(tool: &ToolConfig, model: &str) -> bool {
    tool.enable_gateway_model_discovery && !is_virtual_preset(model) && !is_user_preset(model)
}

/// Catalog `context_length` for a concrete id. Virtual `anyrouter/*` must not
/// inherit auto's 200k listing — that would paint the HUD `[200k]`.
pub fn catalog_context_window(
    requested: &str,
    models: &[crate::http::CatalogModel],
) -> Option<i64> {
    let id = catalog_model_id(requested);
    if is_virtual_preset(&id) {
        return None;
    }
    models
        .iter()
        .find(|m| catalog_model_id(&m.id) == id)
        .and_then(|m| m.context_length)
}

/// Spell a token floor as `[1m]` / `[500k]` (same as `--model` suffixes).
pub fn context_floor_suffix(n: i64) -> Option<String> {
    if n <= 0 {
        return None;
    }
    if n % 1_000_000 == 0 {
        return Some(format!("[{}m]", n / 1_000_000));
    }
    if n % 1_000 == 0 {
        return Some(format!("[{}k]", n / 1_000));
    }
    None
}

/// Peel `[Nm]`/`[Nk]` → catalog id. Claude virtual presets re-attach the floor
/// so the HUD shows `[1m]`/`[500k]` instead of catalog 200k. Concrete ids never
/// get an invented suffix (those SKUs 404).
pub fn model_id_for_tool(tool_name: &str, model: &str, min_context: Option<i64>) -> String {
    let (peeled, peeled_floor) = peel_context_window_suffixes(model);
    let id = catalog_model_id(&peeled);
    let catalog = if is_auto_model(&id) {
        display_model_id(&id)
    } else {
        id
    };
    if tool_name == "claude" && is_virtual_preset(&catalog) {
        if let Some(n) = match (peeled_floor, min_context) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        } {
            if let Some(sfx) = context_floor_suffix(n) {
                return format!("{catalog}{sfx}");
            }
        }
    }
    catalog
}

/// Strip CSI, Claude's `[1m]` 1M suffix, closed `[Nm]` / `[0;1m]` tails, and
/// dangling SGR (`[1m` without `]`). Store/send the catalog id only.
pub fn sanitize_model_id(model: &str) -> String {
    catalog_model_id(model)
}

/// Parse trailing `[<n>k|m]` as a token floor (`k` = thousand, `m` = million).
/// Same spelling as `anyrouter/auto[1m]` / `[500k]`. Does not strip CSI.
pub fn peel_context_window_suffixes(model: &str) -> (String, Option<i64>) {
    let mut s = model.trim().to_string();
    let mut min_context: Option<i64> = None;
    while let Some(open) = s.rfind('[') {
        if !s.ends_with(']') || open + 2 >= s.len() {
            break;
        }
        let inner = &s[open + 1..s.len() - 1];
        let Some(unit) = inner.chars().last() else {
            break;
        };
        let digits = &inner[..inner.len() - 1];
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            break;
        }
        let n: i64 = match digits.parse() {
            Ok(n) if n > 0 => n,
            _ => break,
        };
        let floor = match unit {
            'm' | 'M' => n.saturating_mul(1_000_000),
            'k' | 'K' => n.saturating_mul(1_000),
            _ => break,
        };
        min_context = Some(match min_context {
            Some(existing) => existing.max(floor),
            None => floor,
        });
        s.truncate(open);
    }
    (s, min_context)
}

/// Peel `[1m]` / `[500k]` (and `:exacto`) into routing prefs; return catalog id.
pub fn apply_model_id_routing(
    model: &str,
    routing: &mut crate::config::RoutingConstraints,
) -> String {
    let (mut peeled, floor) = peel_context_window_suffixes(model);
    if let Some(n) = floor {
        routing.merge_min_context(n);
    }
    if let Some((base, suffix)) = peeled.rsplit_once(':') {
        if suffix.eq_ignore_ascii_case(crate::config::ROUTING_SORT_EXACTO) {
            routing.set_exacto(true);
            let (base2, floor2) = peel_context_window_suffixes(base);
            if let Some(n) = floor2 {
                routing.merge_min_context(n);
            }
            peeled = base2;
        }
    }
    catalog_model_id(&peeled)
}

pub fn catalog_model_id(model: &str) -> String {
    let mut s = String::with_capacity(model.len());
    let mut chars = model.trim().chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        s.push(c);
    }
    // Trailing `[1m]` / `[500k]` floors, CSI-like `[0;1m]`, dangling `[1m`.
    if crate::config::parse_context_window_suffix(&s).is_some() {
        s = crate::config::strip_context_window_suffix(&s).to_string();
    }
    if let Some(i) = s.rfind('[') {
        let tail = &s[i + 1..];
        let codes = tail.strip_suffix(']').unwrap_or(tail);
        if codes.ends_with('m')
            && codes.len() > 1
            && codes[..codes.len() - 1]
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b';')
        {
            s.truncate(i);
        }
    }
    s.trim().to_string()
}

pub fn pi_resolved_model(model: &str) -> String {
    let s = catalog_model_id(model);
    if is_auto_model(&s) {
        PI_DEFAULT_MODEL.to_string()
    } else {
        s
    }
}

/// Pi `models.json` `apiKey` that reads the key from `auth_env` when Pi sends a request.
///
/// Pi changed how it reads a plain `apiKey` string. The frozen
/// `@mariozechner/pi-coding-agent` (0.73) looks it up as an env var name.
/// Current Pi (`@earendil-works/pi-coding-agent`, 0.79.4 and newer) sends it as
/// the key itself, so `"ANYROUTER_API_KEY"` went out as
/// `Bearer ANYROUTER_API_KEY` and got a 401.
/// `$VAR` only works on the new versions. A `!` command runs on both, and the
/// key stays out of the file and argv.
pub fn pi_api_key_ref(auth_env: &str) -> Result<String, String> {
    if !is_env_var_name(auth_env) {
        return Err(format!(
            "Pi's auth_env \"{auth_env}\" is not a valid environment variable name. Fix tools.pi.auth_env in your AnyRouter config."
        ));
    }
    Ok(format!("!printf %s \"${auth_env}\""))
}

fn is_env_var_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn grok_home_dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|p| p.join("grok"))
        .unwrap_or_else(|| PathBuf::from("grok"))
}

/// Variables a Grok launch must not inherit. Grok probes a key in either one
/// against xAI with a 400 ms timeout and, when xAI answers 401 in time, drops
/// it and never falls back to the auth provider (`Not signed in`). Blank is not
/// the same as unset: an empty value breaks the provider path too.
pub const GROK_UNSET_ENV: &[&str] = &["XAI_API_KEY", "GROK_CODE_XAI_API_KEY"];

/// Environment variable the Grok auth provider reads the AnyRouter key from.
const GROK_KEY_ENV: &str = "ANYROUTER_API_KEY";

/// Auth provider command that prints the AnyRouter key as the token.
///
/// Grok ranks a stored session (`~/.grok/auth.json`) above both
/// `GROK_CODE_XAI_API_KEY` and an auth provider, so a signed-in user sent their
/// own JWT to AnyRouter (401). Pointing `GROK_HOME` at an AnyRouter-owned
/// directory leaves no session, and then the provider command supplies the
/// token. The key stays out of argv and files: the command reads it from the
/// environment each time it runs.
///
/// The key must not be exported as `XAI_API_KEY` / `GROK_CODE_XAI_API_KEY`:
/// Grok probes such a key against xAI with a 400 ms timeout and, when xAI
/// rejects it in time, falls back to no credential at all (`Not signed in`),
/// never to this provider. Provider-only auth is deterministic. A saved
/// `tools.grok.auth_env` from an older `anyr` may still name one of those, so
/// the provider uses its own fixed variable.
fn grok_auth_provider_command() -> String {
    format!(
        "sh -c 'printf \"{{\\\"access_token\\\":\\\"%s\\\",\\\"expires_in\\\":3600}}\" \"${GROK_KEY_ENV}\"'"
    )
}

/// Entries of the user's Grok home that carry their own setup. `auth.json`
/// (the session) and runtime state (locks, sockets, caches, logs) are left out.
const GROK_SHARED_ENTRIES: &[&str] = &[
    "managed_config.toml",
    "requirements.toml",
    "trusted_folders.toml",
    "mcp_credentials.json",
    "skills",
    "hooks",
    "rules",
    "installed-plugins",
    "marketplace-cache",
    "plugin-data",
    "memory",
    "memory-v2",
    "sessions",
];

/// The Grok home the user would get without `anyr`: `$GROK_HOME`, else `~/.grok`.
pub fn grok_user_home(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    env("GROK_HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| {
            env("HOME")
                .filter(|h| !h.is_empty())
                .map(|h| format!("{h}/.grok"))
        })
        .map(PathBuf::from)
}

/// Give Grok an AnyRouter-owned home so a `grok login` session cannot outrank the key.
///
/// The user's own setup (config, MCP, skills, hooks, memory, history) is
/// symlinked in from `user_home` (`config.toml` is copied fresh each launch); `auth.json` is not. The user's home is only
/// read, never written, so a plain `grok` keeps its login.
pub fn grok_session_summary_model(model: &str) -> String {
    // #225: grok's session-title request defaults to grok-4.6 on /responses,
    // which AnyRouter 404s. Use the launch model, or anyrouter/free for auto.
    let catalog = catalog_model_id(model);
    if catalog.is_empty() || is_auto_model(&catalog) {
        "anyrouter/free".to_string()
    } else {
        catalog
    }
}

/// Set `models.session_summary` in a grok `config.toml` text (#225).
pub fn grok_config_with_session_summary(text: &str, model_id: &str) -> String {
    let escaped = model_id.replace('\\', "\\\\").replace('"', "\\\"");
    let line = format!("session_summary = \"{escaped}\"");
    let mut out: Vec<String> = Vec::new();
    let mut in_models = false;
    let mut replaced = false;
    let mut inserted = false;
    for raw in text.lines() {
        let t = raw.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if in_models && !inserted {
                out.push(line.clone());
                inserted = true;
            }
            in_models = t == "[models]";
            out.push(raw.to_string());
            continue;
        }
        if in_models && !replaced && t.starts_with("session_summary") {
            out.push(line.clone());
            replaced = true;
            inserted = true;
            continue;
        }
        out.push(raw.to_string());
    }
    if in_models && !inserted {
        out.push(line.clone());
        inserted = true;
    }
    if !inserted {
        if !out.is_empty() {
            out.push(String::new());
        }
        out.push("[models]".to_string());
        out.push(line);
    }
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

pub fn prepare_grok_home(
    env: &mut BTreeMap<String, String>,
    config_path: &Path,
    api_key: &str,
    user_home: Option<&Path>,
    model: &str,
) -> Result<(), String> {
    let dir = grok_home_dir(config_path);
    #[cfg(feature = "native")]
    {
        std::fs::create_dir_all(&dir).map_err(|e| {
            format!(
                "Could not create Grok home directory {}: {e}",
                dir.display()
            )
        })?;
        if let Some(src) = user_home.filter(|s| s.is_dir() && *s != dir.as_path()) {
            link_grok_entries(src, &dir);
            copy_grok_config(src, &dir);
        }
        // Point session titles at a model AnyRouter serves (#225), in the
        // AnyRouter-owned copy only — never the user's file.
        let cfg_path = dir.join("config.toml");
        let text = std::fs::read_to_string(&cfg_path).unwrap_or_default();
        let next = grok_config_with_session_summary(&text, &grok_session_summary_model(model));
        std::fs::write(&cfg_path, next)
            .map_err(|e| format!("Could not write {}: {e}", cfg_path.display()))?;
    }
    #[cfg(not(feature = "native"))]
    let _ = user_home;
    env.insert("GROK_HOME".into(), dir.to_string_lossy().into_owned());
    env.insert(
        "GROK_AUTH_PROVIDER_COMMAND".into(),
        grok_auth_provider_command(),
    );
    env.insert(GROK_KEY_ENV.into(), api_key.to_string());
    // A saved `tools.grok.auth_env` from an older `anyr` put the key there.
    for name in GROK_UNSET_ENV {
        env.remove(*name);
    }
    Ok(())
}

/// `config.toml` is copied, not linked, on every launch: Grok and other `anyr`
/// steps may rewrite it, and that must never reach the user's own file.
#[cfg(feature = "native")]
fn copy_grok_config(src: &Path, dir: &Path) {
    let to = dir.join("config.toml");
    let _ = std::fs::remove_file(&to);
    let _ = std::fs::copy(src.join("config.toml"), &to);
}

#[cfg(all(feature = "native", unix))]
fn link_grok_entries(src: &Path, dir: &Path) {
    for name in GROK_SHARED_ENTRIES {
        let from = src.join(name);
        let to = dir.join(name);
        let is_link = std::fs::symlink_metadata(&to).is_ok_and(|m| m.file_type().is_symlink());
        if is_link {
            if std::fs::read_link(&to).is_ok_and(|t| t == from) && from.exists() {
                continue;
            }
            let _ = std::fs::remove_file(&to);
        }
        // A real file or directory Grok made here is the user's; leave it.
        if from.exists() && std::fs::symlink_metadata(&to).is_err() {
            let _ = std::os::unix::fs::symlink(&from, &to);
        }
    }
}

#[cfg(all(feature = "native", not(unix)))]
fn link_grok_entries(_src: &Path, _dir: &Path) {}

/// The Crush config directory the user would get without `anyr`.
pub fn crush_user_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    set("CRUSH_GLOBAL_CONFIG")
        .map(PathBuf::from)
        .or_else(|| set("XDG_CONFIG_HOME").map(|x| Path::new(&x).join("crush")))
        .or_else(|| set("HOME").map(|h| Path::new(&h).join(".config").join("crush")))
}

/// The user's `crush.json` with an `anyrouter` provider listing the catalog,
/// both model slots on `model`, and the MCP gateway when `mcp_url` is set.
/// Crush expands `$VAR` in `api_key` and headers, so no key is written.
pub fn crush_config(
    user: Option<serde_json::Value>,
    base_url: &str,
    key_env: &str,
    model: &str,
    catalog: &[crate::http::CatalogModel],
    mcp_url: Option<&str>,
) -> serde_json::Value {
    let mut config = user
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let selected = catalog_model_id(model);
    let models: Vec<serde_json::Value> = pi_catalog_models(&selected, catalog)
        .into_iter()
        .map(|m| {
            let mut entry = serde_json::json!({ "id": m.id, "name": m.id });
            if let Some(n) = m.context_length.filter(|n| *n > 0) {
                entry["context_window"] = n.into();
            }
            if let Some(n) = m.max_completion_tokens.filter(|n| *n > 0) {
                entry["default_max_tokens"] = n.into();
            }
            entry
        })
        .collect();
    let key_ref = format!("${key_env}");
    let slot = serde_json::json!({ "provider": "anyrouter", "model": selected });
    let set = |config: &mut serde_json::Value, table: &str, name: &str, v: serde_json::Value| {
        if !config[table].is_object() {
            config[table] = serde_json::json!({});
        }
        config[table][name] = v;
    };
    set(
        &mut config,
        "providers",
        "anyrouter",
        serde_json::json!({
            "name": "AnyRouter",
            "type": "openai-compat",
            "base_url": base_url,
            "api_key": key_ref,
            "extra_headers": {
                "X-AnyRouter-Source": "crush",
                "X-AnyRouter-Title": "Crush via anyr CLI",
            },
            "models": models,
        }),
    );
    set(&mut config, "models", "large", slot.clone());
    set(&mut config, "models", "small", slot);
    if let Some(url) = mcp_url {
        set(
            &mut config,
            "mcp",
            "anyrouter",
            serde_json::json!({
                "type": "http",
                "url": url,
                "headers": { "Authorization": format!("Bearer ${}", crate::mcp_wiring::KEY_ENV) },
            }),
        );
    }
    config
}

pub fn crush_config_dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|p| p.join("crush"))
        .unwrap_or_else(|| PathBuf::from("crush"))
}

/// Private stand-in for Crush's data directory, next to the private config.
/// Crush keeps the picked model in `<data>/crush.json`, loads it after the
/// config and rewrites it, so the user's own data directory must stay out.
pub fn crush_data_dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|p| p.join("crush-data"))
        .unwrap_or_else(|| PathBuf::from("crush-data"))
}

/// Point Crush at an AnyRouter-owned global config directory.
///
/// `CRUSH_GLOBAL_CONFIG` replaces the user's directory, so their `crush.json`
/// is copied in (with the AnyRouter provider merged over it) and their
/// `crushrc` copied verbatim. The user's directory is only read.
#[allow(clippy::too_many_arguments)]
pub fn prepare_crush_config(
    env: &mut BTreeMap<String, String>,
    config_path: &Path,
    profile: &Profile,
    tool: &ToolConfig,
    model: &str,
    catalog: &[crate::http::CatalogModel],
    user_dir: Option<&Path>,
    mcp_url: Option<&str>,
) -> Result<(), String> {
    let dir = crush_config_dir(config_path);
    let user_dir = user_dir.filter(|d| *d != dir.as_path());
    let user_json = user_dir
        .and_then(|d| std::fs::read_to_string(d.join("crush.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok());
    let config = crush_config(
        user_json,
        &tool_base_url(profile, tool),
        &tool.auth_env,
        model,
        catalog,
        mcp_url,
    );
    write_crush_files(&dir, &config, user_dir)?;
    let data = crush_data_dir(config_path);
    #[cfg(feature = "native")]
    crate::config::ensure_private_dir(&data).map_err(|e| {
        format!(
            "Could not create Crush data directory {}: {e}",
            data.display()
        )
    })?;
    env.insert(
        "CRUSH_GLOBAL_DATA".into(),
        data.to_string_lossy().into_owned(),
    );
    env.insert(
        "CRUSH_GLOBAL_CONFIG".into(),
        dir.to_string_lossy().into_owned(),
    );
    Ok(())
}

#[cfg(feature = "native")]
fn write_crush_files(
    dir: &Path,
    config: &serde_json::Value,
    user_dir: Option<&Path>,
) -> Result<(), String> {
    crate::config::ensure_private_dir(dir).map_err(|e| {
        format!(
            "Could not create Crush config directory {}: {e}",
            dir.display()
        )
    })?;
    let body = serde_json::to_vec_pretty(config).unwrap_or_else(|_| b"{}".to_vec());
    crate::config::write_private(&dir.join("crush.json"), &body)
        .map_err(|e| format!("Could not write Crush crush.json: {e}"))?;
    let rc = dir.join("crushrc");
    let _ = std::fs::remove_file(&rc);
    if let Some(src) = user_dir.map(|d| d.join("crushrc")).filter(|p| p.is_file()) {
        std::fs::copy(&src, &rc).map_err(|e| format!("Could not copy {}: {e}", src.display()))?;
    }
    Ok(())
}

#[cfg(not(feature = "native"))]
fn write_crush_files(
    dir: &Path,
    config: &serde_json::Value,
    user_dir: Option<&Path>,
) -> Result<(), String> {
    let _ = (dir, config, user_dir);
    Ok(())
}

/// The fx connection name anyr manages in `~/.fx/settings.json`.
pub const FX_CONNECTION: &str = "anyrouter";

/// fx 0.0.11 added custom connections; an older fx ignores FX_PROVIDER and
/// asks for a Vercel AI Gateway login instead.
pub const FX_MIN_VERSION: (u32, u32, u32) = (0, 0, 11);

/// fx refuses a connection with more than 256 `model_metadata` entries and
/// exits `fx: LimitExceeded` (256 loads, 257 fails, checked on fx 0.0.16).
pub const FX_MAX_MODEL_METADATA: usize = 256;

/// `fx --version` prints `0.0.14`.
pub fn parse_fx_version(output: &str) -> Option<(u32, u32, u32)> {
    let mut parts = output.trim().trim_start_matches('v').splitn(3, '.');
    let mut next = || parts.next()?.trim().parse::<u32>().ok();
    Some((next()?, next()?, next()?))
}

#[cfg(feature = "native")]
pub fn fx_version(command: &str) -> Option<String> {
    let out = Command::new(command).arg("--version").output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(not(feature = "native"))]
pub fn fx_version(_command: &str) -> Option<String> {
    None
}

/// Refuse an fx too old for custom connections. An unreadable version passes.
pub fn check_fx_version(output: Option<&str>) -> Result<(), String> {
    match output.and_then(parse_fx_version) {
        Some(found) if found < FX_MIN_VERSION => {
            let (a, b, c) = FX_MIN_VERSION;
            let (x, y, z) = found;
            Err(format!(
                "anyr fx needs fx {a}.{b}.{c} or newer for custom model connections; found {x}.{y}.{z}. Run `fx upgrade`."
            ))
        }
        _ => Ok(()),
    }
}

/// `~/.fx/settings.json`. fx has no variable to move it: it reads `$HOME`.
pub fn fx_settings_path(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    env("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| Path::new(&h).join(".fx").join("settings.json"))
}

/// The `providers.anyrouter` connection: Chat Completions at `base_url`, the
/// key read from `key_env`, and `model_metadata` for the selected model plus
/// every catalog chat model with tool calls. fx's `/model` picker lists only
/// the ids declared there, and the context window lets fx compact. No output
/// cap: fx sends it as `max_tokens` on every request, and the catalog cap is
/// not one every route accepts.
pub fn fx_connection(
    base_url: &str,
    key_env: &str,
    model: &str,
    catalog: &[crate::http::CatalogModel],
) -> serde_json::Value {
    let has = |m: &crate::http::CatalogModel, cap: &str| m.capabilities.iter().any(|c| c == cap);
    let selected = catalog_model_id(model);
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        selected.clone(),
        serde_json::json!({ "supports_tool_use": true }),
    );
    for row in catalog {
        let id = catalog_model_id(&row.id);
        if id != selected
            && !(has(row, "chat") && has(row, "function-calling") && row.is_routable())
        {
            continue;
        }
        if id != selected && metadata.len() >= FX_MAX_MODEL_METADATA && !metadata.contains_key(&id)
        {
            continue;
        }
        let meta = metadata
            .entry(id)
            .or_insert_with(|| serde_json::json!({ "supports_tool_use": true }));
        if let Some(n) = row.context_length.filter(|n| *n > 0) {
            meta["context_window"] = n.into();
        }
        if has(row, "vision") {
            meta["supports_vision"] = true.into();
        }
    }
    serde_json::json!({
        "protocol": "openai-chat-completions",
        "base_url": base_url,
        "auth": { "type": "bearer", "env": key_env },
        "model_metadata": metadata,
    })
}

/// The `model_metadata` ids anyr wrote on an earlier launch, read from
/// `settings.json`. Empty when there is no file or no such entry.
pub fn fx_listed_models(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.pointer(&format!("/providers/{FX_CONNECTION}/model_metadata"))
                .and_then(|m| m.as_object().cloned())
        })
        .unwrap_or_default()
}

/// Put back the models an earlier launch listed, for a launch whose catalog
/// failed to load: `connection` would otherwise shrink `/model` to one id.
/// The selected model keeps the entry `fx_connection` built for it.
pub fn fx_keep_listed_models(
    connection: &mut serde_json::Value,
    previous: serde_json::Map<String, serde_json::Value>,
) {
    if let Some(metadata) = connection["model_metadata"].as_object_mut() {
        for (id, meta) in previous {
            metadata.entry(id).or_insert(meta);
        }
    }
}

/// `settings` with `providers.anyrouter` set to `connection`, or None when it
/// already is. Every other key, including the user's selected `provider` and
/// fields anyr does not know, is kept: the launch selects the connection with
/// FX_PROVIDER instead. None for `settings` means there is no file yet. A
/// file that is not a JSON object, or whose `providers` is not one, is an
/// error: replacing it would drop the user's config.
pub fn fx_settings_with(
    settings: Option<serde_json::Value>,
    connection: &serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    let mut settings = settings.unwrap_or_else(|| serde_json::json!({}));
    let Some(root) = settings.as_object_mut() else {
        return Err("it is not a JSON object".into());
    };
    let providers = root
        .entry("providers")
        .or_insert_with(|| serde_json::json!({}));
    let Some(providers) = providers.as_object_mut() else {
        return Err("its `providers` is not a JSON object".into());
    };
    if providers.get(FX_CONNECTION) == Some(connection) {
        return Ok(None);
    }
    providers.insert(FX_CONNECTION.into(), connection.clone());
    Ok(Some(settings))
}

/// Make sure `~/.fx/settings.json` holds the AnyRouter connection.
///
/// This is the one launcher that writes into the agent's own config: fx reads
/// custom connections only from that file and has no variable to move it.
/// Only `providers.anyrouter` changes, the file never holds the key, and an
/// unchanged entry is not rewritten. fx ignores the file unless the directory
/// is 0700 and the file 0600. Returns the line `--dry-run` prints.
pub fn prepare_fx_settings(
    path: &Path,
    connection: &serde_json::Value,
    dry_run: bool,
) -> Result<String, String> {
    let current = match std::fs::read_to_string(path) {
        Ok(s) => Some(serde_json::from_str::<serde_json::Value>(&s).map_err(|e| {
            format!(
                "{} is not valid JSON ({e}); fix it, then run again. anyr left it unchanged.",
                path.display()
            )
        })?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(format!(
                "Could not read {} ({e}); fix it, then run again. anyr left it unchanged.",
                path.display()
            ))
        }
    };
    let next = fx_settings_with(current, connection).map_err(|why| {
        format!(
            "{} cannot take the AnyRouter connection: {why}. Fix it, then run again. anyr left it unchanged.",
            path.display()
        )
    })?;
    let Some(next) = next else {
        return Ok(format!(
            "fx: providers.{FX_CONNECTION} in {} is current",
            path.display()
        ));
    };
    if dry_run {
        return Ok(format!(
            "fx: would set providers.{FX_CONNECTION} in {} (no key; reads ${})",
            path.display(),
            connection["auth"]["env"].as_str().unwrap_or_default()
        ));
    }
    write_fx_settings(path, &next)?;
    Ok(format!(
        "fx: set providers.{FX_CONNECTION} in {}",
        path.display()
    ))
}

#[cfg(feature = "native")]
fn write_fx_settings(path: &Path, settings: &serde_json::Value) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    crate::config::ensure_private_dir(dir)
        .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    // A dotfiles symlink stays a symlink: write the file it points at.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let tmp = target.with_extension("json.anyr-tmp");
    let body = serde_json::to_vec_pretty(settings)
        .map_err(|e| format!("Could not encode {}: {e}", target.display()))?;
    // A stale tmp from a crashed run could carry a looser mode; start clean.
    let _ = std::fs::remove_file(&tmp);
    let written = crate::config::open_private(&tmp, true).and_then(|mut f| {
        use std::io::Write;
        f.write_all(&body)?;
        f.sync_all()
    });
    let replaced = written.and_then(|()| std::fs::rename(&tmp, &target));
    replaced.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Could not write {}: {e}", target.display())
    })
}

#[cfg(not(feature = "native"))]
fn write_fx_settings(path: &Path, settings: &serde_json::Value) -> Result<(), String> {
    let _ = (path, settings);
    Ok(())
}

/// Build Pi `models.json` for the AnyRouter provider.
///
/// Pi's `/model` picker only lists ids present under `providers.anyrouter.models`.
/// Writing a single selected id made the picker show one row (e.g.
/// `dots-studio/dots-3-note-preview`) with "Only showing models from configured
/// providers". Pass the full catalog (selected first) so `/model` can switch.
///
/// Pi assumes a 128k context window unless an entry sets `contextWindow`, so
/// each entry carries the catalog `context_length` (and `maxTokens` from the
/// catalog `top_provider.max_completion_tokens`) when known.
pub fn pi_models_config(
    base_url: &str,
    api_key: &str,
    models: &[crate::http::CatalogModel],
) -> serde_json::Value {
    let models: Vec<serde_json::Value> = models
        .iter()
        .filter(|m| !m.id.is_empty())
        .map(|m| {
            let mut entry = serde_json::json!({ "id": m.id });
            if let Some(n) = m.context_length.filter(|n| *n > 0) {
                entry["contextWindow"] = n.into();
            }
            if let Some(n) = m.max_completion_tokens.filter(|n| *n > 0) {
                entry["maxTokens"] = n.into();
            }
            entry
        })
        .collect();
    serde_json::json!({
        "providers": {
            "anyrouter": {
                "baseUrl": base_url,
                "api": "openai-completions",
                "apiKey": api_key,
                "authHeader": true,
                "headers": {
                    "X-AnyRouter-Source": "pi",
                    "X-AnyRouter-Title": "pi via anyr CLI"
                },
                "models": models
            }
        }
    })
}

/// Deduped catalog entries for Pi, with `selected` first (after `pi_resolved_model`).
pub fn pi_catalog_models(
    selected: &str,
    catalog: &[crate::http::CatalogModel],
) -> Vec<crate::http::CatalogModel> {
    let selected = pi_resolved_model(selected);
    let entry = |id: &str| {
        let row = catalog.iter().find(|m| catalog_model_id(&m.id) == id);
        crate::http::CatalogModel {
            id: id.to_string(),
            name: None,
            owned_by: None,
            context_length: row.and_then(|m| m.context_length),
            max_completion_tokens: row.and_then(|m| m.max_completion_tokens),
            ..Default::default()
        }
    };
    let mut out = Vec::with_capacity(catalog.len().saturating_add(1));
    if !selected.is_empty() {
        out.push(entry(&selected));
    }
    for m in catalog {
        let id = catalog_model_id(&m.id);
        if id.is_empty() || !m.is_routable() || out.iter().any(|x| x.id == id) {
            continue;
        }
        out.push(entry(&id));
    }
    out
}

pub fn pi_agent_dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|p| p.join("pi"))
        .unwrap_or_else(|| PathBuf::from("pi"))
}

/// Pi reads `models.json` from `PI_CODING_AGENT_DIR` (not `PI_MODELS_JSON`).
/// Write a wrap dir with AnyRouter already registered and the full catalog listed.
///
/// `catalog` should be live `/v1/models` rows (any order). The selected
/// `model` is listed first; if `catalog` is empty, only the selected id is
/// written (offline / fetch-failure fallback).
pub fn prepare_pi_wrapper(
    env: &mut BTreeMap<String, String>,
    config_path: &Path,
    profile: &Profile,
    tool: &ToolConfig,
    model: &str,
    catalog: &[crate::http::CatalogModel],
) -> Result<(), String> {
    let dir = pi_agent_dir(config_path);
    let model_id = pi_resolved_model(model);
    let base = tool_base_url(profile, tool);
    let entries = pi_catalog_models(model, catalog);
    let models = pi_models_config(&base, &pi_api_key_ref(&tool.auth_env)?, &entries);
    write_pi_wrapper_files(&dir, &models, &model_id)?;
    env.insert(
        "PI_CODING_AGENT_DIR".into(),
        dir.to_string_lossy().into_owned(),
    );
    env.insert(
        "PI_MODELS_JSON".into(),
        serde_json::to_string(&models).unwrap_or_else(|_| "{}".into()),
    );
    Ok(())
}

#[cfg(feature = "native")]
fn write_pi_wrapper_files(
    dir: &Path,
    models: &serde_json::Value,
    model_id: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| {
        format!(
            "Could not create Pi wrapper directory {}: {e}",
            dir.display()
        )
    })?;
    std::fs::write(
        dir.join("models.json"),
        serde_json::to_vec_pretty(models).unwrap_or_else(|_| b"{}".to_vec()),
    )
    .map_err(|e| format!("Could not write Pi models.json: {e}"))?;
    let settings = serde_json::json!({
        "defaultProvider": "anyrouter",
        "defaultModel": model_id,
    });
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_vec_pretty(&settings).unwrap_or_else(|_| b"{}".to_vec()),
    )
    .map_err(|e| format!("Could not write Pi settings.json: {e}"))?;
    Ok(())
}

#[cfg(not(feature = "native"))]
fn write_pi_wrapper_files(
    dir: &Path,
    models: &serde_json::Value,
    model_id: &str,
) -> Result<(), String> {
    let _ = (dir, models, model_id);
    Ok(())
}

pub fn model_args_for(
    tool_name: &str,
    model: &str,
    model_mode: &str,
    context_window: Option<i64>,
) -> Vec<String> {
    if tool_name == "pi" {
        let id = pi_resolved_model(model);
        // Official AnyRouter Pi guide: `pi --provider anyrouter --model "<catalog-id>"`.
        // The provider is already `--provider anyrouter`; do not prefix it again
        // (`anyrouter/stealth/ox-alpha` 404s — the catalog id is `stealth/ox-alpha`).
        return vec!["--model".into(), id];
    }
    let catalog = catalog_model_id(model);
    if tool_name == "kilo" && (catalog.is_empty() || is_auto_model(&catalog)) {
        // Kilo ignores the inline config's top-level model, so auto needs -m too.
        let id = display_model_id(model);
        return vec!["-m".into(), format!("anyrouter/{id}")];
    }
    if catalog.is_empty() || is_auto_model(&catalog) || model_mode == "auto" {
        return vec![];
    }
    if tool_name == "grok" {
        // Without `-m` Grok opens its own default model (`grok-4.7`), which
        // AnyRouter does not serve.
        return vec!["-m".into(), catalog];
    }
    if tool_name == "qwen" {
        // A `model.name` in the user's settings.json beats OPENAI_MODEL.
        return vec!["-m".into(), catalog];
    }
    if tool_name == "kilo" {
        // On a fresh HOME kilo ignores the top-level `model` of its inline
        // config and falls back to its own default.
        return vec!["-m".into(), format!("anyrouter/{catalog}")];
    }
    if tool_name == "opencode" {
        // OpenCode's `run` selects the model from its `--model` CLI flag; the
        // injected `OPENCODE_CONFIG_CONTENT` registers the anyrouter provider
        // but `run` does not honour its top-level model. Pass the catalog id
        // as the `anyrouter/<model>` ref OpenCode resolves through the
        // configured provider. Virtual presets (`anyrouter/auto`, …) are routed
        // by the gateway per request, so there is no single id to pin.
        if is_virtual_preset(&catalog) {
            return vec![];
        }
        return vec!["--model".into(), format!("anyrouter/{catalog}")];
    }
    if tool_name != "codex" {
        return vec![];
    }
    let mut args = vec!["-c".into(), format!("model=\"{catalog}\"")];
    // Codex keys its metadata table by OpenAI ids; without this override an
    // AnyRouter id falls back to Codex's default window.
    if let Some(n) = context_window.filter(|n| *n > 0) {
        args.extend(["-c".into(), format!("model_context_window={n}")]);
    }
    args
}

/// The variable an OpenCode-style agent reads its inline JSON config from.
pub fn inline_config_env(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "opencode" => Some("OPENCODE_CONFIG_CONTENT"),
        "kilo" => Some("KILO_CONFIG_CONTENT"),
        _ => None,
    }
}

/// `https://host:port/a/b` -> (`https://host:port`, `/a/b`).
fn split_origin(url: &str) -> (&str, &str) {
    let after_scheme = url.find("://").map_or(0, |i| i + 3);
    match url[after_scheme..].find('/') {
        Some(i) => url.split_at(after_scheme + i),
        None => (url, ""),
    }
}

/// OpenCode 2.x runs `run` and the TUI against a long-lived background
/// service started earlier with its own environment, so the
/// `OPENCODE_CONFIG_CONTENT` we inject never reaches it and OpenCode answers
/// from its built-in provider. `--standalone` makes it start a private server
/// that inherits our env. OpenCode 1.x has no service and rejects the flag.
/// Remove `--model <catalog>` from `args` if present, returning the catalog.
fn extract_model_arg(out: &mut Vec<String>) -> Option<String> {
    let mp = out.iter().position(|a| a == "--model")?;
    if mp + 1 >= out.len() || out[mp + 1].starts_with('-') {
        return None;
    }
    let catalog = out[mp + 1].clone();
    out.remove(mp + 1);
    out.remove(mp);
    Some(catalog)
}

pub fn opencode_standalone_args(args: &[String], major: Option<u32>) -> Vec<String> {
    let mut out = args.to_vec();
    if major.is_some_and(|m| m >= 2) {
        // `--standalone` and `--model <id>` are flags of `run`, not root
        // flags, so they must follow the `run` subcommand. `--model` is
        // injected by `model_args_for` ahead of the user args, so pull it
        // out and re-insert it next to `--standalone` after `run`.
        let model = extract_model_arg(&mut out);
        let run_pos = out.iter().position(|a| a == "run");
        match run_pos {
            Some(pos) => {
                out.insert(pos + 1, "--standalone".into());
                if let Some(catalog) = &model {
                    out.insert(pos + 2, "--model".into());
                    out.insert(pos + 3, catalog.clone());
                }
            }
            None => {
                if out.is_empty() {
                    out.push("--standalone".into());
                    if let Some(catalog) = &model {
                        out.push("--model".into());
                        out.push(catalog.clone());
                    }
                } else if out.first().map(|s| s.starts_with('-')).unwrap_or(false) {
                    out.insert(0, "--standalone".into());
                    if let Some(catalog) = &model {
                        out.insert(1, "--model".into());
                        out.insert(2, catalog.clone());
                    }
                }
            }
        }
    }
    // v1 has no --standalone. --model is a `run` flag in v1 too, but the
    // injected OPENCODE_CONFIG_CONTENT is already read by v1 without
    // --standalone, so leaving args untouched works for both cases.
    out
}

/// Major version from `opencode --version` output such as `opencode v2.0.24`
/// or `1.4.3`.
pub fn parse_opencode_major(output: &str) -> Option<u32> {
    let digits: String = output
        .trim()
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[cfg(not(feature = "native"))]
pub fn opencode_major_version(_command: &str) -> Option<u32> {
    None
}

#[cfg(feature = "native")]
pub fn opencode_major_version(command: &str) -> Option<u32> {
    let out = Command::new(command).arg("--version").output().ok()?;
    parse_opencode_major(&String::from_utf8_lossy(&out.stdout))
}

pub fn provider_args_for(tool_name: &str, profile: &Profile) -> Vec<String> {
    if tool_name == "pi" {
        return vec!["--provider".into(), "anyrouter".into()];
    }
    if tool_name != "codex" {
        return vec![];
    }
    let tool = builtin("codex").unwrap();
    let base = tool_base_url(profile, &tool);
    vec![
        "-c".into(),
        "model_provider=\"anyrouter\"".into(),
        "-c".into(),
        "model_providers.anyrouter.name=\"AnyRouter\"".into(),
        "-c".into(),
        format!("model_providers.anyrouter.base_url=\"{base}\""),
        "-c".into(),
        "model_providers.anyrouter.env_key=\"OPENAI_API_KEY\"".into(),
        "-c".into(),
        "model_providers.anyrouter.wire_api=\"responses\"".into(),
        "-c".into(),
        "model_providers.anyrouter.requires_openai_auth=false".into(),
        "-c".into(),
        "model_providers.anyrouter.http_headers={ \"X-AnyRouter-Source\" = \"codex\", \"X-AnyRouter-Title\" = \"Codex via anyr CLI\" }".into(),
    ]
}

pub fn redact_value(key: &str, value: &str) -> String {
    let upper = key.to_ascii_uppercase();
    let looks_secret = upper.contains("KEY") || upper.contains("TOKEN") || upper.contains("AUTH");
    if looks_secret && !value.chars().all(|c| c.is_ascii_digit()) {
        if value.len() <= 8 {
            return "<redacted>".into();
        }
        let prefix: String = value.chars().take(6).collect();
        let suffix: String = value
            .chars()
            .rev()
            .take(4)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return format!("{prefix}...{suffix}");
    }
    value.to_string()
}

pub fn render_dry_run(command: &str, args: &[String], env: &BTreeMap<String, String>) -> String {
    let mut lines = vec![
        format!("command: {command}"),
        format!(
            "args: {}",
            serde_json::to_string(args).unwrap_or_else(|_| "[]".into())
        ),
        "env:".into(),
    ];
    for (k, v) in env {
        lines.push(format!("{k}={}", redact_value(k, v)));
    }
    lines.join("\n")
}

pub fn env_command_path(tool: &str, env: &BTreeMap<String, String>) -> Option<String> {
    let key = crate::install::tool_hint(tool)?.env;
    env.get(key)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(not(feature = "native"))]
pub fn spawn_child(
    command: &str,
    args: &[String],
    extra_env: &BTreeMap<String, String>,
    unset: &[&str],
) -> i32 {
    let _ = (args, extra_env, unset);
    eprintln!("spawn is not available in the browser demo ({command})");
    1
}

/// The agent's `--help` (stdout and stderr), or None if it does not finish
/// within 3 s.
#[cfg(feature = "native")]
pub fn help_text(command: &str) -> Option<String> {
    let mut child = Command::new(command)
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while child.try_wait().ok()?.is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let out = child.wait_with_output().ok()?;
    Some(format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ))
}

#[cfg(not(feature = "native"))]
pub fn help_text(_command: &str) -> Option<String> {
    None
}

#[cfg(feature = "native")]
pub fn spawn_child(
    command: &str,
    args: &[String],
    extra_env: &BTreeMap<String, String>,
    unset: &[&str],
) -> i32 {
    let mut cmd = Command::new(command);
    for name in unset {
        cmd.env_remove(name);
    }
    match cmd
        .args(args)
        .envs(extra_env)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
    {
        Ok(st) => {
            // The run's exit code is now the agent's own.
            crate::counters::agent_returned();
            st.code().unwrap_or(1)
        }
        Err(err) => {
            eprintln!("Could not start \"{command}\": {err}");
            1
        }
    }
}

#[cfg(test)]
mod fx_tests {
    use super::*;

    #[test]
    fn an_fx_without_custom_connections_is_refused_with_the_fix() {
        assert_eq!(parse_fx_version("0.0.14\n"), Some((0, 0, 14)));
        assert_eq!(parse_fx_version("v0.1.2"), Some((0, 1, 2)));
        assert_eq!(parse_fx_version("garbage"), None);
        let err = check_fx_version(Some("0.0.5")).unwrap_err();
        assert!(
            err.contains("found 0.0.5") && err.contains("fx upgrade"),
            "{err}"
        );
        assert!(check_fx_version(Some("0.0.11")).is_ok());
        assert!(check_fx_version(Some("0.0.14")).is_ok());
        assert!(
            check_fx_version(None).is_ok(),
            "unknown version must not block"
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn opencode_two_runs_standalone_so_injected_config_applies() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            opencode_standalone_args(&a(&["run", "hi"]), Some(2)),
            a(&["run", "--standalone", "hi"])
        );
        assert_eq!(
            opencode_standalone_args(&a(&["-c"]), Some(2)),
            a(&["--standalone", "-c"])
        );
        assert_eq!(opencode_standalone_args(&[], Some(2)), a(&["--standalone"]));
        assert_eq!(
            opencode_standalone_args(&a(&["models"]), Some(2)),
            a(&["models"])
        );
        assert_eq!(
            opencode_standalone_args(&a(&["run", "hi"]), Some(1)),
            a(&["run", "hi"])
        );
        assert_eq!(
            opencode_standalone_args(&a(&["run", "hi"]), None),
            a(&["run", "hi"])
        );
        assert_eq!(parse_opencode_major("opencode v2.0.24\n"), Some(2));
        assert_eq!(parse_opencode_major("1.4.3"), Some(1));
        assert_eq!(parse_opencode_major("garbage"), None);
    }

    #[test]
    fn opencode_model_follows_run_for_v2() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // `--model` is a `run` flag: it must follow `run` (and `--standalone`),
        // not precede them as a root flag.
        assert_eq!(
            opencode_standalone_args(
                &a(&["--model", "anyrouter/claude-3-5-sonnet", "run", "hi"]),
                Some(2)
            ),
            a(&[
                "run",
                "--standalone",
                "--model",
                "anyrouter/claude-3-5-sonnet",
                "hi"
            ])
        );
        // No `run` (interactive): `--standalone` leads, `--model` follows it.
        assert_eq!(
            opencode_standalone_args(
                &a(&["--model", "anyrouter/claude-3-5-sonnet", "--verbose"]),
                Some(2)
            ),
            a(&[
                "--standalone",
                "--model",
                "anyrouter/claude-3-5-sonnet",
                "--verbose"
            ])
        );
        // v1 rejects `--standalone` and leaves args untouched.
        assert_eq!(
            opencode_standalone_args(
                &a(&["--model", "anyrouter/claude-3-5-sonnet", "run", "hi"]),
                Some(1)
            ),
            a(&["--model", "anyrouter/claude-3-5-sonnet", "run", "hi"])
        );
    }

    #[test]
    fn codex_gets_model_context_window_only_for_a_concrete_model() {
        let args = model_args_for(
            "codex",
            "poolside/laguna-s-2.1",
            "concrete",
            Some(1_048_576),
        );
        assert_eq!(
            args,
            [
                "-c",
                "model=\"poolside/laguna-s-2.1\"",
                "-c",
                "model_context_window=1048576"
            ]
        );
        // Unknown or zero window: omit rather than guess.
        for none in [None, Some(0)] {
            assert_eq!(
                model_args_for("codex", "poolside/laguna-s-2.1", "concrete", none).len(),
                2
            );
        }
        // Auto: the gateway picks the model per request, so no single window.
        assert!(model_args_for("codex", "anyrouter/auto", "auto", Some(200_000)).is_empty());
    }

    #[test]
    fn opencode_passes_catalog_model_through_cli_flag() {
        // OpenCode's `run` selects the model from `--model`; the injected
        // `OPENCODE_CONFIG_CONTENT` registers the provider but `run` does not
        // honour its top-level model, so pass the `anyrouter/<model>` ref.
        assert_eq!(
            model_args_for("opencode", "claude-3-5-sonnet", "concrete", None),
            ["--model", "anyrouter/claude-3-5-sonnet"]
        );
        assert_eq!(
            model_args_for("opencode", "deepseek/deepseek-v4-flash", "concrete", None),
            ["--model", "anyrouter/deepseek/deepseek-v4-flash"]
        );
        // Virtual presets are routed by the gateway per request, pin nothing.
        assert!(model_args_for("opencode", "anyrouter/auto", "auto", None).is_empty());
        assert!(model_args_for("opencode", "anyrouter/free", "concrete", None).is_empty());
    }

    use super::*;

    fn profile() -> Profile {
        default_profile_for_env(None, Some("sk-ar-v1-secret"))
    }

    #[test]
    fn sanitize_model_id_strips_ansi_and_claude_1m_suffix() {
        assert_eq!(sanitize_model_id("stealth/ox-alpha"), "stealth/ox-alpha");
        assert_eq!(
            sanitize_model_id("stealth/ox-alpha\u{1b}[1m"),
            "stealth/ox-alpha"
        );
        assert_eq!(sanitize_model_id("stealth/ox-alpha[1m"), "stealth/ox-alpha");
        assert_eq!(
            sanitize_model_id("stealth/ox-alpha[1m]"),
            "stealth/ox-alpha"
        );
        assert_eq!(
            sanitize_model_id("\u{1b}[1mstealth/ox-alpha\u{1b}[0m"),
            "stealth/ox-alpha"
        );
        assert_eq!(
            sanitize_model_id("stealth/ox-alpha[2m]"),
            "stealth/ox-alpha"
        );
        assert_eq!(
            sanitize_model_id("stealth/ox-alpha[0;1m]"),
            "stealth/ox-alpha"
        );
        assert_eq!(pi_resolved_model("anyrouter/auto"), PI_DEFAULT_MODEL);
        assert_eq!(pi_resolved_model("auto"), PI_DEFAULT_MODEL);
        assert_eq!(
            pi_resolved_model("stealth/ox-alpha[1m]"),
            "stealth/ox-alpha"
        );
    }

    #[test]
    fn model_id_for_tool_never_appends_1m_for_claude() {
        assert_eq!(
            model_id_for_tool("claude", "stealth/ox-alpha", None),
            "stealth/ox-alpha"
        );
        assert_eq!(
            model_id_for_tool("claude", "stealth/ox-alpha[1m]", None),
            "stealth/ox-alpha"
        );
        assert_eq!(
            model_id_for_tool("claude", "stealth/ox-alpha", Some(200_000)),
            "stealth/ox-alpha"
        );
        assert_eq!(
            model_id_for_tool("claude", "stealth/ox-alpha", Some(1_000_000)),
            "stealth/ox-alpha"
        );
        assert_eq!(
            model_id_for_tool("claude", "stealth/ox-alpha[2m]", None),
            "stealth/ox-alpha"
        );
        assert_eq!(
            model_id_for_tool("pi", "stealth/ox-alpha[1m]", None),
            "stealth/ox-alpha"
        );
        assert_eq!(
            model_id_for_tool("codex", "stealth/ox-alpha[1m]", Some(1_000_000)),
            "stealth/ox-alpha"
        );
        assert_eq!(model_id_for_tool("claude", "auto", None), "anyrouter/auto");
        assert_eq!(
            model_id_for_tool("claude", "anyrouter/auto", None),
            "anyrouter/auto"
        );
        assert_eq!(
            model_id_for_tool("claude", "anyrouter/free", None),
            "anyrouter/free"
        );
        assert_eq!(
            model_id_for_tool("claude", "anyrouter/auto[1m]", None),
            "anyrouter/auto[1m]"
        );
        assert_eq!(
            model_id_for_tool("claude", "anyrouter/auto[500k]", None),
            "anyrouter/auto[500k]"
        );
        assert_eq!(
            model_id_for_tool("claude", "anyrouter/auto", Some(1_000_000)),
            "anyrouter/auto[1m]"
        );
        assert_eq!(
            model_id_for_tool("claude", "anyrouter/free[1m]", None),
            "anyrouter/free[1m]"
        );
        assert_eq!(
            model_id_for_tool("pi", "anyrouter/auto[1m]", None),
            "anyrouter/auto"
        );
        assert!(is_auto_model("anyrouter/auto[1m]"));
        assert!(is_auto_model("anyrouter/auto[500k]"));
        assert_eq!(catalog_model_id("anyrouter/auto[500k]"), "anyrouter/auto");
    }

    #[test]
    fn session_model_label_is_anyrouter_auto_preset() {
        assert_eq!(session_model_label("auto"), "anyrouter/auto");
        assert_eq!(session_model_label("anyrouter/auto"), "anyrouter/auto");
        assert_eq!(session_model_label(""), "anyrouter/auto");
        assert_eq!(
            session_model_label("stealth/ox-alpha[1m]"),
            "stealth/ox-alpha"
        );
        assert_ne!(session_model_label("auto"), "auto  ·  most used");
    }

    #[test]
    fn redact_auth_token_and_keep_numeric() {
        assert_eq!(
            redact_value("ANTHROPIC_AUTH_TOKEN", "sk-ar-v1-secret-value"),
            "sk-ar-...alue"
        );
        assert_eq!(redact_value("MAX_THINKING_TOKENS", "24000"), "24000");
    }

    #[test]
    fn build_tool_env_claude_sets_base_and_auth() {
        let tool = builtin("claude").unwrap();
        let env = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model: "auto",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        assert_eq!(
            env.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("https://anyrouter.dev/api")
        );
        assert_eq!(
            env.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str),
            Some("sk-ar-v1-secret")
        );
        assert_eq!(
            env.get("ANTHROPIC_MODEL").map(String::as_str),
            Some("anyrouter/auto")
        );
        assert_eq!(
            env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL").map(String::as_str),
            Some("anyrouter/free")
        );
        assert_eq!(
            env.get("ANTHROPIC_DEFAULT_SONNET_MODEL")
                .map(String::as_str),
            Some("anyrouter/free")
        );
        assert_eq!(
            env.get("ANTHROPIC_DEFAULT_OPUS_MODEL").map(String::as_str),
            Some("anyrouter/free")
        );
    }

    #[test]
    fn claude_pinned_model_collapses_unset_aliases() {
        let tool = builtin("claude").unwrap();
        let env = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model: "stealth/ox-alpha",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        assert_eq!(
            env.get("ANTHROPIC_MODEL").map(String::as_str),
            Some("stealth/ox-alpha")
        );
        assert_eq!(
            env.get("CLAUDE_CODE_AUTO_COMPACT_WINDOW")
                .map(String::as_str),
            Some("1000000")
        );
        // Every unset alias slot follows the pinned model so nothing
        // (subagents, automatic fallback) silently falls back to another model.
        for key in [
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
        ] {
            assert_eq!(
                env.get(key).map(String::as_str),
                Some("stealth/ox-alpha"),
                "{key} should follow the pinned model"
            );
        }
    }

    fn claude_env(model: &str, context_window: Option<i64>) -> BTreeMap<String, String> {
        let tool = builtin("claude").unwrap();
        build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model,
            effort: None,
            context_window,
            min_context: None,
            model_map: None,
        })
    }

    #[test]
    fn claude_declares_the_model_context_window() {
        // WHY: Claude Code only knows Anthropic model ids, so a third-party id
        // opens on its 200k default unless the CLI declares the real window.
        assert_eq!(
            claude_env("minimax/minimax-m3.1-flash", Some(1_000_000))
                .get("CLAUDE_CODE_MAX_CONTEXT_TOKENS")
                .map(String::as_str),
            Some("1000000")
        );
        // An unknown window is not a promise Claude Code should size a session to.
        assert!(!claude_env("minimax/minimax-m3.1-flash", None)
            .contains_key("CLAUDE_CODE_MAX_CONTEXT_TOKENS"));
        // A virtual preset picks its hop per request, and `anyrouter/auto[1m]`
        // already carries the suffix Claude Code reads, so a catalog cap would
        // shrink that session.
        assert!(!claude_env("anyrouter/auto", Some(200_000))
            .contains_key("CLAUDE_CODE_MAX_CONTEXT_TOKENS"));
        // A known window below 200k is still declared, so every surface agrees.
        assert_eq!(
            claude_env("minimax/minimax-m3.1-flash", Some(128_000))
                .get("CLAUDE_CODE_MAX_CONTEXT_TOKENS")
                .map(String::as_str),
            Some("128000")
        );
    }

    #[test]
    fn claude_explicit_alias_beats_pinned_model() {
        let tool = builtin("claude").unwrap();
        let mut p = profile();
        p.claude_sonnet = Some("z-ai/glm-4.7-flash".into());
        let env = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &p,
            api_key: "sk-ar-v1-secret",
            model: "stealth/ox-alpha",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        assert_eq!(
            env.get("ANTHROPIC_DEFAULT_SONNET_MODEL")
                .map(String::as_str),
            Some("z-ai/glm-4.7-flash")
        );
        for key in [
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
        ] {
            assert_eq!(
                env.get(key).map(String::as_str),
                Some("stealth/ox-alpha"),
                "{key} should follow the pinned model"
            );
        }
    }

    #[test]
    fn claude_uses_profile_alias_overrides() {
        let tool = builtin("claude").unwrap();
        let mut p = profile();
        p.claude_haiku = Some("z-ai/glm-4.7-flash".into());
        p.claude_sonnet = Some("anthropic/claude-sonnet-4.6".into());
        p.claude_opus = Some("anthropic/claude-opus-4.6".into());
        let env = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &p,
            api_key: "sk-ar-v1-secret",
            model: "anyrouter/auto",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        assert_eq!(
            env.get("ANTHROPIC_MODEL").map(String::as_str),
            Some("anyrouter/auto")
        );
        assert_eq!(
            env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL").map(String::as_str),
            Some("z-ai/glm-4.7-flash")
        );
        assert_eq!(
            env.get("CLAUDE_CODE_SUBAGENT_MODEL").map(String::as_str),
            Some("z-ai/glm-4.7-flash")
        );
        assert_eq!(
            env.get("ANYROUTER_MODEL_MODE").map(String::as_str),
            Some("auto")
        );
    }

    #[test]
    fn opencode_auto_still_names_an_anyrouter_model() {
        // WHY #230: auto left provider models empty and top-level model unset.
        let tool = builtin("opencode").unwrap();
        for model in ["auto", "anyrouter/auto", ""] {
            let env = build_tool_env(BuildToolEnvInput {
                tool_name: "opencode",
                tool: &tool,
                profile: &profile(),
                api_key: "sk-ar-v1-secret",
                model,
                effort: None,
                context_window: None,
                min_context: None,
                model_map: None,
            });
            let config: serde_json::Value =
                serde_json::from_str(env.get("OPENCODE_CONFIG_CONTENT").unwrap()).unwrap();
            assert_eq!(config["model"], "anyrouter/anyrouter/auto", "{model}");
            assert_eq!(
                config["provider"]["anyrouter"]["models"],
                serde_json::json!({"anyrouter/auto": {"name": "anyrouter/auto"}}),
                "{model}"
            );
        }
        let args = model_args_for("kilo", "auto", "auto", None);
        assert_eq!(args, vec!["-m", "anyrouter/anyrouter/auto"]);
    }

    #[test]
    fn build_tool_env_codex_base_ends_with_v1() {
        let tool = builtin("codex").unwrap();
        let env = build_tool_env(BuildToolEnvInput {
            tool_name: "codex",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model: "x",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        assert!(env.get("OPENAI_BASE_URL").unwrap().ends_with("/v1"));
    }

    #[test]
    fn build_tool_env_grok_has_base_no_model_env() {
        let tool = builtin("grok").unwrap();
        let env = build_tool_env(BuildToolEnvInput {
            tool_name: "grok",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model: "x",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        assert!(env.contains_key("GROK_MODELS_BASE_URL"));
        assert!(tool.model_env.is_none());
    }

    #[test]
    fn grok_session_summary_uses_launch_model_not_grok46() {
        assert_eq!(
            grok_session_summary_model("stealth/ox-alpha"),
            "stealth/ox-alpha"
        );
        assert_eq!(grok_session_summary_model("auto"), "anyrouter/free");
        let out = grok_config_with_session_summary(
            "[models]\ndefault = \"grok-4.7\"\nsession_summary = \"grok-4.6\"\n\n[ui]\n",
            "stealth/ox-alpha",
        );
        assert!(
            out.contains("session_summary = \"stealth/ox-alpha\""),
            "{out}"
        );
        assert!(!out.contains("grok-4.6"), "{out}");
        let out2 = grok_config_with_session_summary("[ui]\n", "anyrouter/free");
        assert!(
            out2.contains("[models]\nsession_summary = \"anyrouter/free\""),
            "{out2}"
        );
    }

    #[test]
    fn render_dry_run_contains_base_and_redacts_key() {
        let tool = builtin("claude").unwrap();
        let env = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret-value",
            model: "auto",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        let out = render_dry_run("claude", &[], &env);
        assert!(out.contains("ANTHROPIC_BASE_URL"));
        assert!(!out.contains("sk-ar-v1-secret-value"));
    }

    #[test]
    fn build_tool_env_pi_sets_models_json_and_auth() {
        let tool = builtin("pi").unwrap();
        let env = build_tool_env(BuildToolEnvInput {
            tool_name: "pi",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model: "z-ai/glm-4.7-flash",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        assert_eq!(
            env.get("ANYROUTER_API_KEY").map(String::as_str),
            Some("sk-ar-v1-secret")
        );
        let json = env.get("PI_MODELS_JSON").expect("PI_MODELS_JSON");
        assert!(json.contains("anyrouter.dev/api/v1"), "{json}");
        assert!(json.contains("z-ai/glm-4.7-flash"), "{json}");
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed["providers"]["anyrouter"]["apiKey"],
            "!printf %s \"$ANYROUTER_API_KEY\""
        );
        assert!(!json.contains("sk-ar-v1-secret"), "{json}");
        assert_eq!(
            provider_args_for("pi", &profile()),
            vec!["--provider".to_string(), "anyrouter".to_string()]
        );
        assert_eq!(
            model_args_for("pi", "z-ai/glm-4.7-flash", "concrete", None),
            vec!["--model".to_string(), "z-ai/glm-4.7-flash".to_string()]
        );
        assert_eq!(
            model_args_for("pi", "anyrouter/free", "concrete", None),
            vec!["--model".to_string(), "anyrouter/free".to_string()]
        );
        assert_eq!(
            model_args_for("pi", "stealth/ox-alpha", "concrete", None),
            vec!["--model".to_string(), "stealth/ox-alpha".to_string()]
        );
        assert_eq!(
            model_args_for("pi", "stealth/ox-alpha[1m", "concrete", None),
            vec!["--model".to_string(), "stealth/ox-alpha".to_string()]
        );
        assert_eq!(
            model_args_for("pi", "stealth/ox-alpha[1m]", "concrete", None),
            vec!["--model".to_string(), "stealth/ox-alpha".to_string()]
        );
        assert_eq!(
            model_args_for("pi", "auto", "auto", None),
            vec!["--model".to_string(), PI_DEFAULT_MODEL.to_string()]
        );
        assert_eq!(
            model_args_for("pi", "anyrouter/auto", "auto", None),
            vec!["--model".to_string(), PI_DEFAULT_MODEL.to_string()]
        );
    }

    #[test]
    fn prepare_pi_wrapper_writes_models_json() {
        let dir = std::env::temp_dir().join(format!("anyr-pi-wrap-{}", std::process::id()));
        let cfg = dir.join("config.yaml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tool = builtin("pi").unwrap();
        let mut env = BTreeMap::new();
        let catalog = vec![
            pi_row("anyrouter/free", None, None),
            pi_row("anthropic/claude-sonnet-4.6", Some(200_000), Some(64_000)),
            pi_row("z-ai/glm-5.2", Some(200_000), None),
            pi_row("minimax/minimax-m3.1-flash", Some(1_000_000), Some(524_288)),
        ];
        prepare_pi_wrapper(
            &mut env,
            &cfg,
            &profile(),
            &tool,
            "anyrouter/free",
            &catalog,
        )
        .unwrap();
        let agent = dir.join("pi");
        let agent_s = agent.to_string_lossy().into_owned();
        assert_eq!(
            env.get("PI_CODING_AGENT_DIR").map(String::as_str),
            Some(agent_s.as_str())
        );
        let models = std::fs::read_to_string(agent.join("models.json")).unwrap();
        assert!(models.contains("anyrouter/free"), "{models}");
        assert!(models.contains("anthropic/claude-sonnet-4.6"), "{models}");
        assert!(models.contains("z-ai/glm-5.2"), "{models}");
        assert!(models.contains("anyrouter.dev/api/v1"), "{models}");
        // Pi assumes 128k unless the entry declares the catalog window.
        let json: serde_json::Value = serde_json::from_str(&models).unwrap();
        assert_eq!(
            json["providers"]["anyrouter"]["apiKey"],
            "!printf %s \"$ANYROUTER_API_KEY\""
        );
        let list = json["providers"]["anyrouter"]["models"].as_array().unwrap();
        let m3 = list
            .iter()
            .find(|m| m["id"] == "minimax/minimax-m3.1-flash")
            .unwrap();
        assert_eq!(m3["contextWindow"], 1_000_000);
        assert_eq!(m3["maxTokens"], 524_288);
        let free = list.iter().find(|m| m["id"] == "anyrouter/free").unwrap();
        assert!(free.get("contextWindow").is_none(), "{free}");
        let settings = std::fs::read_to_string(agent.join("settings.json")).unwrap();
        assert!(
            settings.contains("\"defaultProvider\": \"anyrouter\""),
            "{settings}"
        );
        assert!(settings.contains("anyrouter/free"), "{settings}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Current Pi sends a plain `apiKey` string as the key itself. The old bare
    // `ANYROUTER_API_KEY` went out as `Bearer ANYROUTER_API_KEY` (401), and a
    // stale models.json from an older anyr must be replaced on the next launch.
    #[test]
    fn prepare_pi_wrapper_reads_the_key_from_auth_env_on_every_pi_version() {
        let dir = std::env::temp_dir().join(format!("anyr-pi-key-{}", std::process::id()));
        let cfg = dir.join("config.yaml");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pi")).unwrap();
        std::fs::write(
            dir.join("pi").join("models.json"),
            r#"{"providers":{"anyrouter":{"apiKey":"ANYROUTER_API_KEY"}}}"#,
        )
        .unwrap();
        let mut tool = builtin("pi").unwrap();
        tool.auth_env = "MY_ANYR_KEY".into();
        let mut env = BTreeMap::new();
        prepare_pi_wrapper(&mut env, &cfg, &profile(), &tool, "z-ai/glm-5.2", &[]).unwrap();
        let models = std::fs::read_to_string(dir.join("pi").join("models.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&models).unwrap();
        let api_key = json["providers"]["anyrouter"]["apiKey"].as_str().unwrap();
        assert_eq!(api_key, "!printf %s \"$MY_ANYR_KEY\"");
        #[cfg(unix)]
        {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(&api_key[1..])
                .env("MY_ANYR_KEY", "sk-ar-v1-from-env")
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), "sk-ar-v1-from-env");
        }
        tool.auth_env = "BAD;rm -rf ~".into();
        let err =
            prepare_pi_wrapper(&mut env, &cfg, &profile(), &tool, "z-ai/glm-5.2", &[]).unwrap_err();
        assert!(err.contains("tools.pi.auth_env"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn pi_row(id: &str, ctx: Option<i64>, max: Option<i64>) -> crate::http::CatalogModel {
        crate::http::CatalogModel {
            id: id.into(),
            name: None,
            owned_by: None,
            context_length: ctx,
            max_completion_tokens: max,
            ..Default::default()
        }
    }

    fn tool_row(id: &str, routable: Option<bool>) -> crate::http::CatalogModel {
        crate::http::CatalogModel {
            id: id.into(),
            capabilities: vec!["chat".into(), "function-calling".into()],
            routable,
            ..Default::default()
        }
    }

    #[test]
    fn a_model_the_gateway_says_is_not_routable_is_left_out_of_every_agent_list() {
        let rows = [
            tool_row("a/open", None),
            tool_row("b/blocked", Some(false)),
            tool_row("c/ok", Some(true)),
        ];
        let fx = fx_connection("https://gw/v1", "K", "a/open", &rows);
        let ids: Vec<_> = fx["model_metadata"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(ids, vec!["a/open", "c/ok"]);
        let pi: Vec<_> = pi_catalog_models("a/open", &rows)
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(pi, vec!["a/open", "c/ok"]);
        // The model the user asked for stays, whatever the gateway says.
        let asked = fx_connection("https://gw/v1", "K", "b/blocked", &rows);
        assert!(asked["model_metadata"]["b/blocked"].is_object());
    }

    #[test]
    fn keeping_the_listed_models_restores_them_and_never_overwrites_the_selected_entry() {
        let mut conn = fx_connection("https://gw/v1", "K", "sel/m", &[]);
        let previous = serde_json::json!({
            "sel/m": { "supports_tool_use": true, "context_window": 9 },
            "old/one": { "supports_tool_use": true, "context_window": 1000 },
            "old/two": { "supports_tool_use": true },
        });
        fx_keep_listed_models(&mut conn, previous.as_object().unwrap().clone());
        let meta = conn["model_metadata"].as_object().unwrap();
        assert_eq!(meta.len(), 3, "{meta:?}");
        assert_eq!(meta["old/one"]["context_window"], 1000);
        assert!(meta["sel/m"].get("context_window").is_none());
    }

    #[test]
    fn listed_models_come_from_providers_anyrouter_only() {
        let dir = std::env::temp_dir().join(format!("anyr-fx-listed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        assert!(fx_listed_models(&path).is_empty(), "no file");
        std::fs::write(&path, r#"{"providers":{"local":{"model_metadata":{"x":{}}},"anyrouter":{"model_metadata":{"a/b":{}}}}}"#).unwrap();
        assert_eq!(
            fx_listed_models(&path).keys().collect::<Vec<_>>(),
            vec!["a/b"]
        );
        std::fs::write(&path, "not json").unwrap();
        assert!(fx_listed_models(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pi_catalog_models_puts_selected_first_dedupes_and_keeps_limits() {
        let rows = pi_catalog_models(
            "z-ai/glm-5.2",
            &[
                pi_row("anyrouter/free", None, None),
                pi_row("z-ai/glm-5.2", Some(200_000), Some(16_384)),
                pi_row("anyrouter/free", None, None),
            ],
        );
        let ids: Vec<&str> = rows.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["z-ai/glm-5.2", "anyrouter/free"]);
        assert_eq!(rows[0].context_length, Some(200_000));
        assert_eq!(rows[0].max_completion_tokens, Some(16_384));
    }

    #[test]
    fn pi_models_config_declares_each_models_context_window() {
        let json = pi_models_config(
            "https://anyrouter.dev/api/v1",
            "!printf %s \"$ANYROUTER_API_KEY\"",
            &[
                pi_row("a/b", Some(1_000_000), Some(524_288)),
                pi_row("c/d", None, None),
            ],
        );
        let models = json["providers"]["anyrouter"]["models"]
            .as_array()
            .expect("models array");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["id"], "a/b");
        assert_eq!(models[0]["contextWindow"], 1_000_000);
        assert_eq!(models[0]["maxTokens"], 524_288);
        assert_eq!(models[1]["id"], "c/d");
        assert!(models[1].get("contextWindow").is_none());
    }

    #[test]
    fn launchers_send_source_and_via_anyr_cli_title() {
        // WHY #275: a launched request must count for the agent and anyr CLI.
        let pi = pi_models_config("https://anyrouter.dev/api/v1", "k", &[]);
        assert_eq!(
            pi["providers"]["anyrouter"]["headers"]["X-AnyRouter-Source"],
            "pi"
        );
        assert_eq!(
            pi["providers"]["anyrouter"]["headers"]["X-AnyRouter-Title"],
            "pi via anyr CLI"
        );
        let crush = crush_config(
            None,
            "https://anyrouter.dev/api/v1",
            "ANYROUTER_API_KEY",
            "m",
            &[],
            None,
        );
        assert_eq!(
            crush["providers"]["anyrouter"]["extra_headers"]["X-AnyRouter-Source"],
            "crush"
        );
        assert_eq!(
            crush["providers"]["anyrouter"]["extra_headers"]["X-AnyRouter-Title"],
            "Crush via anyr CLI"
        );
        let codex_args = provider_args_for("codex", &profile());
        let joined = codex_args.join(" ");
        assert!(joined.contains("X-AnyRouter-Source"), "{joined}");
        assert!(joined.contains("Codex via anyr CLI"), "{joined}");
        let tool = builtin("claude").unwrap();
        let env = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret000000000000",
            model: "auto",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        let headers = env.get("ANTHROPIC_CUSTOM_HEADERS").expect("claude headers");
        assert!(
            headers.contains("X-AnyRouter-Source: claude-code"),
            "{headers}"
        );
        assert!(
            headers.contains("X-AnyRouter-Title: Claude Code via anyr CLI"),
            "{headers}"
        );
    }

    #[test]
    fn claude_minimal_effort_clamps_to_low() {
        let tool = builtin("claude").unwrap();
        let env = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model: "anthropic/claude-sonnet-4.6",
            effort: Some("minimal"),
            context_window: None,
            min_context: None,
            model_map: None,
        });
        assert_eq!(
            env.get("MAX_THINKING_TOKENS").map(String::as_str),
            Some("2048")
        );
    }

    #[test]
    fn apply_routing_env_sets_claude_extra_body() {
        let mut env = BTreeMap::new();
        let mut routing = crate::config::RoutingConstraints::default();
        apply_routing_env(&mut env, &routing, "claude");
        assert!(!env.contains_key("CLAUDE_CODE_EXTRA_BODY"));
        routing.set_exacto(true);
        routing.set_require_tools(true);
        routing.set_require_1m(true);
        apply_routing_env(&mut env, &routing, "claude");
        let body = env.get("CLAUDE_CODE_EXTRA_BODY").expect("extra body");
        assert!(body.contains("\"sort\":\"exacto\""), "{body}");
        assert!(body.contains("\"require_params\":[\"tools\"]"), "{body}");
        assert!(body.contains("\"min_context\":1000000"), "{body}");
        assert!(body.contains("\"provider\""), "{body}");
        assert_eq!(env.get("ANYROUTER_EXTRA_BODY"), Some(body));
    }

    #[test]
    fn peel_auto_1m_and_500k_into_min_context() {
        let (id, floor) = peel_context_window_suffixes("anyrouter/auto[1m]");
        assert_eq!(id, "anyrouter/auto");
        assert_eq!(floor, Some(1_000_000));
        let (id, floor) = peel_context_window_suffixes("anyrouter/auto[500k]");
        assert_eq!(id, "anyrouter/auto");
        assert_eq!(floor, Some(500_000));
        let mut routing = crate::config::RoutingConstraints::default();
        let catalog = apply_model_id_routing("anyrouter/auto[1m]:exacto", &mut routing);
        assert_eq!(catalog, "anyrouter/auto");
        assert!(is_auto_model(&catalog));
        assert_eq!(routing.min_context, Some(1_000_000));
        assert!(routing.wants_exacto());
        let body = routing.extra_body_json().expect("body");
        assert!(body.contains("\"min_context\":1000000"), "{body}");
        assert!(body.contains("\"sort\":\"exacto\""), "{body}");
        for id in [
            "anyrouter/free[1m]",
            "anyrouter/byok[1m]",
            "anyrouter/hermes[500k]",
            "anyrouter/latest[1m]",
        ] {
            let mut r = crate::config::RoutingConstraints::default();
            let catalog = apply_model_id_routing(id, &mut r);
            assert!(is_virtual_preset(id), "{id}");
            assert!(!catalog.contains('['), "{catalog}");
            assert!(r.min_context.is_some(), "{id}");
        }
    }

    #[test]
    fn claude_virtual_auto_1m_sets_anthropic_model_suffix_and_compact() {
        let tool = builtin("claude").unwrap();
        let mut env = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model: "anyrouter/auto[1m]",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        let mut routing = crate::config::RoutingConstraints::default();
        let catalog = apply_model_id_routing("anyrouter/auto[1m]", &mut routing);
        assert_eq!(catalog, "anyrouter/auto");
        apply_routing_env(&mut env, &routing, "claude");
        assert_eq!(
            env.get("ANTHROPIC_MODEL").map(String::as_str),
            Some("anyrouter/auto[1m]")
        );
        let body = env.get("CLAUDE_CODE_EXTRA_BODY").expect("extra body");
        assert!(body.contains("\"min_context\":1000000"), "{body}");
        assert_eq!(
            env.get("CLAUDE_CODE_AUTO_COMPACT_WINDOW")
                .map(String::as_str),
            Some("1000000")
        );

        // Launch peels the suffix first and only passes routing.min_context.
        let peeled = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model: "anyrouter/auto",
            effort: None,
            context_window: Some(200_000),
            min_context: Some(1_000_000),
            model_map: None,
        });
        assert_eq!(
            peeled.get("ANTHROPIC_MODEL").map(String::as_str),
            Some("anyrouter/auto[1m]"),
            "do not use catalog 200k as the HUD suffix"
        );
        assert_eq!(
            peeled
                .get("CLAUDE_CODE_AUTO_COMPACT_WINDOW")
                .map(String::as_str),
            Some("1000000")
        );
        assert!(!claude_gateway_discovery_enabled(
            &tool,
            "anyrouter/auto[1m]"
        ));
        let auto_row = crate::http::CatalogModel {
            id: "anyrouter/auto".into(),
            name: None,
            owned_by: None,
            context_length: Some(200_000),
            max_completion_tokens: None,
            ..Default::default()
        };
        let ox = crate::http::CatalogModel {
            id: "stealth/ox-alpha".into(),
            name: None,
            owned_by: None,
            context_length: Some(1_000_000),
            max_completion_tokens: None,
            ..Default::default()
        };
        assert_eq!(
            catalog_context_window("anyrouter/auto[1m]", &[auto_row.clone(), ox.clone()]),
            None,
            "virtual preset must not inherit catalog 200k"
        );
        assert_eq!(
            catalog_context_window("stealth/ox-alpha", &[auto_row, ox]),
            Some(1_000_000)
        );
        assert!(claude_wants_auto_compact(
            "anyrouter/auto",
            Some(1_000_000),
            Some(200_000)
        ));
        assert!(!claude_wants_auto_compact(
            "anyrouter/auto[500k]",
            None,
            Some(200_000)
        ));

        let half = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model: "anyrouter/auto[500k]",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        assert_eq!(
            half.get("ANTHROPIC_MODEL").map(String::as_str),
            Some("anyrouter/auto[500k]")
        );
        assert!(!half.contains_key("CLAUDE_CODE_AUTO_COMPACT_WINDOW"));
    }

    #[test]
    fn claude_shadow_env_overrides_parent_anthropic_key() {
        // WHY: a leftover ANTHROPIC_API_KEY in the parent shell must not win.
        let tool = builtin("claude").unwrap();
        assert_eq!(tool.shadow_env.as_deref(), Some("ANTHROPIC_API_KEY"));
        let env = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model: "auto",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        assert_eq!(
            env.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str),
            Some("sk-ar-v1-secret")
        );
        assert_eq!(
            env.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-ar-v1-secret")
        );
        assert_eq!(
            env.get("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY")
                .map(String::as_str),
            Some("0"),
            "auto must not be remapped by catalog discovery"
        );
        let free = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model: "anyrouter/free[1m]",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        assert_eq!(
            free.get("ANTHROPIC_MODEL").map(String::as_str),
            Some("anyrouter/free[1m]")
        );
        assert_eq!(
            free.get("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY")
                .map(String::as_str),
            Some("0")
        );
        let concrete = build_tool_env(BuildToolEnvInput {
            tool_name: "claude",
            tool: &tool,
            profile: &profile(),
            api_key: "sk-ar-v1-secret",
            model: "poolside/laguna-s-2.1",
            effort: None,
            context_window: None,
            min_context: None,
            model_map: None,
        });
        assert_eq!(
            concrete
                .get("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn apply_yaml_partial_does_not_wipe_gateway_discovery() {
        let mut tool = builtin("claude").unwrap();
        let mut map = BTreeMap::new();
        map.insert("command".into(), YamlValue::String("/opt/claude".into()));
        tool.apply_yaml(&map);
        assert_eq!(tool.command, "/opt/claude");
        assert!(
            tool.enable_gateway_model_discovery,
            "partial YAML must keep builtin discovery"
        );
    }

    #[test]
    fn extra_yolo_round_trips_in_yaml() {
        let mut tool = builtin("claude").unwrap();
        let mut map = BTreeMap::new();
        map.insert("yolo".into(), YamlValue::Bool(true));
        tool.apply_yaml(&map);
        assert!(tool.extra_flag("yolo"));
        let yaml = tool.to_yaml_lines().join("\n");
        assert!(yaml.contains("yolo: true"), "{yaml}");
        let parsed = crate::config::parse_config(
            "active_profile: default\nprofiles:\n  default:\n    api_key: x\ntools:\n  claude:\n    yolo: true\n",
        );
        let again = parsed.tools.get("claude").cloned().unwrap();
        assert!(
            again.extra_flag("yolo"),
            "parse_config must keep extra yolo"
        );
    }

    #[test]
    fn merge_command_only_overlay_keeps_codex_suffix() {
        let mut t = builtin("codex").unwrap();
        let over = ToolConfig {
            command: "/opt/codex".into(),
            ..Default::default()
        };
        t.merge(&over);
        assert_eq!(t.command, "/opt/codex");
        assert_eq!(t.base_suffix, "/v1");
    }
}
