use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::config::{resolve_config_path, write_config};
use crate::http::fetch_credits;
use crate::install::available_agents;
use crate::key::load_config_if_present;
use crate::parse::{get_string_flag, FlagValue, ParsedArgs};
use crate::spawn::{canonical_tool, resolve_tool};

#[cfg(not(feature = "native"))]
use crate::term;

#[cfg(feature = "native")]
pub(crate) fn tui_wants_dump(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> bool {
    crate::tui::wants_dump(parsed, env)
}

#[cfg(not(feature = "native"))]
pub(crate) fn tui_wants_dump(_parsed: &ParsedArgs, _env: &BTreeMap<String, String>) -> bool {
    false
}

#[cfg(not(feature = "native"))]
pub(crate) fn tui_menu_select(
    title: &str,
    _header: Vec<String>,
    items: Vec<String>,
) -> Result<Option<usize>, String> {
    match term::pick(title, &items, Some(0)) {
        Ok(i) => Ok(Some(i)),
        Err(_) => Ok(None),
    }
}

#[cfg(not(feature = "native"))]
#[derive(Clone)]
pub(crate) struct InlineEntry {
    pub label: String,
    pub detail: String,
    pub group: String,
    pub action: String,
}

#[cfg(not(feature = "native"))]
impl InlineEntry {
    pub(crate) fn new(
        label: impl Into<String>,
        detail: impl Into<String>,
        _group: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            group: String::new(),
            action: action.into(),
        }
    }
}

#[cfg(feature = "native")]
pub(crate) fn tui_palette_select(
    header: Vec<String>,
    entries: Vec<crate::tui::PaletteEntry>,
    on_idle: impl FnMut(&mut crate::tui::PaletteState),
) -> Result<Option<usize>, String> {
    crate::tui::run_palette_select_idle(header, entries, on_idle)
}

#[cfg(feature = "native")]
pub(crate) fn pick_palette_action(
    header: Vec<String>,
    entries: Vec<crate::tui::PaletteEntry>,
    cache: &Arc<Mutex<CreditsCache>>,
) -> Result<Option<usize>, String> {
    let tick_cache = cache.clone();
    let mut seen = 0u64;
    tui_palette_select(header, entries, move |state| {
        let Ok(credits) = tick_cache.lock() else {
            return;
        };
        if credits.gen <= seen {
            return;
        }
        seen = credits.gen;
        patch_palette_header(state, &credits);
    })
}

#[cfg(not(feature = "native"))]
pub(crate) fn pick_palette_action(
    header: Vec<String>,
    entries: Vec<InlineEntry>,
    _cache: &Arc<Mutex<CreditsCache>>,
) -> Result<Option<usize>, String> {
    tui_palette_select(header, entries)
}

#[cfg(not(feature = "native"))]
pub(crate) fn tui_palette_select(
    _header: Vec<String>,
    entries: Vec<InlineEntry>,
) -> Result<Option<usize>, String> {
    let items: Vec<String> = entries
        .iter()
        .map(|e| {
            if e.detail.is_empty() {
                e.label.clone()
            } else {
                format!("{}  —  {}", e.label, e.detail)
            }
        })
        .collect();
    match term::pick("anyr", &items, Some(0)) {
        Ok(i) => Ok(Some(i)),
        Err(_) => Ok(None),
    }
}

#[cfg(feature = "native")]
pub(crate) fn launcher_uses_palette(env: &BTreeMap<String, String>) -> bool {
    crate::tui::bool_env("ANYR_TUI", env, crate::tui::can_use_fullscreen())
}

#[cfg(not(feature = "native"))]
pub(crate) fn launcher_uses_palette(_env: &BTreeMap<String, String>) -> bool {
    false
}

#[cfg(feature = "native")]
pub(crate) fn tui_dump_palette(
    entries: Vec<crate::tui::PaletteEntry>,
    header: Vec<String>,
    env: &BTreeMap<String, String>,
) -> String {
    crate::tui::dump_palette(
        &crate::tui::PaletteState::new(header, entries),
        crate::tui::dump_cols(env),
    )
}

