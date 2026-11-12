//! Shell completion. One engine (`anyr __complete <words…>`) answers every
//! shell; the scripts from `anyr completion <shell>` are thin shims that call
//! it, so the command grammar lives only in Rust (cobra's model).
//!
//! Protocol: one candidate per line, `value` or `value\tdescription`.
//! Never prints to stderr, never prompts, never colors.

use std::collections::BTreeMap;

use crate::cmd::dispatch::{allowed_flags, value_choices, COMMANDS};

pub const SHELLS: &[&str] = &["bash", "zsh", "fish", "powershell"];

/// Subcommands per command, shared with help/dispatch tests.
pub fn subcommands(command: &str) -> &'static [(&'static str, &'static str)] {
    match command {
        "auth" => &[
            ("login", "Sign in"),
            ("logout", "Remove the stored key"),
            ("status", "Show the signed-in account"),
            ("token", "Print the active API key"),
            ("switch", "Switch the active account"),
        ],
        "account" => &[
            ("list", "List accounts"),
            ("use", "Switch the active account"),
            ("add", "Sign in to another account"),
        ],
        "keys" => &[
            ("list", "List API keys"),
            ("create", "Create an API key"),
            ("set", "Give a coding agent its own key"),
            ("unset", "Remove a coding agent's own key"),
            ("show", "Limit, spent, endpoints and expiry of a key"),
            ("update", "Change a key's limit, endpoints, expiry or state"),
            ("audit", "List keys that look unused"),
            ("rotate", "New secret for a key; the old one stops working"),
            ("revoke", "Revoke an API key"),
        ],
        "credits" => &[
            ("balance", "Credits remaining (same as usage)"),
            ("history", "Credit ledger (same as transactions)"),
        ],
        "stats" => &[
            ("summary", "Your requests, tokens, cost and latency"),
            ("usage", "Your spend and tokens per day"),
            ("keys", "Requests and spend per API key"),
            ("models", "Top models across the platform (public)"),
            ("burn", "Tokens per day across the platform (public)"),
        ],
        "config" => &[
            ("get", "Print current settings"),
            ("path", "Print the config file path"),
            ("use", "Switch the active account"),
        ],
        "telemetry" => &[
            ("status", "On or off, why, and the next payload"),
            ("on", "Turn anonymous usage pings on"),
            ("off", "Turn anonymous usage pings off"),
            ("reset", "Rotate the random install id"),
        ],
        "completion" => &[
            ("bash", "Bash completion script"),
            ("zsh", "Zsh completion script"),
            ("fish", "Fish completion script"),
            ("powershell", "PowerShell completion script"),
        ],
        "models" => &[
            ("list", "List catalog model ids"),
            ("ls", "Alias of list"),
            ("use", "Set the default model"),
        ],
        "byok" => &[
            ("list", "List provider keys with BYOK / DONATED badges"),
            (
                "add",
                "Add a provider key (--donate shares it with the pool)",
            ),
            ("rm", "Delete a saved provider key"),
            ("test", "Check a saved key (or --key-stdin before saving)"),
            ("providers", "Providers that accept BYOK, with your badge"),
        ],
        "mcp" => &[
            ("url", "Print the MCP endpoint URL"),
            ("add", "Wire it into claude, codex, opencode or cursor"),
            ("connections", "List upstream MCP connections"),
        ],
        "presets" => &[
            ("list", "List presets"),
            ("show", "Show one preset"),
            ("create", "Create a preset from --model a,b"),
            ("edit", "Change models, fallback, sort or name"),
            ("rm", "Delete a preset"),
        ],
        "pool" => &[
            ("stats", "Providers, keys, donors, free coverage"),
            ("providers", "Per provider: keys and donors"),
            ("free", "Backends the pool serves for free now"),
            ("relay", "On-device relay pool availability"),
        ],
        "relay" => &[
            ("start", "Serve local models (--detach for background)"),
            ("stop", "Stop the running relay"),
            ("status", "Show whether it runs and what it serves"),
            ("logs", "Show the background relay log"),
            ("models", "List local models; enable or disable them"),
            ("devices", "List, rename, pool or remove paired devices"),
            ("pool", "Show or set pool sharing (on|off)"),
            ("pair", "Pair this device"),
            ("unpair", "Remove this device from your account"),
            ("doctor", "Check macOS, fm serve, Decider, network, auth"),
        ],
        "api" => crate::api::RESOURCES,
        "apps" => crate::publish::APPS_VERBS,
        "artifact" => crate::artifact::VERBS,
        _ => &[],
    }
}

