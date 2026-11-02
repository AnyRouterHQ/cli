use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::spawn::{create_default_tools, ToolConfig};

pub const DEFAULT_BASE_URL: &str = "https://anyrouter.dev/api";
pub const DEFAULT_TIMEOUT_MS: i64 = 3_000_000;
pub const DEFAULT_PROFILE: &str = "default";
pub const DEFAULT_PRESET: &str = "@preset/coding-stack";
/// Documented gateway auto preset. Never invent a catalog SKU for this slot.
pub const DEFAULT_MODEL: &str = "anyrouter/auto";
/// Free-tier defaults: work for a brand-new account (no credits, no BYOK).
/// Switch with `--model` or `anyr models use`. Keep as string literals; the
/// anyrouter repo check parses them.
pub const DEFAULT_CLAUDE_HAIKU: &str = "anyrouter/free";
pub const DEFAULT_CLAUDE_SONNET: &str = "anyrouter/free";
pub const DEFAULT_CLAUDE_OPUS: &str = "anyrouter/free";
pub const DEFAULT_CLAUDE_FABLE: &str = "anyrouter/free";

#[derive(Debug, Clone, PartialEq)]
pub enum YamlValue {
    Map(BTreeMap<String, YamlValue>),
    String(String),
    Bool(bool),
    Int(i64),
    Null,
}