#[cfg(not(feature = "native"))]
pub(crate) fn tui_dump_palette(
    entries: Vec<InlineEntry>,
    _header: Vec<String>,
    _env: &BTreeMap<String, String>,
) -> String {
    let mut lines = vec!["▲ anyr".to_string()];
    lines.extend(entries.iter().map(|e| format!("  {}", e.label)));
    lines.push(String::new());
    lines.join("\n")
}

#[cfg(feature = "native")]
pub(crate) fn tui_settings_select(
    state: crate::tui::SettingsState,
) -> Result<Option<crate::tui::SettingsOutcome>, String> {
    if !crate::tui::is_interactive() {
        return Ok(None);
    }
    match crate::tui::run_settings_live(state)? {
        crate::tui::SettingsOutcome::Close | crate::tui::SettingsOutcome::Stay => Ok(None),
        outcome => Ok(Some(outcome)),
    }
}

#[cfg(feature = "native")]
pub(crate) fn tui_dump_settings(
    state: crate::tui::SettingsState,
    env: &BTreeMap<String, String>,
) -> String {
    crate::tui::dump_settings(&state, crate::tui::dump_cols(env))
}

/// The flags `anyr <agent>` keeps for itself. Every other flag goes to the
/// agent unchanged and in order; `--` sends the rest raw, so an agent flag
/// spelled like one of these can still reach it. `--json` is anyr's own
/// output flag (never sent to the agent; `-- --json` reaches it).
pub(crate) const LAUNCH_FLAGS: &[&str] = &[
    "model",
    "effort",
    "profile",
    "preset",
    "key",
    "base-url",
    "command-path",
    "config",
    "timeout",
    "plaintext",
    "dry-run",
    "json",
    "yes",
    "ok",
    "device",
    "device-code",
    "paste",
    "install",
    "yolo",
    "skills-off",
    "tools-off",
    "no-mcp",
];

/// Claude Code also takes the `/model` alias flags and hubs.
const CLAUDE_LAUNCH_FLAGS: &[&str] = &[
    "model",
    "effort",
    "haiku",
    "sonnet",
    "opus",
    "fable",
    "profile",
    "preset",
    "key",
    "base-url",
    "command-path",
    "claude-path",
    "config",
    "timeout",
    "plaintext",
    "dry-run",
    "json",
    "yes",
    "ok",
    "device",
    "device-code",
    "paste",
    "hub",
    "install",
    "yolo",
    "skills-off",
    "tools-off",
    "no-mcp",
];

/// `anyr pool` launches Poolside or, with a subcommand, reads pool health (`--json`).
pub(crate) const POOL_FLAGS: &[&str] = &[
    "model",
    "effort",
    "profile",
    "preset",
    "key",
    "base-url",
    "command-path",
    "config",
    "timeout",
    "plaintext",
    "dry-run",
    "yes",
    "ok",
    "device",
    "device-code",
    "paste",
    "install",
    "yolo",
    "skills-off",
    "tools-off",
    "no-mcp",
    "json",
];