/// Candidates for the word being completed. `words` excludes the binary name;
/// the last element is the (possibly empty) partial word.
pub fn complete(words: &[String], env: &BTreeMap<String, String>) -> Vec<String> {
    let (current, before) = match words.split_last() {
        Some((cur, rest)) => (cur.as_str(), rest),
        None => ("", &[][..]),
    };
    let prefixed = |items: Vec<(String, String)>| -> Vec<String> {
        items
            .into_iter()
            .filter(|(v, _)| v.starts_with(current))
            .map(|(v, d)| if d.is_empty() { v } else { format!("{v}\t{d}") })
            .collect()
    };

    // After `--` everything belongs to the wrapped agent.
    if before.iter().any(|w| w == "--") {
        return Vec::new();
    }

    let Some(command) = before.first().map(String::as_str) else {
        return prefixed(
            COMMANDS
                .iter()
                .map(|(n, d)| (n.to_string(), d.to_string()))
                .collect(),
        );
    };

    // Value for the previous `--flag`.
    if let Some(flag) = before.last().and_then(|w| w.strip_prefix("--")) {
        if before.len() > 1 && crate::parse::VALUE_FLAGS.contains(flag) {
            return prefixed(
                value_choices(flag, env)
                    .into_iter()
                    .map(|v| (v, String::new()))
                    .collect(),
            );
        }
    }

    if current.starts_with('-') {
        let canonical = crate::cmd::dispatch::canonical_name(command);
        let mut flags: Vec<(String, String)> = allowed_flags(canonical)
            .unwrap_or(&[])
            .iter()
            .map(|f| (format!("--{f}"), String::new()))
            .collect();
        if crate::cmd::dispatch::launch_flags(canonical).is_some() {
            for alias in crate::cmd::portable::ALIASES {
                let flag = format!("--{}", alias.name);
                if !flags.iter().any(|(f, _)| *f == flag) {
                    flags.push((flag, String::new()));
                }
            }
        }
        flags.push(("--help".into(), String::new()));
        return prefixed(flags);
    }

    let positional: Vec<&str> = before[1..]
        .iter()
        .map(String::as_str)
        .filter(|w| !w.starts_with('-'))
        .collect();
    let canonical = crate::cmd::dispatch::canonical_name(command);
    match (canonical, positional.as_slice()) {
        ("api", [resource]) => prefixed(
            crate::api::verbs(resource)
                .iter()
                .map(|(v, d)| (v.to_string(), d.to_string()))
                .collect(),
        ),
        ("account" | "config" | "auth", ["use" | "switch"]) => prefixed(
            value_choices("profile", env)
                .into_iter()
                .map(|v| (v, String::new()))
                .collect(),
        ),
        ("schema", []) => prefixed(
            COMMANDS
                .iter()
                .filter(|(n, _)| *n != "schema")
                .map(|(n, d)| (n.to_string(), d.to_string()))
                .collect(),
        ),
        ("schema", [command]) => prefixed(
            subcommands(crate::cmd::dispatch::canonical_name(command))
                .iter()
                .map(|(n, d)| (n.to_string(), d.to_string()))
                .collect(),
        ),
        (_, []) => prefixed(
            subcommands(canonical)
                .iter()
                .map(|(n, d)| (n.to_string(), d.to_string()))
                .collect(),
        ),
        _ => Vec::new(),
    }
}

/// Shell script that wires `<bin> __complete` into the given shell.
pub fn script(shell: &str, bin: &str) -> Option<String> {
    let body = match shell {
        "bash" | "zsh" | "fish" | "powershell" => crate::help::doc(&format!("completion_{shell}")),
        "pwsh" => crate::help::doc("completion_powershell"),
        _ => return None,
    };
    let func = bin.replace(['-', '.'], "_");
    Some(body.replace("{bin}", bin).replace("{func}", &func))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &[&str]) -> Vec<String> {
        s.iter().map(|w| w.to_string()).collect()
    }

    fn values(out: &[String]) -> Vec<&str> {
        out.iter().map(|l| l.split('\t').next().unwrap()).collect()
    }

    #[test]
    fn top_level_lists_launch_and_api_commands() {
        let out = complete(&words(&[""]), &BTreeMap::new());
        let v = values(&out);
        for want in ["login", "claude", "grok", "api", "completion"] {
            assert!(v.contains(&want), "missing {want}: {v:?}");
        }
    }

    #[test]
    fn prefix_filters_candidates() {
        let out = complete(&words(&["gr"]), &BTreeMap::new());
        assert_eq!(values(&out), vec!["grok"]);
    }

    #[test]
    fn launch_flags_include_yolo() {
        let out = complete(&words(&["claude", "--y"]), &BTreeMap::new());
        assert!(values(&out).contains(&"--yolo"), "{out:?}");
    }

    #[test]
    fn launchers_complete_portable_flags() {
        let out = complete(&words(&["codex", "--add"]), &BTreeMap::new());
        assert_eq!(values(&out), ["--add-dir"], "{out:?}");
        let out = complete(&words(&["usage", "--add"]), &BTreeMap::new());
        assert!(values(&out).is_empty(), "{out:?}");
    }

    #[test]
    fn effort_flag_offers_levels() {
        let out = complete(&words(&["claude", "--effort", ""]), &BTreeMap::new());
        assert!(values(&out).contains(&"high"), "{out:?}");
    }

    #[test]
    fn nothing_after_double_dash() {
        let out = complete(&words(&["claude", "--", ""]), &BTreeMap::new());
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn api_resource_then_verb() {
        let res = complete(&words(&["api", ""]), &BTreeMap::new());
        assert!(values(&res).contains(&"keys"), "{res:?}");
        let verbs = complete(&words(&["api", "keys", ""]), &BTreeMap::new());
        assert!(values(&verbs).contains(&"create"), "{verbs:?}");
    }

    #[test]
    fn every_shell_has_a_script_that_calls_the_engine() {
        for shell in SHELLS {
            let s = script(shell, "anyr").unwrap();
            assert!(
                s.contains("__complete") && s.contains("anyr"),
                "{shell}: {s}"
            );
        }
        assert!(script("tcsh", "anyr").is_none());
    }
}
