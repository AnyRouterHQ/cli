use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

pub static VALUE_FLAGS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "model",
        "effort",
        "preset",
        "key",
        "profile",
        "base-url",
        "config",
        "command-path",
        "claude-path",
        "timeout",
        "system",
        "plan-model",
        "do-model",
        "to",
        "from",
        "limit",
        "cursor",
        "status",
        "type",
        "hub",
        "source",
        "tool",
        "agent",
        "channel",
        "fixture",
        "haiku",
        "sonnet",
        "opus",
        "fable",
        "target",
        "token",
        "url",
        "name",
        "description",
        "max-concurrency",
        "lines",
        "state",
        "questions",
        "data",
        "method",
        "idempotency-key",
        "visibility",
        "light",
        "title",
        "id",
        "label",
        "file",
        "owner",
        "search",
        "sort",
        "reset",
        "endpoints",
        "expires",
        "dimensions",
        "prompts",
        "max-tokens",
        "concurrency",
    ])
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagValue {
    Bool(bool),
    Value(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedArgs {
    pub command: String,
    pub flags: HashMap<String, FlagValue>,
    pub passthrough: Vec<String>,
    /// Index in `passthrough` where the tokens after `--` start.
    pub raw_from: Option<usize>,
}

impl ParsedArgs {
    pub fn flag_true(&self, name: &str) -> bool {
        matches!(self.flags.get(name), Some(FlagValue::Bool(true)))
    }

    /// `--yes` or `--ok` skip confirmation prompts (login / install / revoke).
    pub fn skip_confirm(&self) -> bool {
        self.flag_true("yes") || self.flag_true("ok")
    }
}

pub fn parse_cli_args<I, S>(argv: I) -> Result<ParsedArgs, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let tokens: Vec<String> = argv.into_iter().map(|s| s.as_ref().to_string()).collect();
    let command = tokens
        .first()
        .cloned()
        .unwrap_or_else(|| "help".to_string());
    let rest = if tokens.is_empty() {
        &[][..]
    } else {
        &tokens[1..]
    };

    let launcher_owned = crate::cmd::dispatch::launch_flags(&command);
    let mut flags = HashMap::new();
    let mut passthrough = Vec::new();
    let mut after_separator = false;
    let mut raw_from = None;
    let mut i = 0usize;

    while i < rest.len() {
        let arg = &rest[i];
        if after_separator {
            passthrough.push(arg.clone());
            i += 1;
            continue;
        }
        if arg == "--" {
            after_separator = true;
            raw_from = Some(passthrough.len());
            i += 1;
            continue;
        }
        if !arg.starts_with("--") || launcher_owned.is_some_and(|owned| !owns(owned, arg)) {
            passthrough.push(arg.clone());
            i += 1;
            continue;
        }
        if let Some(eq) = arg.find('=') {
            let eq_name = arg[2..eq].to_string();
            let eq_value = arg[eq + 1..].to_string();
            if takes_value(&command, &eq_name) {
                if eq_value.is_empty() {
                    return Err(format!("Flag --{eq_name} requires a value."));
                }
                insert_value_flag(&mut flags, &command, eq_name, eq_value);
            } else {
                flags.insert(
                    eq_name.clone(),
                    FlagValue::Bool(parse_bool_flag(&eq_name, &eq_value)?),
                );
            }
            i += 1;
            continue;
        }
        let name = arg[2..].to_string();
        if takes_value(&command, &name) {
            let next = rest.get(i + 1);
            if next.is_none()
                || next == Some(&"--".to_string())
                || next.is_some_and(|n| n.starts_with("--"))
            {
                return Err(format!("Flag --{name} requires a value."));
            }
            insert_value_flag(&mut flags, &command, name, rest[i + 1].clone());
            i += 2;
        } else {
            flags.insert(name, FlagValue::Bool(true));
            i += 1;
        }
    }

    Ok(ParsedArgs {
        command,
        flags,
        passthrough,
        raw_from,
    })
}

/// Does a launcher keep `--name` / `--name=value` for itself? Of the global
/// flags `--help` and `--json` (anyr's own output) are always anyr's;
/// `--quiet` reaches the agent unless the launcher lists it.
fn owns(owned: &[&str], arg: &str) -> bool {
    let name = arg[2..].split('=').next().unwrap_or_default();
    owned.contains(&name) || matches!(name, "help" | "json")
}

/// `--target` may repeat (relay with several local endpoints), and so may
/// `relay --model` (several on-device models): repeats are joined with
/// commas. Every other value flag keeps last-wins.
/// `--status` filters lists elsewhere, but on `update` it is a switch.
pub(crate) fn takes_value(command: &str, name: &str) -> bool {
    VALUE_FLAGS.contains(name) && !(matches!(command, "update" | "upgrade") && name == "status")
}

fn insert_value_flag(
    flags: &mut HashMap<String, FlagValue>,
    command: &str,
    name: String,
    value: String,
) {
    if name == "target" || (command == "relay" && name == "model") {
        if let Some(FlagValue::Value(prev)) = flags.get(&name) {
            let joined = format!("{prev},{value}");
            flags.insert(name, FlagValue::Value(joined));
            return;
        }
    }
    flags.insert(name, FlagValue::Value(value));
}

fn parse_bool_flag(name: &str, raw: &str) -> Result<bool, String> {
    let t = raw.trim();
    if t.is_empty()
        || t == "1"
        || t.eq_ignore_ascii_case("true")
        || t.eq_ignore_ascii_case("yes")
        || t.eq_ignore_ascii_case("on")
    {
        return Ok(true);
    }
    if t == "0"
        || t.eq_ignore_ascii_case("false")
        || t.eq_ignore_ascii_case("no")
        || t.eq_ignore_ascii_case("off")
    {
        return Ok(false);
    }
    Err(format!("Flag --{name} does not take a value ({raw})."))
}

pub fn get_string_flag(flags: &HashMap<String, FlagValue>, name: &str) -> Option<String> {
    match flags.get(name) {
        Some(FlagValue::Value(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn update_status_is_a_switch_not_a_value_flag() {
        let p = parse_cli_args(["update", "--status", "--json"]).unwrap();
        assert!(p.flag_true("status") && p.flag_true("json"));
        assert!(parse_cli_args(["keys", "--status"]).is_err());
    }

    use super::*;

    #[test]
    fn repeated_target_flags_are_joined_but_other_flags_keep_last() {
        let parsed = parse_cli_args([
            "relay",
            "start",
            "--target",
            "http://a/v1",
            "--target=http://b/v1",
            "--effort",
            "low",
            "--effort",
            "high",
        ])
        .unwrap();
        assert_eq!(
            get_string_flag(&parsed.flags, "target").as_deref(),
            Some("http://a/v1,http://b/v1")
        );
        assert_eq!(
            get_string_flag(&parsed.flags, "effort").as_deref(),
            Some("high")
        );
    }

    #[test]
    fn relay_model_repeats_but_other_commands_keep_last_model() {
        // WHY: `relay start --model a --model b` serves both; dropping one would
        // silently stop serving a model the user asked for.
        let relay = parse_cli_args(["relay", "start", "--model", "a", "--model=b"]).unwrap();
        assert_eq!(
            get_string_flag(&relay.flags, "model").as_deref(),
            Some("a,b")
        );
        let claude = parse_cli_args(["claude", "--model", "a", "--model", "b"]).unwrap();
        assert_eq!(
            get_string_flag(&claude.flags, "model").as_deref(),
            Some("b")
        );
    }

    #[test]
    fn no_pool_is_a_boolean_flag() {
        let parsed = parse_cli_args(["relay", "start", "--no-pool", "--all"]).unwrap();
        assert!(parsed.flag_true("no-pool"));
        assert!(parsed.flag_true("all"));
    }

    #[test]
    fn login_yes_is_boolean_true() {
        let parsed = parse_cli_args(["login", "--yes"]).unwrap();
        assert_eq!(parsed.command, "login");
        assert!(parsed.flag_true("yes"));
    }

    #[test]
    fn model_without_value_errors() {
        assert!(parse_cli_args(["claude", "--model"])
            .unwrap_err()
            .contains("--model requires a value"));
        assert!(parse_cli_args(["claude", "--model="])
            .unwrap_err()
            .contains("--model requires a value"));
    }

    #[test]
    fn decision_state_and_questions_require_values() {
        assert!(parse_cli_args(["decision", "--state"])
            .unwrap_err()
            .contains("--state requires a value"));
        assert!(parse_cli_args(["decision", "--questions"])
            .unwrap_err()
            .contains("--questions requires a value"));
        let parsed = parse_cli_args([
            "decision",
            "--state",
            "hello",
            "--questions",
            "{\"urgent\":{\"type\":\"noul\"}}",
        ])
        .unwrap();
        assert_eq!(
            get_string_flag(&parsed.flags, "state").as_deref(),
            Some("hello")
        );
        assert_eq!(
            get_string_flag(&parsed.flags, "questions").as_deref(),
            Some("{\"urgent\":{\"type\":\"noul\"}}")
        );
    }

    #[test]
    fn account_list_goes_to_passthrough() {
        let parsed = parse_cli_args(["account", "list"]).unwrap();
        assert_eq!(parsed.passthrough, vec!["list"]);
    }

    #[test]
    fn auth_login_goes_to_passthrough() {
        let parsed = parse_cli_args(["auth", "login", "--yes"]).unwrap();
        assert_eq!(parsed.command, "auth");
        assert_eq!(parsed.passthrough, vec!["login"]);
        assert!(parsed.flag_true("yes"));
    }

    #[test]
    fn separator_starts_passthrough() {
        let parsed = parse_cli_args(["claude", "--yes", "--", "--print"]).unwrap();
        assert_eq!(parsed.passthrough, vec!["--print"]);
    }

    #[test]
    fn bool_flag_equals_form_is_still_true() {
        // WHY: `--yes=true` used to land as FlagValue::Value, so flag_true missed it.
        let parsed = parse_cli_args(["login", "--yes=true"]).unwrap();
        assert!(parsed.flag_true("yes"));
        assert!(parsed.skip_confirm());
        let device = parse_cli_args(["login", "--device=1"]).unwrap();
        assert!(device.flag_true("device"));
        let off = parse_cli_args(["login", "--yes=false"]).unwrap();
        assert!(!off.flag_true("yes"));
    }

    #[test]
    fn ok_is_confirm_alias() {
        let parsed = parse_cli_args(["claude", "--ok"]).unwrap();
        assert!(parsed.skip_confirm());
        assert!(parsed.flag_true("ok"));
    }
}