impl YamlValue {
    pub fn as_map(&self) -> Option<&BTreeMap<String, YamlValue>> {
        match self {
            YamlValue::Map(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            YamlValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_string_lossy(&self) -> String {
        match self {
            YamlValue::String(s) => s.clone(),
            YamlValue::Bool(b) => b.to_string(),
            YamlValue::Int(n) => n.to_string(),
            YamlValue::Null => String::new(),
            YamlValue::Map(_) => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Profile {
    pub api_key: Option<String>,
    pub management_key: Option<String>,
    pub base_url: Option<String>,
    pub pinned_preset: Option<String>,
    pub default_model: Option<String>,
    pub default_tool: Option<String>,
    pub claude_haiku: Option<String>,
    pub claude_sonnet: Option<String>,
    pub claude_opus: Option<String>,
    pub claude_fable: Option<String>,
    pub timeout_ms: Option<i64>,
    /// rk_ pairing token for `anyr relay` (shared with the TS CLI).
    pub relay_token: Option<String>,
    /// Cached paired-device id so `relay --pool` can PATCH without a lookup.
    pub relay_device_id: Option<String>,
    /// Device name chosen once for `anyr relay`, so it stays stable.
    pub relay_device_name: Option<String>,
    pub extra: BTreeMap<String, YamlValue>,
}

impl Profile {
    pub fn base_url(&self) -> &str {
        self.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL)
    }

    pub fn pinned_preset(&self) -> &str {
        self.pinned_preset.as_deref().unwrap_or(DEFAULT_PRESET)
    }

    pub fn default_model(&self) -> &str {
        self.default_model.as_deref().unwrap_or(DEFAULT_MODEL)
    }

    pub fn claude_haiku(&self) -> &str {
        nonempty(&self.claude_haiku).unwrap_or(DEFAULT_CLAUDE_HAIKU)
    }

    pub fn claude_sonnet(&self) -> &str {
        nonempty(&self.claude_sonnet).unwrap_or(DEFAULT_CLAUDE_SONNET)
    }

    pub fn claude_opus(&self) -> &str {
        nonempty(&self.claude_opus).unwrap_or(DEFAULT_CLAUDE_OPUS)
    }

    pub fn claude_fable(&self) -> &str {
        nonempty(&self.claude_fable).unwrap_or(DEFAULT_CLAUDE_FABLE)
    }

    pub fn timeout_ms(&self) -> i64 {
        self.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)
    }
}

fn nonempty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Per-coding-agent overrides. When set, launch uses these instead of the
/// session-wide active profile / default model. A stored `api_key` is used
/// as-is — launch must not silently fall back to the default profile key.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentBinding {
    pub profile: Option<String>,
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    /// Flags passed to the agent on every launch, before the user's own
    /// (`agents.<id>.args`, space separated).
    pub args: Vec<String>,
    /// Request-level routing controls. Field names match AnyRouter presets
    /// (`provider.sort`, `require_params`, `min_context`).
    pub routing: RoutingConstraints,
}

impl AgentBinding {
    pub fn is_empty(&self) -> bool {
        self.profile
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
            && self
                .api_key
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
            && self
                .default_model
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
            && self.args.is_empty()
            && self.routing.is_empty()
    }
}

/// Routing filters forwarded on launch. Same names as AnyRouter preset /
/// chat-completion request fields (API issue sibling).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RoutingConstraints {
    /// `provider.sort` — `"exacto"` when the user wants Exacto quality routing.
    pub sort: Option<String>,
    /// `require_params` — e.g. `["tools"]` to require tool-calling backends.
    pub require_params: Vec<String>,
    /// `min_context` — e.g. `1_000_000` to require a ≥1M context window.
    pub min_context: Option<i64>,
}

pub const ROUTING_SORT_EXACTO: &str = "exacto";
pub const ROUTING_PARAM_TOOLS: &str = "tools";
pub const ROUTING_MIN_1M_CONTEXT: i64 = 1_000_000;

/// `[1m]` → 1_000_000, `[500k]` → 500_000. None if the id has no floor suffix.
pub fn parse_context_window_suffix(model: &str) -> Option<i64> {
    let s = model.trim();
    let start = s.rfind('[')?;
    if !s.ends_with(']') || start + 2 >= s.len() {
        return None;
    }
    let inner = &s[start + 1..s.len() - 1];
    if inner.len() < 2 {
        return None;
    }
    let (num, unit) = inner.split_at(inner.len() - 1);
    let n: i64 = num.parse().ok()?;
    if n <= 0 {
        return None;
    }
    match unit {
        "k" | "K" => Some(n.saturating_mul(1_000)),
        "m" | "M" => Some(n.saturating_mul(1_000_000)),
        _ => None,
    }
}

/// Catalog id without a `[<n>k|m]` floor suffix.
pub fn strip_context_window_suffix(model: &str) -> &str {
    let s = model.trim();
    if parse_context_window_suffix(s).is_some() {
        if let Some(i) = s.rfind('[') {
            return s[..i].trim_end();
        }
    }
    s
}

impl RoutingConstraints {
    pub fn is_empty(&self) -> bool {
        self.sort.as_deref().map(str::trim).unwrap_or("").is_empty()
            && self.require_params.is_empty()
            && self.min_context.is_none()
    }

    pub fn wants_exacto(&self) -> bool {
        self.sort
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| s.eq_ignore_ascii_case(ROUTING_SORT_EXACTO))
    }

    pub fn requires_tools(&self) -> bool {
        self.require_params
            .iter()
            .any(|p| p.trim().eq_ignore_ascii_case(ROUTING_PARAM_TOOLS))
    }

    pub fn requires_1m_context(&self) -> bool {
        self.min_context
            .is_some_and(|n| n >= ROUTING_MIN_1M_CONTEXT)
    }

    pub fn set_exacto(&mut self, on: bool) {
        self.sort = on.then(|| ROUTING_SORT_EXACTO.to_string());
    }

    pub fn set_require_tools(&mut self, on: bool) {
        self.require_params
            .retain(|p| !p.trim().eq_ignore_ascii_case(ROUTING_PARAM_TOOLS));
        if on {
            self.require_params.push(ROUTING_PARAM_TOOLS.to_string());
        }
    }

    pub fn set_require_1m(&mut self, on: bool) {
        self.min_context = on.then_some(ROUTING_MIN_1M_CONTEXT);
    }

    /// Raise the token floor. Higher (stricter) wins.
    pub fn merge_min_context(&mut self, floor: i64) {
        if floor <= 0 {
            return;
        }
        self.min_context = Some(match self.min_context {
            Some(existing) => existing.max(floor),
            None => floor,
        });
    }

    /// Merge `[1m]` / `[500k]` on a model id into `min_context` (higher wins).
    pub fn apply_model_id_context_suffix(&mut self, model: &str) {
        if let Some(n) = parse_context_window_suffix(model) {
            self.merge_min_context(n);
        }
    }

    /// JSON object merged into the inference request body (Claude
    /// `CLAUDE_CODE_EXTRA_BODY`, printed on dry-run for every agent).
    pub fn extra_body_json(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut body = serde_json::Map::new();
        let mut provider = serde_json::Map::new();
        if let Some(sort) = self
            .sort
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            provider.insert("sort".into(), serde_json::json!(sort));
        }
        if let Some(n) = self.min_context {
            provider.insert("min_context".into(), serde_json::json!(n));
        }
        if !provider.is_empty() {
            body.insert("provider".into(), serde_json::Value::Object(provider));
        }
        if !self.require_params.is_empty() {
            let params: Vec<&str> = self
                .require_params
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if !params.is_empty() {
                body.insert("require_params".into(), serde_json::json!(params));
            }
        }
        if body.is_empty() {
            return None;
        }
        Some(serde_json::Value::Object(body).to_string())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub active_profile: String,
    pub last_tool: Option<String>,
    /// `None` means default on. Explicit `false` disables background auto-update.
    pub auto_update: Option<bool>,
    /// Upgrade channel: `stable` (default) or `beta`. Not shown in `ar config`.
    pub channel: Option<String>,
    /// `mcp.auto`: `None` follows the workspace setting; `false` never wires the
    /// AnyRouter MCP gateway into a launched agent on this machine.
    pub mcp_auto: Option<bool>,
    pub profiles: BTreeMap<String, Profile>,
    /// Per-agent model / account / key. Keys are canonical tool ids (`claude`, …).
    pub agents: BTreeMap<String, AgentBinding>,
    pub tools: BTreeMap<String, ToolConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            active_profile: DEFAULT_PROFILE.into(),
            last_tool: None,
            auto_update: None,
            channel: None,
            mcp_auto: None,
            profiles: BTreeMap::new(),
            agents: BTreeMap::new(),
            tools: create_default_tools(),
        }
    }
}

impl Config {
    pub fn auto_update(&self) -> bool {
        self.auto_update.unwrap_or(true)
    }

