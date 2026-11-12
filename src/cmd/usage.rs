use std::collections::BTreeMap;

use crate::http::{fetch_credits, format_usage_report};
use crate::key::{load_config_if_present, no_key_error, resolve_api_key, resolve_base_url};
use crate::parse::ParsedArgs;

use crate::cmd::dispatch::config_path;

pub(crate) fn run_usage(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let path = config_path(parsed, env);
    let existing = load_config_if_present(&path);
    let profile = existing
        .as_ref()
        .and_then(|c| c.profiles.get(&c.active_profile));
    let key = resolve_api_key(&parsed.flags, env, profile).ok_or_else(no_key_error)?;
    let base = resolve_base_url(&parsed.flags, profile);
    let credits = fetch_credits(&base, &key)?;
    print!(
        "{}",
        format_usage_report(&credits, parsed.flag_true("json"))
    );
    Ok(0)
}

/// `anyr credits [balance|history]`: `balance` (the default) is `usage`,
/// `history` is `transactions`; same code paths, same output and JSON.
pub(crate) fn run_credits(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let mut next = parsed.clone();
    let verb = if next.passthrough.is_empty() {
        "balance".to_string()
    } else {
        next.passthrough.remove(0)
    };
    // Each verb takes only its own command's flags.
    let (own, other): (&[&str], &str) = match verb.as_str() {
        "balance" => (&["no-detail"], "history"),
        "history" => (&["limit", "type"], "balance"),
        _ => {
            return Err(crate::cmd::dispatch::hint(&format!(
                "Unknown credits subcommand \"{verb}\". Try: balance | history"
            )))
        }
    };
    for flag in ["no-detail", "limit", "type"] {
        if next.flags.contains_key(flag) && !own.contains(&flag) {
            return Err(crate::cmd::dispatch::hint(&format!(
                "--{flag} belongs to: {{bin}} credits {other}"
            )));
        }
    }
    if verb == "history" {
        crate::cmd::transactions::run_transactions(&next, env)
    } else if next.passthrough.is_empty() {
        run_usage(&next, env)
    } else {
        Err(crate::cmd::dispatch::hint(
            "Usage: {bin} credits [balance] [--json]",
        ))
    }
}
