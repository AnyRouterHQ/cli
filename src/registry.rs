//! The command tree as data: `anyr commands --json` / `anyr help --json`, and
//! the agent guide behind `anyr help agents`.
//!
//! Built from `dispatch::COMMANDS` (names and summaries), `canonical_command`
//! (aliases) and `allowed_flags` (flags), so it cannot drift from what the
//! parser accepts. Subcommands and groups live here; a test checks every
//! subcommand against the command's own `--help`.

use serde_json::{json, Value};

use crate::cmd::dispatch::{allowed_flags, COMMANDS};

/// `canonical_command` plus the aliases only dispatch knows (`status`).
fn canonical_command(name: &str) -> &str {
    match name {
        "status" => "whoami",
        other => crate::cmd::dispatch::canonical_command(other),
    }
}

/// Bumped only on a breaking change to the shape below. Fields may be added.
pub const SCHEMA: &str = "anyr.commands/v1";

/// The everyday commands the short `anyr --help` shows, in order.
pub const EVERYDAY: &[&str] = &[
    "login", "chat", "models", "claude", "relay", "artifact", "usage",
];

/// Agents `anyr <agent>` launches.
pub const LAUNCHERS: &[&str] = &[
    "claude", "codex", "grok", "opencode", "pi", "pool", "qwen", "goose", "aider", "crush", "kilo",
    "fx",
];

/// Flags every command accepts.
pub const GLOBAL_FLAGS: &[(&str, &str)] = &[
    (
        "json",
        "JSON on stdout where the command has a JSON form; errors as one JSON object on stderr",
    ),
    ("quiet", "No update notices or background update check"),
    ("help", "Help for this command"),
];

pub const ENV: &[(&str, &str)] = &[
    (
        "ANYROUTER_API_KEY",
        "Inference key (sk-ar-v1-…); wins over the saved login",
    ),
    (
        "ANYROUTER_MANAGEMENT_KEY",
        "Management key (ak_…) for presets, byok and device removal",
    ),
    ("ANYROUTER_PROFILE", "Use another saved account"),
    ("ANYR_OUTPUT", "json: same as --json on every command"),
    ("ANYR_QUIET", "1: same as --quiet"),
    ("NO_COLOR", "Plain text, no ANSI colour"),
];

pub fn group(canonical: &str) -> &'static str {
    match canonical {
        "login" | "logout" | "auth" | "whoami" | "account" => "auth",
        "chat" | "models" | "embed" | "eval" | "decision" | "task" | "delegate" => "chat",
        c if LAUNCHERS.contains(&c) => "launch",
        "relay" => "relay",
        "artifact" | "publish" | "apps" => "publish",
        "usage" | "keys" | "byok" | "presets" | "logs" | "generation" | "transactions" | "mcp"
        | "api" => "account",
        _ => "setup",
    }
}

/// Group titles, in the order `anyr help --all` prints them.
/// Titles are plain words: `help --all` prints them in capitals as headings.
pub const GROUPS: &[(&str, &str)] = &[
    ("auth", "Sign in"),
    ("chat", "Chat and models"),
    ("launch", "Launch"),
    ("relay", "Relay"),
    ("publish", "Publish"),
    ("account", "Account"),
    ("setup", "Setup and help"),
];

pub fn subcommands(canonical: &str) -> &'static [&'static str] {
    match canonical {
        "auth" => &["login", "logout", "status", "switch", "token"],
        "account" => &["list", "use", "add", "rename", "remove"],
        "models" => &["list", "show", "use"],
        "keys" => &[
            "list", "create", "set", "unset", "show", "update", "audit", "revoke",
        ],
        "byok" => &["list", "add", "rm", "test", "providers"],
        "presets" => &["list", "show", "create", "edit", "rm"],
        "mcp" => &["url", "add", "connections"],
        "logs" => &["sessions"],
        "config" => &["get", "path", "use"],
        "relay" => &[
            "start", "stop", "status", "logs", "models", "devices", "pool", "pair", "rename",
            "unpair", "doctor",
        ],
        "artifact" => &["publish", "ls", "rm", "visibility"],
        "apps" => &["schema", "ls", "rm", "visibility"],
        "pool" => crate::cmd::pool::SUBCOMMANDS,
        "api" => &[
            "me",
            "credits",
            "keys",
            "logs",
            "presets",
            "aliases",
            "models",
            "providers",
            "dashboard",
            "hubs",
            "connections",
        ],
        "onboard" => &["impl", "plan", "fix", "deploy", "cp"],
        "completion" => crate::completion::SHELLS,
        "telemetry" => &["status", "on", "off", "reset"],
        _ => &[],
    }
}