    pub fn channel(&self) -> &str {
        match self
            .channel
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(ch) => ch,
            None => "stable",
        }
    }

    pub fn agent_binding(&self, tool: &str) -> Option<&AgentBinding> {
        let id = crate::spawn::canonical_tool(tool);
        self.agents.get(id).or_else(|| self.agents.get(tool))
    }

    pub fn agent_binding_mut(&mut self, tool: &str) -> &mut AgentBinding {
        let id = crate::spawn::canonical_tool(tool).to_string();
        self.agents.entry(id).or_default()
    }

    /// The agent a bare launch starts: `--tool`, then `launcher.agent`
    /// (`default_tool`), then the last one launched.
    pub fn preferred_agent(&self, flag: Option<&str>) -> Option<String> {
        let pinned = self
            .profiles
            .get(&self.active_profile)
            .and_then(|p| p.default_tool.as_deref());
        [flag, pinned, self.last_tool.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|s| !s.is_empty())
            .map(str::to_string)
    }

    pub fn prune_empty_agents(&mut self) {
        self.agents.retain(|_, b| !b.is_empty());
    }

    /// Point every agent bound to account `old` at `new`, or unbind it when
    /// `new` is `None`. Returns the agents it changed.
    pub fn rebind_agents(&mut self, old: &str, new: Option<&str>) -> Vec<String> {
        let mut changed = Vec::new();
        for (id, binding) in &mut self.agents {
            if binding.profile.as_deref().map(str::trim) == Some(old) {
                binding.profile = new.map(str::to_string);
                changed.push(id.clone());
            }
        }
        self.prune_empty_agents();
        changed
    }
}

pub struct DefaultProfileInput {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub preset: Option<String>,
    pub timeout_ms: Option<i64>,
    pub default_model: Option<String>,
}

pub fn normalize_preset(input: &str) -> String {
    let value = input.trim();
    if let Some(rest) = value.strip_prefix("@presets/") {
        return format!("@preset/{rest}");
    }
    if value.starts_with("@preset/") {
        return value.to_string();
    }
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return format!("@preset/{value}");
    }
    value.to_string()
}

pub fn create_default_profile(input: DefaultProfileInput) -> Profile {
    Profile {
        api_key: input.api_key,
        base_url: Some(input.base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into())),
        pinned_preset: Some(normalize_preset(
            input.preset.as_deref().unwrap_or(DEFAULT_PRESET),
        )),
        default_model: Some(input.default_model.unwrap_or_else(|| DEFAULT_MODEL.into())),
        timeout_ms: Some(input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
        ..Profile::default()
    }
}

pub fn resolve_config_path(flag: Option<&str>, env: &BTreeMap<String, String>) -> PathBuf {
    if let Some(p) = flag.filter(|s| !s.is_empty()) {
        return PathBuf::from(p);
    }
    if let Some(home) = env
        .get("ANYROUTER_HOME")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return PathBuf::from(home).join("config.yaml");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".anyrouter")
        .join("config.yaml")
}

pub fn get_active_profile<'a>(
    config: &'a Config,
    profile_name: Option<&str>,
    env: &BTreeMap<String, String>,
) -> Result<&'a Profile, String> {
    let env_name = env
        .get("ANYROUTER_PROFILE")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let name = profile_name
        .filter(|s| !s.is_empty())
        .or(env_name)
        .unwrap_or(config.active_profile.as_str());
    config
        .profiles
        .get(name)
        .ok_or_else(|| format!("Profile \"{name}\" was not found in AnyRouter config."))
}

pub fn upsert_profile(mut config: Config, name: &str, profile: Profile) -> Config {
    config.profiles.insert(name.to_string(), profile);
    if config.active_profile.is_empty() {
        config.active_profile = name.to_string();
    }
    config
}

pub fn set_active_profile(mut config: Config, name: &str) -> Result<Config, String> {
    if !config.profiles.contains_key(name) {
        let existing = config
            .profiles
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "Account \"{name}\" was not found. Existing: {}.",
            if existing.is_empty() {
                "(none)".into()
            } else {
                existing
            }
        ));
    }
    config.active_profile = name.to_string();
    Ok(config)
}

pub fn valid_account_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

fn parse_yaml_scalar(raw: &str) -> YamlValue {
    match raw {
        "true" => YamlValue::Bool(true),
        "false" => YamlValue::Bool(false),
        "null" => YamlValue::Null,
        _ if raw.chars().all(|c| c.is_ascii_digit() || c == '-') && raw.parse::<i64>().is_ok() => {
            YamlValue::Int(raw.parse().unwrap())
        }
        _ if raw.starts_with('"') => serde_json::from_str::<String>(raw)
            .map(YamlValue::String)
            .unwrap_or_else(|_| YamlValue::String(raw.trim_matches('"').into())),
        _ => YamlValue::String(raw.to_string()),
    }
}

fn yaml_scalar(value: &str) -> String {
    if value.is_empty()
        || value.contains([':', '#', '@'])
        || value.starts_with(' ')
        || value.ends_with(' ')
    {
        return serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""));
    }
    value.to_string()
}

pub fn yaml_scalar_value(value: &YamlValue) -> String {
    match value {
        YamlValue::Bool(b) => b.to_string(),
        YamlValue::Int(n) => n.to_string(),
        YamlValue::Null => "null".into(),
        YamlValue::String(s) => yaml_scalar(s),
        YamlValue::Map(_) => String::new(),
    }
}

fn map_at_path<'a>(
    root: &'a mut BTreeMap<String, YamlValue>,
    path: &[String],
) -> &'a mut BTreeMap<String, YamlValue> {
    let mut cur = root;
    for key in path {
        cur = match cur.get_mut(key) {
            Some(YamlValue::Map(m)) => m,
            _ => panic!("yaml path missing {key}"),
        };
    }
    cur
}