/// Flags anyr owns on a launcher, or None when `command` is not one.
pub(crate) fn launch_flags(command: &str) -> Option<&'static [&'static str]> {
    Some(match canonical_command(command) {
        "claude" => CLAUDE_LAUNCH_FLAGS,
        "pool" => POOL_FLAGS,
        c if crate::registry::LAUNCHERS.contains(&c) => LAUNCH_FLAGS,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CmdKind {
    Implemented,
    Stub,
    HelpOnly,
}

pub(crate) fn cmd_kind(command: &str) -> Option<CmdKind> {
    let c = canonical_command(command);
    Some(match c {
        "setup" | "login" | "auth" | "agents" | "menu" | "models" | "config" | "keys"
        | "whoami" | "status" | "logout" | "account" | "usage" | "claude" | "codex" | "grok"
        | "opencode" | "pool" | "pi" | "qwen" | "goose" | "aider" | "crush" | "kilo" | "fx"
        | "upgrade" | "onboard" | "impl" | "plan" | "fix" | "deploy" | "cp" | "relay"
        | "decision" | "commands" | "api" | "completion" | "byok" | "publish" | "apps"
        | "artifact" | "chat" | "logs" | "transactions" | "credits" | "stats" | "embed"
        | "generation" | "presets" | "mcp" | "eval" | "task" | "delegate" | "schema"
        | "telemetry" | "report" => CmdKind::Implemented,
        "cursor" | "cline" | "windsurf" => CmdKind::HelpOnly,
        "audit" | "skills" | "prompt" => CmdKind::Stub,
        _ => return None,
    })
}

pub(crate) fn known_command(command: &str) -> bool {
    cmd_kind(command).is_some()
}

pub(crate) fn canonical_command(command: &str) -> &str {
    match command {
        "update" => "upgrade",
        "implement" => "impl",
        "decisions" | "systemone" => "decision",
        "model" => "models",
        "gen" => "generation",
        other => canonical_tool(other),
    }
}

/// Public command surface (name, one-line description). Drives shell
/// completion; a test keeps it in sync with `cmd_kind`.
pub(crate) const COMMANDS: &[(&str, &str)] = &[
    ("login", "Sign in to AnyRouter"),
    ("logout", "Remove the stored key"),
    ("whoami", "Show the signed-in account"),
    ("status", "Alias of whoami"),
    ("claude", "Launch Claude Code"),
    ("cc", "Alias of claude"),
    ("codex", "Launch Codex"),
    ("grok", "Launch Grok Build"),
    ("opencode", "Launch OpenCode"),
    ("pi", "Launch Pi"),
    ("pool", "Launch Poolside"),
    ("poolside", "Alias of pool"),
    ("qwen", "Launch Qwen Code"),
    ("goose", "Launch Goose"),
    ("aider", "Launch Aider"),
    ("crush", "Launch Crush"),
    ("kilo", "Launch Kilo Code"),
    ("fx", "Launch fx"),
    ("api", "Dashboard from the terminal; raw calls like gh api"),
    ("publish", "Publish a JSON document or a page to an app"),
    ("apps", "Manage what you published to apps"),
    ("decision", "Structured decisions (Decisions API)"),
    ("models", "List catalog and set the default"),
    ("model", "Alias of models"),
    ("usage", "Credits remaining"),
    ("credits", "Credit balance and history"),
    ("stats", "Usage analytics; --json is the API's own JSON"),
    ("logs", "Recent requests, failures and sessions"),
    ("generation", "Cost, provider and timing of one call"),
    ("gen", "Alias of generation"),
    ("presets", "Saved routing presets"),
    ("mcp", "Wire the MCP gateway into an agent"),
    ("transactions", "Credit ledger (purchases, usage, refunds)"),
    (
        "chat",
        "Ask a model: prompt, stdin or a REPL; --json for scripts",
    ),
    ("embed", "Embed text (args, --file or stdin)"),
    ("eval", "Compare one prompt across models"),
    ("task", "Plan with one model, implement with another"),
    ("delegate", "Hand a task to another coding agent"),
    ("keys", "Manage API keys"),
    (
        "byok",
        "Provider keys (BYOK); --donate shares one with the pool",
    ),
    ("artifact", "Publish a file or folder to a shareable URL"),
    ("agents", "Per-agent model, account, key and args"),
    ("account", "Manage multiple accounts"),
    ("auth", "Authenticate with AnyRouter"),
    ("config", "Print current settings"),
    (
        "menu",
        "Launcher HUD (default with no command on a terminal)",
    ),
    ("onboard", "Paste-ready prompts for coding agents"),
    ("impl", "Onboard: implement"),
    ("plan", "Onboard: plan"),
    ("fix", "Onboard: fix"),
    ("deploy", "Onboard: deploy"),
    ("cp", "Onboard: commit and push"),
    (
        "relay",
        "Serve this machine's local models through AnyRouter",
    ),
    ("upgrade", "Update anyr"),
    ("update", "Alias of upgrade"),
    ("completion", "Shell completion script"),
    ("commands", "Full command map"),
    ("schema", "Typed command map for agents (JSON)"),
    ("telemetry", "Anonymous usage pings: status, on, off, reset"),
    ("report", "Redacted bug report; --file opens a GitHub issue"),
    ("help", "Show help"),
];

pub(crate) fn canonical_name(command: &str) -> &str {
    canonical_command(command)
}

/// Known values for a value-taking flag (completion only; never networked).
pub(crate) fn value_choices(flag: &str, env: &BTreeMap<String, String>) -> Vec<String> {
    let owned = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect();
    match flag {
        "effort" => owned(crate::spawn::REASONING_LEVELS),
        "agent" | "tool" | "to" | "from" => owned(crate::registry::LAUNCHERS),
        "channel" => owned(&["stable", "beta"]),
        "model" | "haiku" | "sonnet" | "opus" | "fable" => owned(&["auto"]),
        "profile" => {
            crate::key::load_config_if_present(&crate::config::resolve_config_path(None, env))
                .map(|c| c.profiles.keys().cloned().collect())
                .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// Levenshtein distance in chars.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Closest known command for a typo (edit distance ≤ 2, or a unique prefix).
pub(crate) fn suggest_command(input: &str) -> Option<&'static str> {
    let dist = edit_distance;
    let names = COMMANDS.iter().map(|(n, _)| *n);
    let mut prefix = names
        .clone()
        .filter(|n| n.starts_with(input) && input.len() >= 2);
    if let (Some(only), None) = (prefix.next(), prefix.next()) {
        return Some(only);
    }
    names
        .map(|n| (dist(input, n), n))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, n)| n)
}

pub(crate) fn allowed_flags(command: &str) -> Option<&'static [&'static str]> {
    let canonical = canonical_command(command);
    Some(match canonical {
        "login" | "setup" => &[
            "key",
            "key-stdin",
            "preset",
            "profile",
            "base-url",
            "timeout",
            "config",
            "plaintext",
            "device",
            "device-code",
            "paste",
            "yes",
        ],
        "models" => &[
            "profile", "config", "json", "key", "base-url", "pick", "haiku", "sonnet", "opus",
            "fable", "agent", "dump-tui", "free", "type", "owner", "search", "sort",
        ],
        "usage" => &["profile", "base-url", "config", "json", "key", "no-detail"],
        "credits" => &[
            "profile",
            "base-url",
            "config",
            "json",
            "key",
            "no-detail",
            "limit",
            "type",
        ],
        "decision" => &[
            "model",
            "state",
            "questions",
            "stdin",
            "json",
            "key",
            "profile",
            "base-url",
            "config",
        ],
        "whoami" => &["profile", "config", "json", "key", "base-url", "verbose"],
        "config" => &[
            "config", "json", "key", "base-url", "profile", "dump-tui", "pick", "verbose",
        ],
        "account" => &[
            "yes",
            "key-stdin",
            "config",
            "profile",
            "key",
            "preset",
            "base-url",
            "timeout",
            "plaintext",
            "device",
            "device-code",
            "paste",
            "agent",
            "clear",
        ],
        "logs" => &[
            "profile", "base-url", "config", "json", "key", "limit", "model", "status",
        ],
        "stats" => &[
            "profile", "base-url", "config", "json", "key", "period", "from", "to", "days",
            "limit", "range",
        ],
        "generation" => &["profile", "base-url", "config", "json", "key"],
        "mcp" => &[
            "profile", "base-url", "config", "json", "key", "print", "show-key",
        ],
        "presets" => &[
            "profile",
            "base-url",
            "config",
            "json",
            "key",
            "model",
            "fallback",
            "sort",
            "name",
            "description",
            "yes",
            "if-exists",
        ],
        "transactions" => &[
            "profile", "base-url", "config", "json", "key", "limit", "type",
        ],
        "embed" => &[
            "model",
            "dimensions",
            "file",
            "json",
            "key",
            "profile",
            "base-url",
            "config",
        ],
        "eval" => &[
            "model",
            "file",
            "prompts",
            "max-tokens",
            "concurrency",
            "full",
            "json",
            "key",
            "profile",
            "base-url",
            "config",
        ],
        "chat" => &[
            "model",
            "system",
            "key",
            "profile",
            "base-url",
            "config",
            "json",
            "no-stream",
        ],
        "skills" => &["profile", "config", "hub", "json", "dry-run", "source"],
        "relay" => &[
            "target",
            "token",
            "url",
            "verbose",
            "name",
            "pool",
            "no-pool",
            "no-launch",
            "model",
            "all",
            "max-concurrency",
            "detach",
            "json",
            "quiet",
            "follow",
            "lines",
            "yes",
            "devices",
            "profile",
            "config",
            "key",
            "base-url",
        ],
        "task" => &[
            "plan-model",
            "do-model",
            "profile",
            "base-url",
            "config",
            "key",
            "yes",
            "json",
        ],
        "delegate" => &[
            "to",
            "from",
            "model",
            "profile",
            "base-url",
            "config",
            "key",
            "yes",
            "dry-run",
            "command-path",
        ],
        "keys" => &[
            "profile",
            "base-url",
            "config",
            "json",
            "yes",
            "agent",
            "key",
            "limit",
            "reset",
            "endpoints",
            "expires",
            "quiet",
            "disable",
            "enable",
            "name",
            "if-missing",
            "if-exists",
        ],
        "audit" => &["profile", "config", "json", "launches", "tool", "limit"],
        "logout" => &["profile", "config", "json"],
        "agents" => &[
            "config",
            "json",
            "model",
            "profile",
            "key-stdin",
            "args",
            "key",
            "all",
            "yes",
        ],
        "auth" => &[
            "key",
            "key-stdin",
            "preset",
            "profile",
            "base-url",
            "timeout",
            "config",
            "plaintext",
            "device",
            "device-code",
            "paste",
            "yes",
            "json",
            "masked",
            "verbose",
        ],
        "prompt" => &["base-url", "json"],
        "menu" => &["dump-tui", "config", "profile", "key", "base-url"],
        "api" => crate::api::FLAGS,
        "publish" => crate::publish::PUBLISH_FLAGS,
        "apps" => crate::publish::APPS_FLAGS,
        "completion" | "schema" => &[],
        "telemetry" => &["json", "dry-run"],
        "report" => &["json", "last", "title", "file", "yes"],
        "onboard" | "impl" | "plan" | "fix" | "deploy" | "cp" => &["json", "copy"],
        "upgrade" => &[
            "check", "channel", "fixture", "dry-run", "yes", "auto", "force", "beta", "stable",
            "status", "rollback", "json",
        ],
        c if crate::registry::LAUNCHERS.contains(&c) => return launch_flags(c),
        "cursor" | "cline" | "windsurf" => &["profile", "key", "base-url", "config", "yes"],
        "byok" => &[
            "profile",
            "config",
            "base-url",
            "json",
            "yes",
            "donate",
            "key-stdin",
            "if-exists",
        ],
        "artifact" => crate::artifact::FLAGS,
        _ => return None,
    })
}

pub(crate) fn assert_known_flags(
    command: &str,
    flags: &HashMap<String, FlagValue>,
    allowed: &[&str],
) -> Result<(), String> {
    for name in flags.keys() {
        // Global on every command: --help, --json (errors as JSON even where
        // the output has no JSON form) and --quiet.
        if matches!(name.as_str(), "help" | "json" | "quiet") {
            continue;
        }
        if !allowed.contains(&name.as_str()) {
            return Err(format!("Unknown flag --{name} for \"{command}\"."));
        }
    }
    Ok(())
}

/// Usage errors carry this prefix so `usage_exit` exits 2 instead of 1.
pub(crate) const USAGE: &str = "\u{0}usage\u{0}";

/// Map a `USAGE`-prefixed error to exit 2 (printed without the prefix).
pub(crate) fn usage_exit(result: Result<i32, String>) -> Result<i32, String> {
    match result {
        Err(e) if e.starts_with(USAGE) => {
            crate::exit::print(crate::exit::USAGE, &e[USAGE.len()..]);
            Ok(crate::exit::USAGE)
        }
        other => other,
    }
}

pub(crate) fn wants_help(parsed: &ParsedArgs) -> bool {
    parsed.flag_true("help")
        || parsed
            .passthrough
            .iter()
            .any(|a| a == "-h" || a == "--help")
}

pub(crate) fn help_topic(parsed: &ParsedArgs) -> String {
    match parsed.command.as_str() {
        "auth" => parsed
            .passthrough
            .iter()
            .find(|s| *s != "-h" && *s != "--help")
            .cloned()
            .unwrap_or_else(|| "auth".into()),
        "model" => "models".into(),
        "gen" => "generation".into(),
        other => other.to_string(),
    }
}

pub(crate) fn shift_passthrough(parsed: &ParsedArgs) -> ParsedArgs {
    let mut next = parsed.clone();
    if !next.passthrough.is_empty() {
        next.passthrough.remove(0);
        next.raw_from = next.raw_from.map(|r| r.saturating_sub(1));
    }
    next
}

/// Every caller passes a `Usage: ...` line, so it is a usage error (exit 2).
pub(crate) fn hint(template: &str) -> String {
    format!(
        "{USAGE}{}",
        template.replace("{bin}", &crate::help::invoked_bin())
    )
}

pub(crate) fn stub(command: &str) -> Result<i32, String> {
    Err(format!(
        "\"{command}\" is not yet implemented in the native CLI. Run \"{} --help\" for available commands, or \"{} onboard\" for agent paste prompts.",
        crate::help::invoked_bin(),
        crate::help::invoked_bin(),
    ))
}

pub(crate) fn config_path(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> PathBuf {
    resolve_config_path(get_string_flag(&parsed.flags, "config").as_deref(), env)
}

pub(crate) fn tool_command_for(path: &Path, id: &str) -> String {
    let cfg = load_config_if_present(path);
    resolve_tool(cfg.as_ref(), id)
        .map(|t| t.command)
        .unwrap_or_else(|_| id.to_string())
}

pub(crate) fn catalog_lookup_enabled(env: &BTreeMap<String, String>) -> bool {
    !matches!(
        env.get("ANYR_NO_CATALOG").map(|s| s.as_str()),
        Some("1" | "true" | "TRUE" | "yes")
    )
}

pub(crate) fn persist_tool_command(path: &Path, id: &str, command: &str) -> Result<(), String> {
    let builtin = resolve_tool(None, id)
        .map(|t| t.command)
        .unwrap_or_else(|_| id.to_string());
    if !crate::install::should_persist_command(command, &builtin) {
        return Ok(());
    }
    let mut cfg = load_config_if_present(path).unwrap_or_default();
    let mut tool = resolve_tool(Some(&cfg), id)?;
    tool.command = command.to_string();
    cfg.tools.insert(id.to_string(), tool);
    write_config(&cfg, path)
}

pub(crate) fn launcher_last_tool(
    path: &Path,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> String {
    let cfg = load_config_if_present(path).unwrap_or_default();
    cfg.preferred_agent(get_string_flag(&parsed.flags, "tool").as_deref())
        .unwrap_or_else(|| {
            available_agents(env, |id| tool_command_for(path, id))
                .first()
                .map(|(id, _)| (*id).to_string())
                .unwrap_or_else(|| "claude".into())
        })
}

pub(crate) struct CreditsCache {
    value: Option<Result<String, ()>>,
    me: Option<Result<crate::http::MeInfo, ()>>,
    fetched_at: Option<std::time::Instant>,
    gen: u64,
}

pub(crate) const CREDITS_TTL: std::time::Duration = std::time::Duration::from_secs(300);

impl CreditsCache {
    pub(crate) fn fresh() -> Self {
        Self {
            value: None,
            me: None,
            fetched_at: None,
            gen: 0,
        }
    }

    pub(crate) fn peek_credits(&self) -> String {
        match &self.value {
            Some(Ok(s)) => s.clone(),
            Some(Err(())) => "(unknown)".into(),
            None => "-".into(),
        }
    }

    pub(crate) fn peek_identity(&self) -> Option<crate::http::MeInfo> {
        match &self.me {
            Some(Ok(me)) => Some(me.clone()),
            _ => None,
        }
    }

    /// Refresh both credits and identity when stale.
    pub(crate) fn refresh(&mut self, base_url: &str, api_key: Option<&str>) {
        let expired = self
            .fetched_at
            .map(|t| t.elapsed() > CREDITS_TTL)
            .unwrap_or(true);
        if !expired {
            return;
        }
        match api_key {
            Some(key) => {
                // /v1/me carries email + username + balance in one call; fall
                // back to /v1/credits when it is unavailable (older gateway).
                self.me = Some(crate::http::fetch_me(base_url, key).map_err(|_| ()));
                let from_me = match &self.me {
                    Some(Ok(me)) => me.balance.map(crate::http::format_usd),
                    _ => None,
                };
                self.value = Some(match from_me {
                    Some(s) => Ok(s),
                    None => fetch_credits(base_url, key)
                        .map(|c| crate::http::format_usd(c["balance"].as_f64().unwrap_or(0.0)))
                        .map_err(|_| ()),
                });
            }
            None => {
                self.me = Some(Err(()));
                self.value = Some(Err(()));
            }
        }
        self.fetched_at = Some(std::time::Instant::now());
        self.gen = self.gen.saturating_add(1);
    }

    /// Return the cached credits display string, refreshing when stale.
    pub(crate) fn get(&mut self, base_url: &str, api_key: Option<&str>) -> String {
        self.refresh(base_url, api_key);
        self.peek_credits()
    }

    /// Cached identity, refreshing when stale. `None` when unknown.
    pub(crate) fn identity(
        &mut self,
        base_url: &str,
        api_key: Option<&str>,
    ) -> Option<crate::http::MeInfo> {
        self.refresh(base_url, api_key);
        self.peek_identity()
    }
}

#[cfg(feature = "native")]
pub(crate) fn kick_credits_refresh(
    cache: &Arc<Mutex<CreditsCache>>,
    base: String,
    key: Option<String>,
) {
    let Some(key) = key.filter(|s| !s.is_empty()) else {
        return;
    };
    let cache = cache.clone();
    let _ = std::thread::Builder::new()
        .name("anyr-credits".into())
        .spawn(move || {
            let mut tmp = CreditsCache::fresh();
            tmp.refresh(&base, Some(&key));
            if let Ok(mut slot) = cache.lock() {
                *slot = tmp;
            }
        });
}

#[cfg(feature = "native")]
pub(crate) fn patch_palette_header(state: &mut crate::tui::PaletteState, credits: &CreditsCache) {
    if let Some(me) = credits.peek_identity() {
        if let Some(line) = state.header.first_mut() {
            *line = format!("account  {}", me.display_label());
        }
    }
    let shown = credits.peek_credits();
    if let Some(line) = state.header.iter_mut().find(|l| l.starts_with("credits")) {
        *line = format!("credits  {shown}");
    }
}

pub(crate) fn should_open_launcher(raw: &[String], interactive: bool, dump: bool) -> bool {
    raw.is_empty() && (interactive || dump)
}

#[cfg(test)]
mod tests {
    use super::should_open_launcher;

    #[test]
    fn persist_tool_command_skips_bare_builtin() {
        assert!(!crate::install::should_persist_command("claude", "claude"));
        assert!(crate::install::should_persist_command(
            "/opt/claude",
            "claude"
        ));
    }

    #[test]
    fn bare_tty_opens_launcher_not_help() {
        assert!(should_open_launcher(&[], true, false));
        assert!(!should_open_launcher(&[], false, false));
        assert!(should_open_launcher(&[], false, true));
        assert!(!should_open_launcher(&["help".into()], true, false));
        assert!(!should_open_launcher(&["--help".into()], true, false));
        assert!(!should_open_launcher(&["config".into()], true, false));
    }

    #[test]
    fn completion_table_matches_dispatch() {
        // WHY: completion is generated from COMMANDS; a command added to
        // dispatch but not the table would silently never complete.
        use super::{cmd_kind, CmdKind, COMMANDS};
        for (name, _) in COMMANDS {
            assert!(
                *name == "help" || cmd_kind(name).is_some(),
                "{name} is not dispatchable"
            );
        }
        let implemented = [
            "login",
            "logout",
            "whoami",
            "claude",
            "codex",
            "grok",
            "opencode",
            "pi",
            "pool",
            "api",
            "publish",
            "apps",
            "decision",
            "models",
            "usage",
            "keys",
            "byok",
            "account",
            "auth",
            "config",
            "menu",
            "onboard",
            "relay",
            "upgrade",
            "completion",
            "commands",
        ];
        for name in implemented {
            assert_eq!(cmd_kind(name), Some(CmdKind::Implemented), "{name}");
            assert!(
                COMMANDS.iter().any(|(n, _)| *n == name),
                "{name} missing from COMMANDS"
            );
        }
    }

    #[test]
    fn typo_suggests_nearest_command() {
        assert_eq!(super::suggest_command("cluade"), Some("claude"));
        assert_eq!(super::suggest_command("logn"), Some("login"));
        assert_eq!(super::suggest_command("comp"), Some("completion"));
        assert_eq!(super::suggest_command("zzzzzz"), None);
    }
}