/// Does `--json` print JSON on stdout (not only JSON errors)?
pub fn has_json_output(canonical: &str) -> bool {
    if LAUNCHERS.contains(&canonical) && canonical != "pool" {
        return false;
    }
    matches!(canonical, "commands" | "help" | "schema")
        || allowed_flags(canonical).is_some_and(|f| f.contains(&"json"))
}

/// Canonical commands in `COMMANDS` order (aliases folded in).
pub fn canonical_names() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for (name, _) in COMMANDS {
        if canonical_command(name) == *name && !out.contains(name) {
            out.push(name);
        }
    }
    out
}

pub fn summary(name: &str) -> &'static str {
    COMMANDS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
        .unwrap_or("")
}

pub fn aliases(canonical: &str) -> Vec<&'static str> {
    COMMANDS
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| *n != canonical && canonical_command(n) == canonical)
        .collect()
}

fn flags(canonical: &str) -> Vec<&'static str> {
    let mut f: Vec<&'static str> = allowed_flags(canonical)
        .map(<[_]>::to_vec)
        .unwrap_or_default();
    f.retain(|x| !matches!(*x, "json" | "quiet" | "help" | "dump-tui"));
    f.sort_unstable();
    f
}

/// `anyr impl|plan|fix|deploy|cp` are shortcuts for `anyr onboard <mode>`;
/// `help --all` lists them once, under `onboard`.
pub fn is_shortcut(canonical: &str) -> bool {
    subcommands("onboard").contains(&canonical)
}

/// Commands that print their help when run with no subcommand.
pub fn bare_is_help(canonical: &str) -> bool {
    matches!(canonical, "api" | "apps" | "auth")
}

/// One command's entry, as in `commands --json` (`--help --json` prints it).
pub fn command_entry(name: &str) -> Option<Value> {
    let name = canonical_names()
        .into_iter()
        .find(|n| *n == canonical_command(name))?;
    Some(json!({
        "name": name,
        "summary": summary(name),
        "group": group(name),
        "everyday": EVERYDAY.contains(&name),
        "aliases": aliases(name),
        "subcommands": subcommands(name),
        "flags": flags(name),
        "json": has_json_output(name),
    }))
}

/// The whole command tree, one JSON document.
pub fn tree() -> Value {
    let commands: Vec<Value> = canonical_names()
        .into_iter()
        .filter_map(command_entry)
        .collect();
    json!({
        "schema": SCHEMA,
        "version": crate::VERSION,
        "bin": crate::help::invoked_bin(),
        "usage": format!("{} <command> [subcommand] [args] [--flags]", crate::help::invoked_bin()),
        "global_flags": GLOBAL_FLAGS.iter().map(|(n, d)| json!({"name": n, "summary": d})).collect::<Vec<_>>(),
        "env": ENV.iter().map(|(n, d)| json!({"name": n, "summary": d})).collect::<Vec<_>>(),
        "exit_codes": crate::exit::CODES.iter().map(|(c, n, d)| json!({"code": c, "name": n, "meaning": d})).collect::<Vec<_>>(),
        "groups": GROUPS.iter().map(|(id, title)| json!({"id": id, "title": title})).collect::<Vec<_>>(),
        "commands": commands,
    })
}

pub fn tree_json() -> String {
    serde_json::to_string_pretty(&tree()).unwrap_or_else(|_| "{}".into())
}

/// `anyr schema`: the typed command map. Bumped only on a breaking change to
/// its shape; fields may be added.
pub const SCHEMA_V2: &str = "anyr.schema/v2";

/// The credential a command path sends: none, an inference key (`sk-ar-v1-…`)
/// or a management key (`ak_…`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth {
    None,
    Sk,
    Ak,
}

