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
        _ => &[],
    }
}

/// Does `--json` print JSON on stdout (not only JSON errors)?
pub fn has_json_output(canonical: &str) -> bool {
    if LAUNCHERS.contains(&canonical) && canonical != "pool" {
        return false;
    }
    matches!(canonical, "commands" | "help")
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

/// `anyr help agents`: how a coding agent should drive the CLI. Source of
/// truth is `docs/agents.md` in this repo.
pub fn agents_guide() -> String {
    include_str!("../docs/agents.md").replace("anyr ", &format!("{} ", crate::help::invoked_bin()))
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
        assert_eq!(codes, vec![0, 1, 2, 3, 4, 5]);
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