fn parse_yaml_map(source: &str) -> BTreeMap<String, YamlValue> {
    let mut root = BTreeMap::new();
    let mut stack: Vec<(isize, Vec<String>)> = vec![(-1, vec![])];

    for raw_line in source.split('\n') {
        let raw_line = raw_line.trim_end_matches('\r');
        if raw_line.trim().is_empty() || raw_line.trim().starts_with('#') {
            continue;
        }
        let indent = raw_line.len() - raw_line.trim_start().len();
        let line = raw_line.trim();
        let Some(colon) = line.find(':') else {
            continue;
        };
        let key = line[..colon].trim().to_string();
        let raw_value = line[colon + 1..].trim();

        while stack.len() > 1 && indent as isize <= stack.last().unwrap().0 {
            stack.pop();
        }
        let parent_path = stack.last().unwrap().1.clone();
        let parent = map_at_path(&mut root, &parent_path);

        if raw_value.is_empty() {
            parent.insert(key.clone(), YamlValue::Map(BTreeMap::new()));
            let mut child_path = parent_path;
            child_path.push(key);
            stack.push((indent as isize, child_path));
        } else {
            parent.insert(key, parse_yaml_scalar(raw_value));
        }
    }
    root
}

fn profile_from_map(map: &BTreeMap<String, YamlValue>) -> Profile {
    let mut p = Profile::default();
    for (k, v) in map {
        match k.as_str() {
            "api_key" => p.api_key = Some(v.as_string_lossy()).filter(|s| !s.is_empty()),
            "management_key" => {
                p.management_key = Some(v.as_string_lossy()).filter(|s| !s.is_empty())
            }
            "base_url" => p.base_url = Some(v.as_string_lossy()),
            "pinned_preset" => p.pinned_preset = Some(normalize_preset(&v.as_string_lossy())),
            "default_model" => p.default_model = Some(v.as_string_lossy()),
            "default_tool" => p.default_tool = Some(v.as_string_lossy()).filter(|s| !s.is_empty()),
            "claude_haiku" => p.claude_haiku = Some(v.as_string_lossy()).filter(|s| !s.is_empty()),
            "claude_sonnet" => {
                p.claude_sonnet = Some(v.as_string_lossy()).filter(|s| !s.is_empty())
            }
            "claude_opus" => p.claude_opus = Some(v.as_string_lossy()).filter(|s| !s.is_empty()),
            "claude_fable" => p.claude_fable = Some(v.as_string_lossy()).filter(|s| !s.is_empty()),
            "timeout_ms" => {
                p.timeout_ms = match v {
                    YamlValue::Int(n) => Some(*n),
                    _ => v.as_string_lossy().parse().ok(),
                }
            }
            "relay_token" => p.relay_token = Some(v.as_string_lossy()).filter(|s| !s.is_empty()),
            "relay_device_id" => {
                p.relay_device_id = Some(v.as_string_lossy()).filter(|s| !s.is_empty())
            }
            "relay_device_name" => {
                p.relay_device_name = Some(v.as_string_lossy()).filter(|s| !s.is_empty())
            }
            _ => {
                p.extra.insert(k.clone(), v.clone());
            }
        }
    }
    p
}

fn agent_binding_from_map(map: &BTreeMap<String, YamlValue>) -> AgentBinding {
    let mut b = AgentBinding::default();
    for (k, v) in map {
        match k.as_str() {
            "profile" => b.profile = Some(v.as_string_lossy()).filter(|s| !s.is_empty()),
            "api_key" => b.api_key = Some(v.as_string_lossy()).filter(|s| !s.is_empty()),
            "default_model" => {
                b.default_model = Some(v.as_string_lossy()).filter(|s| !s.is_empty())
            }
            "provider" => {
                if let Some(pm) = v.as_map() {
                    if let Some(sort) = pm
                        .get("sort")
                        .map(|s| s.as_string_lossy())
                        .filter(|s| !s.trim().is_empty())
                    {
                        b.routing.sort = Some(sort);
                    }
                }
            }
            "sort" => {
                let s = v.as_string_lossy();
                if !s.trim().is_empty() {
                    b.routing.sort = Some(s);
                }
            }
            "require_params" => b.routing.require_params = parse_require_params(v),
            "args" => b.args = split_args(&v.as_string_lossy()),
            "min_context" => {
                b.routing.min_context = match v {
                    YamlValue::Int(n) => Some(*n),
                    YamlValue::Bool(true) => Some(ROUTING_MIN_1M_CONTEXT),
                    _ => v.as_string_lossy().parse().ok(),
                }
            }
            _ => {}
        }
    }
    b
}

/// `agents.<id>.args`: flags split on whitespace. Quoting is not supported;
/// an argument with spaces goes after `--` on the command line instead.
pub fn split_args(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(str::to_string).collect()
}

fn parse_require_params(value: &YamlValue) -> Vec<String> {
    match value {
        YamlValue::Bool(true) => vec![ROUTING_PARAM_TOOLS.to_string()],
        YamlValue::Bool(false) | YamlValue::Null => Vec::new(),
        YamlValue::Map(map) => map
            .values()
            .map(|v| v.as_string_lossy())
            .filter(|s| !s.trim().is_empty())
            .collect(),
        YamlValue::Int(_) | YamlValue::String(_) => value
            .as_string_lossy()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
    }
}