/// What a command path needs and does. A subcommand without a row inherits
/// its command's row.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub path: &'static str,
    pub auth: Auth,
    pub network: bool,
    pub mutating: bool,
    /// Asks before acting; off a terminal it needs `--yes` (else exit 6).
    pub confirms: bool,
    pub args: &'static [&'static str],
}

/// Runs on this machine only and changes nothing.
const fn local(path: &'static str, args: &'static [&'static str]) -> Spec {
    Spec {
        path,
        auth: Auth::None,
        network: false,
        mutating: false,
        confirms: false,
        args,
    }
}

/// Changes local state (config, agent settings) without a request.
const fn edit(path: &'static str, args: &'static [&'static str]) -> Spec {
    Spec {
        mutating: true,
        ..local(path, args)
    }
}

/// Reads from the API.
const fn read(path: &'static str, auth: Auth, args: &'static [&'static str]) -> Spec {
    Spec {
        path,
        auth,
        network: true,
        mutating: false,
        confirms: false,
        args,
    }
}

/// Creates or changes something through the API.
const fn write(path: &'static str, auth: Auth, args: &'static [&'static str]) -> Spec {
    Spec {
        mutating: true,
        ..read(path, auth, args)
    }
}

/// A `write` that asks first (delete, revoke, publish publicly, spend).
const fn guarded(path: &'static str, auth: Auth, args: &'static [&'static str]) -> Spec {
    Spec {
        confirms: true,
        ..write(path, auth, args)
    }
}

use Auth::{Ak, Sk};

/// One row per command (launchers share one, below) and per subcommand that
/// differs from its command. A test requires a row for every command.
pub const SPECS: &[Spec] = &[
    write("login", Auth::None, &[]),
    edit("logout", &[]),
    read("whoami", Sk, &[]),
    read("api", Sk, &["<resource|METHOD>", "[verb|path]", "[id]"]),
    guarded("api keys", Sk, &["[verb]", "[id]"]),
    guarded("api presets", Ak, &["[verb]", "[slug]"]),
    guarded("publish", Sk, &["<app>", "[file|-]"]),
    read("apps", Sk, &[]),
    read("apps schema", Sk, &["<app>"]),
    read("apps ls", Sk, &["<app>"]),
    guarded("apps rm", Sk, &["<app>", "<id>"]),
    write("apps visibility", Sk, &["<app>", "<id>", "<visibility>"]),
    read("decision", Sk, &[]),
    read("models", Auth::None, &[]),
    read("models show", Auth::None, &["<model>"]),
    edit("models use", &["<model>"]),
    read("usage", Sk, &[]),
    read("logs", Sk, &[]),
    read("generation", Sk, &["<id>"]),
    read("presets", Ak, &[]),
    read("presets show", Ak, &["<slug>"]),
    write("presets create", Ak, &["<slug>"]),
    write("presets edit", Ak, &["<slug>"]),
    guarded("presets rm", Ak, &["<slug>"]),
    read("mcp", Sk, &[]),
    local("mcp url", &[]),
    edit("mcp add", &["<agent>"]),
    read("transactions", Sk, &[]),
    read("chat", Sk, &["[prompt|-]"]),
    read("embed", Sk, &["[text…]"]),
    read("eval", Sk, &["[prompt]"]),
    guarded("task", Sk, &["<request>"]),
    write("delegate", Sk, &["<task>"]),
    read("keys", Sk, &[]),
    write("keys create", Sk, &["[name]"]),
    write("keys set", Sk, &["<agent>"]),
    edit("keys unset", &["<agent>"]),
    read("keys show", Sk, &["<last4|id|hash>"]),
    write("keys update", Sk, &["<last4|id|hash>"]),
    guarded("keys revoke", Sk, &["<last4|id|hash>"]),
    read("byok", Ak, &[]),
    guarded("byok add", Ak, &["<provider>"]),
    guarded("byok rm", Ak, &["<provider>"]),
    read("byok test", Ak, &["[provider]"]),
    read("artifact", Sk, &[]),
    guarded("artifact publish", Sk, &["<file|dir>"]),
    guarded("artifact rm", Sk, &["<id>"]),
    guarded("artifact visibility", Sk, &["<id>", "<visibility>"]),
    local("account", &[]),
    edit("account use", &["<name>"]),
    write("account add", Auth::None, &["[name]"]),
    edit("account rename", &["<old>", "<new>"]),
    edit("account remove", &["<name>"]),
    local("auth", &[]),
    write("auth login", Auth::None, &[]),
    edit("auth logout", &[]),
    read("auth status", Sk, &[]),
    edit("auth switch", &["[name]"]),
    local("config", &[]),
    edit("config use", &["<name>"]),
    local("menu", &[]),
    local("onboard", &[]),
    local("impl", &[]),
    local("plan", &[]),
    local("fix", &[]),
    local("deploy", &[]),
    local("cp", &[]),
    read("relay", Sk, &[]),
    write("relay start", Sk, &[]),
    edit("relay stop", &[]),
    local("relay status", &[]),
    local("relay logs", &[]),
    local("relay models", &[]),
    guarded("relay devices", Sk, &["[ls|rename|pool|rm]", "[id]"]),
    write("relay pool", Sk, &["[on|off]"]),
    write("relay pair", Sk, &[]),
    write("relay rename", Sk, &["<name>"]),
    guarded("relay unpair", Sk, &[]),
    read("pool stats", Auth::None, &[]),
    read("pool providers", Auth::None, &[]),
    read("pool free", Auth::None, &[]),
    read("pool relay", Auth::None, &[]),
    write("upgrade", Auth::None, &[]),
    local("completion", &["<shell>"]),
    local("commands", &[]),
    local("schema", &["[command…]"]),
    local("telemetry", &[]),
    local("telemetry status", &[]),
    edit("telemetry on", &[]),
    edit("telemetry off", &[]),
    edit("telemetry reset", &[]),
    // Prints locally; `--file` posts through gh and asks first.
    Spec {
        confirms: true,
        ..local("report", &[])
    },
    local("help", &["[topic]"]),
];

/// Every launcher (`anyr claude`, …) without a row of its own. A headless
/// run (`-p`) asks before spending, so it can exit 6.
const LAUNCH: Spec = Spec {
    confirms: true,
    ..read("", Sk, &["[agent args…]"])
};

/// Value flags whose value is a whole number.
const INT_FLAGS: &[&str] = &[
    "limit",
    "timeout",
    "lines",
    "max-concurrency",
    "max-tokens",
    "concurrency",
    "dimensions",
];

/// The fixed choices of a value flag; `None` = any string.
fn flag_enum(command: &str, flag: &str) -> Option<&'static [&'static str]> {
    match (command, flag) {
        (_, "effort") => Some(crate::spawn::REASONING_LEVELS),
        ("upgrade", "channel") => Some(&["stable", "beta"]),
        ("publish", "visibility") => Some(crate::publish::VISIBILITIES),
        ("artifact", "visibility") => Some(&crate::artifact::VISIBILITIES),
        _ => None,
    }
}

/// The env var that sets a flag when the flag is absent.
fn flag_env(flag: &str) -> Option<&'static str> {
    match flag {
        "key" => Some("ANYROUTER_API_KEY"),
        "profile" => Some("ANYROUTER_PROFILE"),
        "json" => Some("ANYR_OUTPUT"),
        "quiet" => Some("ANYR_QUIET"),
        _ => None,
    }
}

/// `{name, type, required, enum?, env?}` for one flag of `command`. Bool or
/// value comes from the parser itself (`parse::takes_value`).
fn typed_flag(command: &str, flag: &str) -> Value {
    let choices = flag_enum(command, flag);
    let kind = if !crate::parse::takes_value(command, flag) {
        "bool"
    } else if choices.is_some() {
        "enum"
    } else if INT_FLAGS.contains(&flag) {
        "int"
    } else {
        "string"
    };
    let mut f = json!({ "name": flag, "type": kind, "required": false });
    if let Some(choices) = choices {
        f["enum"] = json!(choices);
    }
    if let Some(env) = flag_env(flag) {
        f["env"] = json!(env);
    }
    f
}

/// The row for `path` (`keys create`), falling back to its command's row.
/// A subcommand inherits what its command needs and does, not its args.
pub fn spec(path: &str) -> Spec {
    if let Some(row) = SPECS.iter().find(|s| s.path == path) {
        return *row;
    }
    let (command, sub) = path.split_once(' ').unwrap_or((path, ""));
    let row = SPECS
        .iter()
        .find(|s| s.path == command)
        .copied()
        .unwrap_or(LAUNCH);
    if sub.is_empty() {
        row
    } else {
        Spec { args: &[], ..row }
    }
}

/// Exit codes a path can return, from what it does.
fn exits(s: &Spec) -> Vec<i32> {
    use crate::exit::*;
    let mut codes = vec![OK, ERROR, USAGE];
    if s.network {
        codes.extend([NOT_FOUND, RATE_LIMITED, NETWORK]);
    }
    if s.auth != Auth::None {
        codes.extend([AUTH, PAYMENT_REQUIRED]);
    }
    if s.confirms {
        codes.push(CONFIRM_REQUIRED);
    }
    codes.sort_unstable();
    codes
}

fn auth_name(a: Auth) -> &'static str {
    match a {
        Auth::None => "none",
        Auth::Sk => "sk",
        Auth::Ak => "ak",
    }
}

/// One `anyr schema` entry: a command (`keys`) or a subcommand (`keys create`).
fn schema_entry(command: &'static str, sub: Option<&'static str>) -> Value {
    let path = match sub {
        Some(sub) => format!("{command} {sub}"),
        None => command.to_string(),
    };
    let s = spec(&path);
    let summary = match sub {
        None => summary(command),
        Some(sub) => crate::completion::subcommands(command)
            .iter()
            .find(|(n, _)| *n == sub)
            .map(|(_, d)| *d)
            .unwrap_or(""),
    };
    json!({
        "name": path,
        "summary": summary,
        "aliases": if sub.is_none() { aliases(command) } else { Vec::new() },
        "subcommands": if sub.is_none() { subcommands(command) } else { &[] },
        "flags": flags(command).iter().map(|f| typed_flag(command, f)).collect::<Vec<_>>(),
        "args": s.args,
        "auth": auth_name(s.auth),
        "mutating": s.mutating,
        "json": has_json_output(command),
        "exits": exits(&s),
    })
}

fn schema_entries() -> Vec<Value> {
    let mut out = Vec::new();
    for name in canonical_names() {
        out.push(schema_entry(name, None));
        for sub in subcommands(name) {
            out.push(schema_entry(name, Some(sub)));
        }
    }
    out
}

/// `anyr schema [command [subcommand]]`: the whole map, or one entry. An
/// unknown path is a not-found error (exit 3).
pub fn schema(path: &[String]) -> Result<Value, String> {
    let Some(first) = path.first() else {
        let global: Vec<Value> = GLOBAL_FLAGS
            .iter()
            .map(|(n, d)| {
                let mut f = typed_flag("", n);
                f["summary"] = json!(d);
                f
            })
            .collect();
        return Ok(json!({
            "schema": SCHEMA_V2,
            "version": crate::VERSION,
            "bin": crate::help::invoked_bin(),
            "global_flags": global,
            "exit_codes": crate::exit::CODES.iter().map(|(c, n, d)| json!({"code": c, "name": n, "meaning": d})).collect::<Vec<_>>(),
            "commands": schema_entries(),
        }));
    };
    let unknown = || {
        crate::exit::tag(
            crate::exit::NOT_FOUND,
            format!(
                "No command \"{}\". Run: {} schema",
                path.join(" "),
                crate::help::invoked_bin()
            ),
        )
    };
    let command = canonical_names()
        .into_iter()
        .find(|n| *n == canonical_command(first))
        .ok_or_else(unknown)?;
    let sub = match path.get(1..).unwrap_or_default() {
        [] => None,
        [sub] => Some(
            subcommands(command)
                .iter()
                .copied()
                .find(|s| s == sub)
                .ok_or_else(unknown)?,
        ),
        _ => return Err(unknown()),
    };
    let mut entry = schema_entry(command, sub);
    entry["schema"] = json!(SCHEMA_V2);
    Ok(entry)
}

/// `anyr help agents`: how a coding agent should drive the CLI. Source of
/// truth is `docs/agents.md` in this repo.
pub fn agents_guide() -> String {
    crate::help::doc("agents").replace("anyr ", &format!("{} ", crate::help::invoked_bin()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::dispatch::{cmd_kind, CmdKind};

    #[test]
    fn every_launcher_is_wired_in_each_table() {
        // A launcher is still listed in several tables; one missing entry
        // means a launch that fails, a help page without the agent, or a
        // missing-binary error with no install hint.
        for id in LAUNCHERS {
            assert_eq!(cmd_kind(id), Some(CmdKind::Implemented), "{id}");
            assert!(
                crate::spawn::resolve_tool(None, id).is_ok(),
                "{id}: builtin"
            );
            assert!(crate::install::tool_hint(id).is_some(), "{id}: tool_hint");
            assert!(
                crate::install::KNOWN_AGENTS.iter().any(|(a, _)| a == id),
                "{id}: KNOWN_AGENTS"
            );
            assert!(
                crate::cmd::portable::alias("yolo").is_some_and(|a| a.native(id).is_some()),
                "{id}: portable --yolo cell"
            );
            assert!(
                crate::mcp_wiring::LAUNCHERS.iter().any(|(a, _)| a == id),
                "{id}: MCP strategy"
            );
            let help = crate::help::command_help(id).unwrap_or_default();
            assert!(help.contains("through AnyRouter"), "{id}: help\n{help}");
            assert_eq!(group(id), "launch", "{id}");
        }
    }

    #[test]
    fn every_implemented_command_is_in_the_tree() {
        // Drift guard: a command the dispatcher runs but `COMMANDS` omits is
        // invisible to completion, `help --all` and agents.
        let names = canonical_names();
        let implemented = [
            "login",
            "auth",
            "menu",
            "models",
            "config",
            "keys",
            "whoami",
            "logout",
            "account",
            "usage",
            "claude",
            "codex",
            "grok",
            "opencode",
            "pool",
            "pi",
            "upgrade",
            "onboard",
            "relay",
            "decision",
            "commands",
            "api",
            "completion",
            "byok",
            "publish",
            "apps",
            "artifact",
            "chat",
            "logs",
            "transactions",
            "embed",
            "generation",
            "presets",
            "mcp",
            "eval",
            "task",
            "delegate",
            "schema",
            "telemetry",
            "report",
        ];
        for c in implemented {
            assert_eq!(cmd_kind(c), Some(CmdKind::Implemented), "{c}");
            assert!(names.contains(&c), "{c} missing from dispatch::COMMANDS");
        }
        for c in names.iter().filter(|c| **c != "help") {
            assert!(cmd_kind(c).is_some(), "{c} in COMMANDS but not dispatched");
        }
    }

    #[test]
    fn every_listed_subcommand_appears_in_that_commands_help() {
        for name in canonical_names() {
            let help = crate::help::command_help(name).unwrap_or_default();
            for sub in subcommands(name) {
                assert!(
                    help.contains(sub),
                    "`{name} {sub}` is in the registry but not in `{name} --help`"
                );
            }
        }
    }

    #[test]
    fn everyday_commands_exist_and_every_group_has_a_title() {
        let names = canonical_names();
        for c in EVERYDAY {
            assert!(names.contains(c), "{c}");
        }
        for name in names {
            let g = group(name);
            assert!(GROUPS.iter().any(|(id, _)| *id == g), "{name} → {g}");
        }
    }

    #[test]
    fn tree_lists_exit_codes_and_marks_json_commands() {
        let t = tree();
        assert_eq!(t["schema"], SCHEMA);
        let codes: Vec<i64> = t["exit_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["code"].as_i64().unwrap())
            .collect();
        assert_eq!(codes, vec![0, 1, 2, 3, 4, 5, 6, 7, 8]);
        let find = |n: &str| {
            t["commands"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == n)
                .cloned()
                .unwrap()
        };
        assert_eq!(find("chat")["json"], true);
        assert_eq!(find("claude")["json"], false);
        assert_eq!(find("models")["aliases"], json!(["model"]));
    }

    fn all_paths() -> Vec<(String, &'static str)> {
        let mut out = Vec::new();
        for name in canonical_names() {
            out.push((name.to_string(), name));
            for sub in subcommands(name) {
                out.push((format!("{name} {sub}"), name));
            }
        }
        out
    }

    #[test]
    fn schema_lists_every_path_with_the_flags_the_parser_accepts() {
        // Drift guard: schema, `commands --json` and the parser read the same
        // tables, so an agent that trusts the schema never sends a flag the
        // parser rejects, or a value to a switch.
        let all = schema(&[]).unwrap();
        let entries = all["commands"].as_array().unwrap();
        for (path, command) in all_paths() {
            let e = entries
                .iter()
                .find(|e| e["name"] == path.as_str())
                .unwrap_or_else(|| panic!("{path}: no schema entry"));
            let names: Vec<&str> = e["flags"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f["name"].as_str().unwrap())
                .collect();
            assert_eq!(names, flags(command), "{path}");
            for f in e["flags"].as_array().unwrap() {
                let name = f["name"].as_str().unwrap();
                let kind = f["type"].as_str().unwrap();
                assert!(
                    matches!(kind, "bool" | "string" | "int" | "enum"),
                    "{path} --{name}: {kind}"
                );
                assert_eq!(
                    kind == "bool",
                    !crate::parse::takes_value(command, name),
                    "{path} --{name}"
                );
                assert_eq!(kind == "enum", f["enum"].is_array(), "{path} --{name}");
            }
            assert!(e["exits"].as_array().unwrap().contains(&json!(0)), "{path}");
        }
        assert_eq!(all["schema"], SCHEMA_V2);
    }

    #[test]
    fn every_command_has_a_spec_row_and_every_row_names_a_real_path() {
        // A command without a row would inherit the launcher row and claim an
        // `sk` key it never sends.
        let paths: Vec<String> = all_paths().into_iter().map(|(p, _)| p).collect();
        for row in SPECS {
            assert!(
                paths.iter().any(|p| p == row.path),
                "stale row {}",
                row.path
            );
        }
        for name in canonical_names() {
            if !LAUNCHERS.contains(&name) {
                assert!(SPECS.iter().any(|s| s.path == name), "{name}: no row");
            }
        }
    }

    #[test]
    fn int_and_enum_types_only_mark_value_flags() {
        for name in canonical_names() {
            for f in flags(name) {
                if INT_FLAGS.contains(&f) || flag_enum(name, f).is_some() {
                    assert!(crate::parse::takes_value(name, f), "{name} --{f}");
                }
            }
        }
    }

    #[test]
    fn schema_path_returns_one_entry_and_unknown_paths_exit_3() {
        let e = schema(&["keys".into(), "create".into()]).unwrap();
        assert_eq!(e["name"], "keys create");
        assert_eq!(e["schema"], SCHEMA_V2);
        assert_eq!(e["auth"], "sk");
        assert_eq!(e["mutating"], true);
        assert_eq!(
            schema(&["presets".into(), "rm".into()]).unwrap()["auth"],
            "ak"
        );
        assert_eq!(schema(&["cc".into()]).unwrap()["name"], "claude");
        let e = schema(&["models".into()]).unwrap();
        let effort = schema(&["claude".into()]).unwrap()["flags"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == "effort")
            .cloned()
            .unwrap();
        assert_eq!(effort["type"], "enum");
        assert_eq!(e["mutating"], false);
        for bad in [
            vec!["nope"],
            vec!["keys", "nope"],
            vec!["keys", "create", "x"],
        ] {
            let path: Vec<String> = bad.iter().map(|s| s.to_string()).collect();
            let err = schema(&path).unwrap_err();
            assert_eq!(
                crate::exit::classify(&err).0,
                crate::exit::NOT_FOUND,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn agents_guide_names_json_mode_and_exit_codes() {
        let g = agents_guide();
        for want in [
            "--json",
            "ANYR_OUTPUT=json",
            "commands --json",
            "Exit codes",
        ] {
            assert!(g.contains(want), "{want}");
        }
        for (code, name, _) in crate::exit::CODES {
            assert!(g.contains(&format!("{code}")) && g.contains(name), "{name}");
        }
    }
}