fn yaml_bool(v: &YamlValue) -> Option<bool> {
    match v {
        YamlValue::Bool(b) => Some(*b),
        YamlValue::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "on" | "yes" => Some(true),
            "false" | "0" | "off" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub fn parse_config(source: &str) -> Config {
    let root = parse_yaml_map(source);
    let mut config = Config::default();
    if let Some(s) = root.get("active_profile").and_then(|v| v.as_str()) {
        config.active_profile = s.to_string();
    }
    if let Some(s) = root.get("last_tool").and_then(|v| v.as_str()) {
        config.last_tool = Some(s.to_string());
    }
    // `update: { auto: false }`; the flat `auto_update:` older builds wrote
    // still reads.
    let nested = root
        .get("update")
        .and_then(|v| v.as_map())
        .and_then(|m| m.get("auto"));
    if let Some(v) = nested.or_else(|| root.get("auto_update")) {
        config.auto_update = yaml_bool(v);
    }
    if let Some(v) = root
        .get("mcp")
        .and_then(|v| v.as_map())
        .and_then(|m| m.get("auto"))
    {
        config.mcp_auto = yaml_bool(v);
    }
    if let Some(s) = root
        .get("channel")
        .or_else(|| root.get("update_channel"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        config.channel = Some(s.to_ascii_lowercase());
    }
    if let Some(map) = root.get("profiles").and_then(|v| v.as_map()) {
        for (name, value) in map {
            if let Some(pm) = value.as_map() {
                config.profiles.insert(name.clone(), profile_from_map(pm));
            }
        }
    }
    if let Some(map) = root.get("agents").and_then(|v| v.as_map()) {
        for (name, value) in map {
            if let Some(am) = value.as_map() {
                let id = crate::spawn::canonical_tool(name).to_string();
                let binding = agent_binding_from_map(am);
                if !binding.is_empty() {
                    config.agents.insert(id, binding);
                }
            }
        }
    }
    if let Some(map) = root.get("tools").and_then(|v| v.as_map()) {
        for (name, value) in map {
            if let Some(tm) = value.as_map() {
                let mut tool = config
                    .tools
                    .get(name)
                    .cloned()
                    .unwrap_or_else(ToolConfig::default);
                tool.apply_yaml(tm);
                config.tools.insert(name.clone(), tool);
            }
        }
    }
    config
}

pub fn serialize_config(config: &Config) -> String {
    let mut lines = vec![format!(
        "active_profile: {}",
        yaml_scalar(&config.active_profile)
    )];
    if let Some(tool) = &config.last_tool {
        lines.push(format!("last_tool: {}", yaml_scalar(tool)));
    }
    if let Some(v) = config.auto_update {
        lines.push("update:".into());
        lines.push(format!("  auto: {v}"));
    }
    if let Some(ch) = &config.channel {
        lines.push(format!("channel: {}", yaml_scalar(ch)));
    }
    if let Some(v) = config.mcp_auto {
        lines.push("mcp:".into());
        lines.push(format!("  auto: {v}"));
    }
    lines.push("profiles:".into());
    for (name, profile) in &config.profiles {
        lines.push(format!("  {name}:"));
        if let Some(k) = &profile.api_key {
            lines.push(format!("    api_key: {}", yaml_scalar(k)));
        }
        if let Some(k) = &profile.management_key {
            lines.push(format!("    management_key: {}", yaml_scalar(k)));
        }
        if let Some(u) = &profile.base_url {
            lines.push(format!("    base_url: {}", yaml_scalar(u)));
        }
        if let Some(p) = &profile.pinned_preset {
            lines.push(format!("    pinned_preset: {}", yaml_scalar(p)));
        }
        if let Some(m) = &profile.default_model {
            lines.push(format!("    default_model: {}", yaml_scalar(m)));
        }
        if let Some(t) = &profile.default_tool {
            lines.push(format!("    default_tool: {}", yaml_scalar(t)));
        }
        if let Some(m) = &profile.claude_haiku {
            lines.push(format!("    claude_haiku: {}", yaml_scalar(m)));
        }
        if let Some(m) = &profile.claude_sonnet {
            lines.push(format!("    claude_sonnet: {}", yaml_scalar(m)));
        }
        if let Some(m) = &profile.claude_opus {
            lines.push(format!("    claude_opus: {}", yaml_scalar(m)));
        }
        if let Some(m) = &profile.claude_fable {
            lines.push(format!("    claude_fable: {}", yaml_scalar(m)));
        }
        if let Some(t) = profile.timeout_ms {
            lines.push(format!("    timeout_ms: {t}"));
        }
        if let Some(k) = &profile.relay_token {
            lines.push(format!("    relay_token: {}", yaml_scalar(k)));
        }
        if let Some(id) = &profile.relay_device_id {
            lines.push(format!("    relay_device_id: {}", yaml_scalar(id)));
        }
        if let Some(n) = &profile.relay_device_name {
            lines.push(format!("    relay_device_name: {}", yaml_scalar(n)));
        }
        // Unknown/forward-compat fields (e.g. written by other CLI builds)
        // round-trip instead of being silently dropped on rewrite.
        for (key, value) in &profile.extra {
            lines.push(format!("    {key}: {}", yaml_scalar_value(value)));
        }
    }
    let agents: Vec<(&String, &AgentBinding)> = config
        .agents
        .iter()
        .filter(|(_, b)| !b.is_empty())
        .collect();
    if !agents.is_empty() {
        lines.push("agents:".into());
        for (name, binding) in agents {
            lines.push(format!("  {name}:"));
            if let Some(p) = &binding.profile {
                lines.push(format!("    profile: {}", yaml_scalar(p)));
            }
            if let Some(k) = &binding.api_key {
                lines.push(format!("    api_key: {}", yaml_scalar(k)));
            }
            if let Some(m) = &binding.default_model {
                lines.push(format!("    default_model: {}", yaml_scalar(m)));
            }
            if !binding.args.is_empty() {
                // Always quoted: a leading `-` must not read as a YAML list item.
                let joined = binding.args.join(" ");
                lines.push(format!(
                    "    args: {}",
                    serde_json::to_string(&joined).unwrap_or_default()
                ));
            }
            if let Some(sort) = binding
                .routing
                .sort
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                lines.push("    provider:".into());
                lines.push(format!("      sort: {}", yaml_scalar(sort)));
            }
            if !binding.routing.require_params.is_empty() {
                lines.push(format!(
                    "    require_params: {}",
                    yaml_scalar(&binding.routing.require_params.join(","))
                ));
            }
            if let Some(n) = binding.routing.min_context {
                lines.push(format!("    min_context: {n}"));
            }
        }
    }
    lines.push("tools:".into());
    for (name, tool) in &config.tools {
        lines.push(format!("  {name}:"));
        lines.extend(tool.to_yaml_lines());
    }
    format!("{}\n", lines.join("\n"))
}

pub fn read_config(path: &Path) -> Result<Config, String> {
    let src = fs::read_to_string(path).map_err(|_| {
        format!(
            "Missing AnyRouter config. Run: {} login (first run sets you up).",
            crate::help::invoked_bin()
        )
    })?;
    Ok(parse_config(&src))
}

/// Create `dir` (and parents) and make it owner-only (0700) on unix.
pub(crate) fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Open `path` for writing, created owner-only (0600) on unix so the file is
/// never readable by others, not even briefly. `exclusive` fails if it exists.
pub(crate) fn open_private(path: &Path, exclusive: bool) -> std::io::Result<fs::File> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true);
    if exclusive {
        opts.create_new(true);
    } else {
        opts.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// Write `body` to `path` as an owner-only file, fixing up a looser existing mode.
pub(crate) fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = open_private(path, false)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(body)
}

pub fn write_config(config: &Config, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        ensure_private_dir(dir).map_err(|e| format!("could not secure config dir: {e}"))?;
    }
    let body = serialize_config(config);
    let tmp = path.with_extension("yaml.tmp");
    // A stale tmp from a crashed run could carry a looser mode; start clean.
    let _ = fs::remove_file(&tmp);
    let written = open_private(&tmp, true).and_then(|mut f| {
        use std::io::Write;
        f.write_all(body.as_bytes())
    });
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(format!("Could not write config: {e}"));
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Could not replace config: {e}")
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honors_anyrouter_home() {
        let mut env = BTreeMap::new();
        env.insert("ANYROUTER_HOME".into(), "/tmp/xyz".into());
        assert_eq!(
            resolve_config_path(None, &env),
            PathBuf::from("/tmp/xyz/config.yaml")
        );
    }

    #[test]
    fn parse_config_roundtrip_minimal_yaml() {
        let src = "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-test
    base_url: https://anyrouter.dev/api
    pinned_preset: \"@preset/coding-stack\"
    default_model: auto
    timeout_ms: 3000000
";
        let cfg = parse_config(src);
        assert_eq!(
            cfg.profiles.get("default").unwrap().api_key.as_deref(),
            Some("sk-ar-v1-test")
        );
        let again = parse_config(&serialize_config(&cfg));
        assert_eq!(
            again.profiles.get("default").unwrap().api_key.as_deref(),
            Some("sk-ar-v1-test")
        );
    }

    #[test]
    fn serialize_keeps_management_key_and_last_tool() {
        let src = "\
active_profile: work
last_tool: claude
profiles:
  work:
    api_key: sk-ar-v1-test
    management_key: ak_mgmt
    default_tool: codex
    default_model: anthropic/claude-sonnet-4.6
";
        let cfg = parse_config(src);
        assert_eq!(cfg.last_tool.as_deref(), Some("claude"));
        let p = cfg.profiles.get("work").unwrap();
        assert_eq!(p.management_key.as_deref(), Some("ak_mgmt"));
        assert_eq!(p.default_tool.as_deref(), Some("codex"));
        let again = parse_config(&serialize_config(&cfg));
        assert_eq!(again.last_tool.as_deref(), Some("claude"));
        assert_eq!(
            again
                .profiles
                .get("work")
                .unwrap()
                .management_key
                .as_deref(),
            Some("ak_mgmt")
        );
    }

    #[test]
    fn serialize_keeps_claude_aliases() {
        let src = "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-test
    default_model: anyrouter/auto
    claude_haiku: z-ai/glm-4.7-flash
    claude_sonnet: anthropic/claude-sonnet-4.6
    claude_opus: anthropic/claude-opus-4.6
";
        let cfg = parse_config(src);
        let p = cfg.profiles.get("default").unwrap();
        assert_eq!(p.claude_haiku(), "z-ai/glm-4.7-flash");
        assert_eq!(p.claude_sonnet(), "anthropic/claude-sonnet-4.6");
        assert_eq!(p.claude_opus(), "anthropic/claude-opus-4.6");
        let again = parse_config(&serialize_config(&cfg));
        let p2 = again.profiles.get("default").unwrap();
        assert_eq!(p2.claude_haiku.as_deref(), Some("z-ai/glm-4.7-flash"));
        assert_eq!(p2.default_model.as_deref(), Some("anyrouter/auto"));
    }

    #[test]
    fn agents_section_roundtrips_per_agent_bindings() {
        let src = "\
active_profile: default
last_tool: claude
profiles:
  default:
    api_key: sk-ar-v1-default-key-aaaa
    default_model: auto
  work:
    api_key: sk-ar-v1-work-key-bbbb
    default_model: anthropic/claude-sonnet-4.6
agents:
  claude:
    profile: work
    api_key: sk-ar-v1-claude-key-cccc
    default_model: stealth/ox-alpha
  grok:
    default_model: grok-4
";
        let cfg = parse_config(src);
        let claude = cfg.agent_binding("claude").expect("claude binding");
        assert_eq!(claude.profile.as_deref(), Some("work"));
        assert_eq!(claude.api_key.as_deref(), Some("sk-ar-v1-claude-key-cccc"));
        assert_eq!(claude.default_model.as_deref(), Some("stealth/ox-alpha"));
        let grok = cfg.agent_binding("grok").expect("grok binding");
        assert_eq!(grok.profile, None);
        assert_eq!(grok.api_key, None);
        assert_eq!(grok.default_model.as_deref(), Some("grok-4"));
        assert!(cfg.agent_binding("codex").is_none());
        let yaml = serialize_config(&cfg);
        assert!(yaml.contains("agents:"), "{yaml}");
        assert!(yaml.contains("  claude:"), "{yaml}");
        assert!(
            yaml.contains("    default_model: stealth/ox-alpha"),
            "{yaml}"
        );
        let again = parse_config(&yaml);
        assert_eq!(
            again
                .agent_binding("claude")
                .and_then(|b| b.api_key.as_deref()),
            Some("sk-ar-v1-claude-key-cccc")
        );
        assert_eq!(
            again
                .agent_binding("grok")
                .and_then(|b| b.default_model.as_deref()),
            Some("grok-4")
        );
    }

    #[test]
    fn agents_section_roundtrips_routing_constraints() {
        let src = "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-test
    default_model: auto
agents:
  claude:
    default_model: anyrouter/auto
    provider:
      sort: exacto
    require_params: tools
    min_context: 1000000
  grok:
    default_model: anyrouter/free
";
        let cfg = parse_config(src);
        let claude = cfg.agent_binding("claude").expect("claude binding");
        assert!(claude.routing.wants_exacto());
        assert!(claude.routing.requires_tools());
        assert!(claude.routing.requires_1m_context());
        let body = claude.routing.extra_body_json().expect("body");
        assert!(body.contains("\"sort\":\"exacto\""), "{body}");
        assert!(body.contains("\"require_params\":[\"tools\"]"), "{body}");
        assert!(body.contains("\"min_context\":1000000"), "{body}");
        let grok = cfg.agent_binding("grok").expect("grok binding");
        assert!(grok.routing.is_empty());
        let yaml = serialize_config(&cfg);
        assert!(yaml.contains("      sort: exacto"), "{yaml}");
        assert!(yaml.contains("    require_params: tools"), "{yaml}");
        assert!(yaml.contains("    min_context: 1000000"), "{yaml}");
        let again = parse_config(&yaml);
        let claude2 = again.agent_binding("claude").expect("claude");
        assert!(claude2.routing.wants_exacto());
        assert!(claude2.routing.requires_tools());
        assert_eq!(claude2.routing.min_context, Some(1_000_000));
    }

    #[test]
    fn partial_tools_yaml_keeps_builtin_gateway_discovery() {
        // WHY: `tools.claude.command` alone must not flip discovery off.
        let cfg = parse_config(
            "\
active_profile: default
profiles:
  default:
    api_key: x
tools:
  claude:
    command: /opt/claude
",
        );
        let claude = cfg.tools.get("claude").expect("claude tool");
        assert_eq!(claude.command, "/opt/claude");
        assert!(
            claude.enable_gateway_model_discovery,
            "partial overlay wiped gateway discovery"
        );
        let yaml = serialize_config(&cfg);
        let again = parse_config(&yaml);
        assert!(
            again
                .tools
                .get("claude")
                .expect("roundtrip")
                .enable_gateway_model_discovery
        );
    }

    #[test]
    fn model_id_context_suffix_sets_min_context_floor() {
        assert_eq!(
            parse_context_window_suffix("anyrouter/auto[1m]"),
            Some(1_000_000)
        );
        assert_eq!(
            parse_context_window_suffix("anyrouter/auto[500k]"),
            Some(500_000)
        );
        assert_eq!(parse_context_window_suffix("anyrouter/auto"), None);
        let mut r = RoutingConstraints::default();
        r.apply_model_id_context_suffix("anyrouter/auto[500k]");
        r.apply_model_id_context_suffix("anyrouter/auto[1m]");
        assert_eq!(r.min_context, Some(1_000_000));
        let body = r.extra_body_json().expect("body");
        assert!(body.contains("\"min_context\":1000000"), "{body}");
        assert!(body.contains("\"provider\""), "{body}");
    }

    #[test]
    fn set_active_profile_rejects_unknown() {
        let cfg = parse_config("active_profile: default\nprofiles:\n  default:\n    api_key: x\n");
        assert!(set_active_profile(cfg, "missing").is_err());
    }

    #[test]
    fn mcp_auto_follows_the_workspace_by_default_and_roundtrips_off() {
        let missing =
            parse_config("active_profile: default\nprofiles:\n  default:\n    api_key: x\n");
        assert_eq!(missing.mcp_auto, None);
        assert!(!serialize_config(&missing).contains("mcp:"));
        let off = parse_config(
            "active_profile: default\nmcp:\n  auto: false\nprofiles:\n  default:\n    api_key: x\n",
        );
        assert_eq!(off.mcp_auto, Some(false));
        // Every launch rewrites the config, so the opt-out must survive it.
        let again = parse_config(&serialize_config(&off));
        assert_eq!(again.mcp_auto, Some(false));
        assert_eq!(again.profiles["default"].api_key.as_deref(), Some("x"));
    }

    #[test]
    fn auto_update_defaults_on_and_roundtrips_off() {
        let missing =
            parse_config("active_profile: default\nprofiles:\n  default:\n    api_key: x\n");
        assert!(missing.auto_update());
        assert_eq!(missing.auto_update, None);
        assert!(!serialize_config(&missing).contains("auto_update:"));

        let off = parse_config(
            "active_profile: default\nauto_update: false\nprofiles:\n  default:\n    api_key: x\n",
        );
        assert!(!off.auto_update());
        let again = parse_config(&serialize_config(&off));
        assert_eq!(again.auto_update, Some(false));
    }

    #[test]
    fn nested_update_auto_survives_a_rewrite() {
        // WHY: every launch rewrites the config; an opt-out it drops would
        // silently turn auto-update back on.
        let off = parse_config(
            "active_profile: default\nupdate:\n  auto: false\nprofiles:\n  default:\n    api_key: x\n",
        );
        assert_eq!(off.auto_update, Some(false));
        let written = serialize_config(&off);
        assert!(written.contains("update:\n  auto: false"), "{written}");
        assert_eq!(parse_config(&written).auto_update, Some(false));
    }

    #[test]
    fn channel_defaults_stable_and_roundtrips_beta() {
        let missing =
            parse_config("active_profile: default\nprofiles:\n  default:\n    api_key: x\n");
        assert_eq!(missing.channel(), "stable");
        assert_eq!(missing.channel, None);
        assert!(!serialize_config(&missing).contains("channel:"));

        let beta = parse_config(
            "active_profile: default\nchannel: beta\nprofiles:\n  default:\n    api_key: x\n",
        );
        assert_eq!(beta.channel(), "beta");
        let again = parse_config(&serialize_config(&beta));
        assert_eq!(again.channel.as_deref(), Some("beta"));

        let alias = parse_config(
            "active_profile: default\nupdate_channel: BETA\nprofiles:\n  default:\n    api_key: x\n",
        );
        assert_eq!(alias.channel(), "beta");
    }

    #[test]
    fn write_config_sets_owner_only_permissions_on_unix() {
        let dir = std::env::temp_dir().join(format!(
            "anyr-cfg-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("config.yaml");
        let cfg = parse_config(
            "active_profile: default\nprofiles:\n  default:\n    api_key: sk-ar-v1-test\n",
        );
        write_config(&cfg, &path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "file mode {mode:o}");
            let dmode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(dmode & 0o777, 0o700, "dir mode {dmode:o}");
        }
        // Content survived intact.
        assert_eq!(
            read_config(&path)
                .unwrap()
                .profiles
                .get("default")
                .unwrap()
                .api_key
                .as_deref(),
            Some("sk-ar-v1-test")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // WHY: chmod-after-write leaves a window where the key is world-readable;
    // a stale tmp with a loose mode must not leak into the final config either.
    #[cfg(unix)]
    #[test]
    fn write_config_ignores_loose_stale_tmp() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("anyr-cfg-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        let tmp = dir.join("config.yaml.tmp");
        std::fs::write(&tmp, "stale").unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o666)).unwrap();
        let cfg = parse_config("active_profile: default\nprofiles:\n  default:\n    api_key: x\n");
        write_config(&cfg, &path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("stale"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_config_leaves_no_tmp_file_behind() {
        let dir = std::env::temp_dir().join(format!(
            "anyr-cfg-tmp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("config.yaml");
        let cfg = parse_config("active_profile: default\nprofiles:\n  default:\n    api_key: x\n");
        write_config(&cfg, &path).unwrap();
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["config.yaml".to_string()], "{entries:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebind_agents_follows_a_rename_and_drops_a_removed_account() {
        let mut cfg = parse_config(
            "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-default-key-aaaa
  work:
    api_key: sk-ar-v1-work-key-bbbb
agents:
  claude:
    profile: work
  codex:
    profile: work
    default_model: openai/gpt-5.5
  grok:
    profile: default
",
        );
        assert_eq!(
            cfg.rebind_agents("work", Some("job")),
            vec!["claude".to_string(), "codex".to_string()]
        );
        assert_eq!(
            cfg.agent_binding("claude").unwrap().profile.as_deref(),
            Some("job")
        );
        assert_eq!(cfg.rebind_agents("job", None).len(), 2);
        assert!(
            cfg.agent_binding("claude").is_none(),
            "empty binding pruned"
        );
        let codex = cfg.agent_binding("codex").expect("model binding kept");
        assert_eq!(codex.profile, None);
        assert_eq!(codex.default_model.as_deref(), Some("openai/gpt-5.5"));
        assert_eq!(
            cfg.agent_binding("grok").unwrap().profile.as_deref(),
            Some("default")
        );
    }

    #[test]
    fn agent_binding_mut_does_not_clobber_sibling() {
        let mut cfg = parse_config(
            "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-test
    default_model: auto
",
        );
        cfg.agent_binding_mut("claude").default_model = Some("stealth/ox-alpha".into());
        cfg.agent_binding_mut("grok").default_model = Some("grok-4".into());
        cfg.agent_binding_mut("claude").default_model = Some("anthropic/claude-sonnet-4.6".into());
        assert_eq!(
            cfg.agent_binding("claude")
                .and_then(|b| b.default_model.as_deref()),
            Some("anthropic/claude-sonnet-4.6")
        );
        assert_eq!(
            cfg.agent_binding("grok")
                .and_then(|b| b.default_model.as_deref()),
            Some("grok-4")
        );
    }
}
