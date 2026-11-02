use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::config::{get_active_profile, read_config, Config, Profile, DEFAULT_BASE_URL};
use crate::parse::{get_string_flag, FlagValue};
use crate::spawn::canonical_tool;

/// Known non-secret fixture literals (mirrors the TS CLI's persistApiKey guard):
/// these exact values once overwrote a user's real key via a test fixture, so
/// they must never act as a live credential — treat them like "no key".
const DUMMY_KEYS: [&str; 3] = ["sk-ar-v1-test", "sk-ar-v1-testkey", "sk-ar-v1-test-key"];

pub fn is_dummy_key(value: &str) -> bool {
    let trimmed = value.trim();
    DUMMY_KEYS
        .iter()
        .any(|dummy| trimmed.eq_ignore_ascii_case(dummy))
}

/// Reject anything that is not a whole AnyRouter API key before it is stored.
/// The message never repeats the input.
pub fn validate_api_key(key: &str) -> Result<(), String> {
    let body = key.strip_prefix("sk-ar-v1-").ok_or(
        "That is not an AnyRouter API key. Copy the whole key, which starts with sk-ar-v1.",
    )?;
    let well_formed = body.len() >= 16
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !well_formed || is_dummy_key(key) {
        return Err(
            "That API key looks cut off or malformed. Copy the whole key and try again.".into(),
        );
    }
    Ok(())
}

/// `--key -` reads one line from stdin. A literal `--key sk-...` still works
/// but lands in argv (`ps`, shell history), so it warns on `warn_to`. Shared by
/// `auth login` and every other command. Returns the key and whether it came
/// from stdin; `None` for an empty flag.
pub fn key_from_flag(
    raw: &str,
    stdin: &mut dyn std::io::BufRead,
    warn_to: &mut dyn std::io::Write,
) -> Result<Option<(String, bool)>, String> {
    let trimmed = raw.trim();
    if trimmed == "-" {
        let mut line = String::new();
        stdin
            .read_line(&mut line)
            .map_err(|e| format!("could not read the key from stdin: {e}"))?;
        let key = line.trim();
        if key.is_empty() {
            return Err("No key on stdin (expected `--key -` to read one line).".into());
        }
        return Ok(Some((key.to_string(), true)));
    }
    if trimmed.is_empty() {
        return Ok(None);
    }
    let _ = writeln!(
        warn_to,
        "warning: --key puts the secret in your process list and shell history; \
prefer `--key -` (stdin), ANYROUTER_API_KEY, or --paste."
    );
    Ok(Some((trimmed.to_string(), false)))
}

/// Resolve `--key -` once, up front, for commands that read the flag through
/// `resolve_api_key` (many call sites; stdin can only be read once). A literal
/// key warns once here. Login-family commands handle the flag themselves.
pub fn normalize_key_flag(
    flags: &mut HashMap<String, FlagValue>,
    stdin: &mut dyn std::io::BufRead,
    warn_to: &mut dyn std::io::Write,
) -> Result<(), String> {
    let Some(raw) = get_string_flag(flags, "key") else {
        return Ok(());
    };
    match key_from_flag(&raw, stdin, warn_to)? {
        Some((key, _)) => flags.insert("key".into(), FlagValue::Value(key)),
        None => flags.remove("key"),
    };
    Ok(())
}

pub fn resolve_api_key(
    flags: &std::collections::HashMap<String, FlagValue>,
    env: &BTreeMap<String, String>,
    profile: Option<&Profile>,
) -> Option<String> {
    if let Some(key) = get_string_flag(flags, "key") {
        let trimmed = key.trim();
        if !trimmed.is_empty() && !is_dummy_key(trimmed) {
            return Some(trimmed.to_string());
        }
    }
    if let Some(key) = env
        .get("ANYROUTER_API_KEY")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !is_dummy_key(s))
    {
        return Some(key.to_string());
    }
    profile
        .and_then(|p| p.api_key.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_dummy_key(s))
        .map(str::to_string)
}

/// Which account `tool` launches with, and whether the agent's own binding
/// chose it: `--profile` / `ANYROUTER_PROFILE`, then the agent's bound
/// account, then the session `active_profile`.
pub fn account_for_agent(
    config: &Config,
    flags: &HashMap<String, FlagValue>,
    env: &BTreeMap<String, String>,
    tool: &str,
) -> (String, bool) {
    let explicit = get_string_flag(flags, "profile")
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            env.get("ANYROUTER_PROFILE")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });
    if let Some(name) = explicit {
        return (name, false);
    }
    if let Some(name) = config
        .agent_binding(canonical_tool(tool))
        .and_then(|b| b.profile.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return (name.to_string(), true);
    }
    (config.active_profile.clone(), false)
}

/// Profile used when launching `tool` (see [`account_for_agent`]).
pub fn profile_for_agent<'a>(
    config: &'a Config,
    flags: &HashMap<String, FlagValue>,
    env: &BTreeMap<String, String>,
    tool: &str,
) -> Option<&'a Profile> {
    config
        .profiles
        .get(&account_for_agent(config, flags, env, tool).0)
}

/// Where a launch key came from, in precedence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// `--key` on the command line.
    Flag,
    /// `ANYROUTER_API_KEY`.
    Env,
    /// `agents.<id>.api_key`.
    AgentKey,
    /// The account bound with `account use <name> --agent <id>`.
    AgentAccount(String),
    /// `--profile` / `ANYROUTER_PROFILE`, else the active account.
    Account(String),
}

impl KeySource {
    /// Stable id for `--json`.
    pub fn code(&self) -> &'static str {
        match self {
            KeySource::Flag => "flag",
            KeySource::Env => "env",
            KeySource::AgentKey => "agent_key",
            KeySource::AgentAccount(_) => "agent_account",
            KeySource::Account(_) => "account",
        }
    }

    pub fn account(&self) -> Option<&str> {
        match self {
            KeySource::AgentAccount(name) | KeySource::Account(name) => Some(name),
            _ => None,
        }
    }

    pub fn label(&self) -> String {
        match self {
            KeySource::Flag => "--key".into(),
            KeySource::Env => "ANYROUTER_API_KEY".into(),
            KeySource::AgentKey => "agent key".into(),
            KeySource::AgentAccount(name) => format!("agent account {name}"),
            KeySource::Account(name) => format!("account {name}"),
        }
    }
}

fn usable_key(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty() && !is_dummy_key(s))
        .map(str::to_string)
}

/// Launch key for `tool` and its source: `--key`, then `ANYROUTER_API_KEY`,
/// then the agent's own `api_key`, then its bound account, then `--profile` /
/// `ANYROUTER_PROFILE` or the active account. An agent key beats
/// `--profile` on purpose: it is the narrower, explicit choice for that agent.
/// It never falls back to the default profile when the stored key is set.
pub fn resolve_launch_key(
    flags: &HashMap<String, FlagValue>,
    env: &BTreeMap<String, String>,
    config: Option<&Config>,
    tool: &str,
) -> Option<(String, KeySource)> {
    if let Some(key) = get_string_flag(flags, "key").and_then(|k| usable_key(Some(&k))) {
        return Some((key, KeySource::Flag));
    }
    if let Some(key) = usable_key(env.get("ANYROUTER_API_KEY").map(String::as_str)) {
        return Some((key, KeySource::Env));
    }
    let cfg = config?;
    let binding = cfg.agent_binding(canonical_tool(tool));
    if let Some(key) = usable_key(binding.and_then(|b| b.api_key.as_deref())) {
        return Some((key, KeySource::AgentKey));
    }
    let (name, bound) = account_for_agent(cfg, flags, env, tool);
    let key = usable_key(cfg.profiles.get(&name)?.api_key.as_deref())?;
    let source = if bound {
        KeySource::AgentAccount(name)
    } else {
        KeySource::Account(name)
    };
    Some((key, source))
}

/// Keep `[1m]` / `[500k]` on virtual presets (`anyrouter/auto`, `free`, `byok`, …)
/// so launch can peel them into `provider.min_context`. Concrete ids still drop
/// Claude's `[1m]` tag.
fn launch_model_id(raw: &str) -> String {
    let raw = raw.trim();
    if crate::config::parse_context_window_suffix(raw).is_some()
        && crate::spawn::is_virtual_preset(raw)
    {
        let stem = crate::config::strip_context_window_suffix(raw);
        let suffix = &raw[stem.len()..];
        return format!("{}{}", crate::spawn::display_model_id(stem), suffix);
    }
    crate::spawn::display_model_id(raw)
}

/// Launch model for `tool`: `--model`, then the agent's bound id, then the
/// profile default. Does not invent catalog ids.
pub fn resolve_launch_model(
    flags: &HashMap<String, FlagValue>,
    config: Option<&Config>,
    profile: &Profile,
    tool: &str,
) -> String {
    if let Some(m) = get_string_flag(flags, "model") {
        return launch_model_id(&m);
    }
    let id = canonical_tool(tool);
    if let Some(m) = config
        .and_then(|c| c.agent_binding(id))
        .and_then(|b| b.default_model.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return launch_model_id(m);
    }
    launch_model_id(profile.default_model())
}

pub fn resolve_base_url(
    flags: &std::collections::HashMap<String, FlagValue>,
    profile: Option<&Profile>,
) -> String {
    if let Some(url) = get_string_flag(flags, "base-url") {
        return url;
    }
    profile
        .map(|p| p.base_url().to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

pub fn load_config_if_present(path: &Path) -> Option<Config> {
    if path.exists() {
        read_config(path).ok()
    } else {
        None
    }
}

/// Load the config for a credentialed command. A missing config file is fine
/// when a `--key` flag or `ANYROUTER_API_KEY` already supplies the credential,
/// so headless/CI use needs no `anyr login`.
pub fn config_for_credential(
    path: &Path,
    flags: &HashMap<String, FlagValue>,
    env: &BTreeMap<String, String>,
) -> Result<Config, String> {
    match load_config_if_present(path) {
        Some(cfg) => Ok(cfg),
        None if resolve_api_key(flags, env, None).is_some() => Ok(Config::default()),
        None => Err(no_key_error()),
    }
}

/// Stored profile `name`, or an empty one when the config holds no profiles at
/// all and a flag/env key stands in for it. A named-but-missing profile in a
/// populated config stays an error so typos are not silently ignored.
pub fn profile_or_default(
    cfg: &Config,
    name: &str,
    flags: &HashMap<String, FlagValue>,
    env: &BTreeMap<String, String>,
) -> Result<Profile, String> {
    match cfg.profiles.get(name) {
        Some(p) => Ok(p.clone()),
        None if cfg.profiles.is_empty() && resolve_api_key(flags, env, None).is_some() => {
            Ok(Profile::default())
        }
        None => Err(format!(
            "Profile \"{name}\" was not found in AnyRouter config."
        )),
    }
}

pub fn active_profile<'a>(
    config: &'a Config,
    flags: &std::collections::HashMap<String, FlagValue>,
    env: &BTreeMap<String, String>,
) -> Result<&'a Profile, String> {
    let name = get_string_flag(flags, "profile");
    get_active_profile(config, name.as_deref(), env)
}

pub fn mask_api_key(key: Option<&str>) -> String {
    let k = key.unwrap_or("").trim();
    if k.is_empty() {
        return "<none>".into();
    }
    let prefix_len = if k.starts_with("sk-ar-v1-") { 14 } else { 8 };
    // Too short to show a prefix and a suffix without showing most of it:
    // keep only the family marker.
    if k.chars().count() <= prefix_len + 8 {
        let marker = ["sk-ar-v1-", "ak_"]
            .into_iter()
            .find(|m| k.starts_with(m))
            .unwrap_or("");
        return format!("{marker}…");
    }
    let prefix: String = k.chars().take(prefix_len).collect();
    let suffix: String = k
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{prefix}…{suffix}")
}

/// True when `out` ends in a `%XX` escape or a literal `\n`, `\t`, `\r`.
fn after_escape(out: &str) -> bool {
    let b = out.as_bytes();
    let n = b.len();
    let pct = n >= 3 && b[n - 3] == b'%' && b[n - 2..].iter().all(u8::is_ascii_hexdigit);
    let esc = n >= 2 && b[n - 2] == b'\\' && matches!(b[n - 1], b'n' | b't' | b'r');
    pct || esc
}

/// Mask every `sk-ar-v1-…` and `ak_…` token in free text (error messages and
/// echoed server bodies). A token runs from its marker to the first character
/// outside `[A-Za-z0-9_-]`. `sk-ar-v1-` is long enough to match anywhere; the
/// short `ak_` must not be glued to a preceding word character (`break_even`),
/// except after an escaped separator (`%20`, a literal `\n`, `\t` or `\r`).
/// Tokens too short to hide behind a prefix keep only the prefix.
pub fn mask_secrets(text: &str) -> String {
    const MARKERS: [&str; 2] = ["sk-ar-v1-", "ak_"];
    let is_token_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut prev_word = false;
    while let Some(c) = rest.chars().next() {
        let long_marker = rest.starts_with(MARKERS[0]);
        let short_ok = rest.starts_with(MARKERS[1]) && (!prev_word || after_escape(&out));
        if long_marker || short_ok {
            let end = rest.find(|c: char| !is_token_char(c)).unwrap_or(rest.len());
            let token = &rest[..end];
            let prefix_len = if token.starts_with("sk-ar-v1-") {
                14
            } else {
                8
            };
            if token.chars().count() > prefix_len + 4 {
                out.push_str(&mask_api_key(Some(token)));
            } else {
                out.extend(token.chars().take(prefix_len));
                out.push('…');
            }
            rest = &rest[end..];
            prev_word = true;
            continue;
        }
        prev_word = is_token_char(c);
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Exits 4 (auth) through `commands::run`.
pub fn no_key_error() -> String {
    let bin = crate::help::invoked_bin();
    crate::exit::tag(
        crate::exit::AUTH,
        format!(
            "not signed in, no AnyRouter key found\n{} run `{bin} login` (add --device over SSH), \
or set ANYROUTER_API_KEY",
            crate::term::dim("hint:")
        ),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn dash_key_reads_stdin_so_the_secret_never_hits_argv() {
        let mut flags = HashMap::from([("key".to_string(), FlagValue::Value("-".into()))]);
        let mut warn = Vec::new();
        normalize_key_flag(&mut flags, &mut "sk-ar-v1-real\n".as_bytes(), &mut warn).unwrap();
        assert_eq!(
            get_string_flag(&flags, "key").as_deref(),
            Some("sk-ar-v1-real")
        );
        assert!(warn.is_empty(), "stdin keys must not warn");
    }

    #[test]
    fn literal_key_warns_that_argv_is_visible() {
        let mut flags =
            HashMap::from([("key".to_string(), FlagValue::Value("sk-ar-v1-real".into()))]);
        let mut warn = Vec::new();
        normalize_key_flag(&mut flags, &mut "".as_bytes(), &mut warn).unwrap();
        assert_eq!(
            get_string_flag(&flags, "key").as_deref(),
            Some("sk-ar-v1-real")
        );
        assert!(String::from_utf8(warn).unwrap().contains("process list"));
    }

    #[test]
    fn empty_stdin_for_dash_key_is_an_error_not_a_literal_dash() {
        let mut flags = HashMap::from([("key".to_string(), FlagValue::Value("-".into()))]);
        assert!(normalize_key_flag(&mut flags, &mut "".as_bytes(), &mut Vec::new()).is_err());
    }

    #[test]
    fn mask_secrets_hides_keys_wherever_they_sit() {
        let k = "sk-ar-v1-abcdefghijklmnop1234";
        let m = mask_api_key(Some(k));
        assert_eq!(
            mask_secrets(&format!("bad key {k}")),
            format!("bad key {m}")
        );
        assert_eq!(
            mask_secrets(&format!("{k} and {k}")),
            format!("{m} and {m}")
        );
        assert_eq!(
            mask_secrets(&format!("\"{k}\" '{k}'")),
            format!("\"{m}\" '{m}'")
        );
        assert_eq!(mask_secrets(&format!("x{k}")), format!("x{m}"));
        assert_eq!(mask_secrets(&format!("line\n{k}")), format!("line\n{m}"));
        assert_eq!(mask_secrets(&format!("line\\n{k}")), format!("line\\n{m}"));
    }

    #[test]
    fn mask_secrets_ak_keys_respect_word_boundaries() {
        let k = "ak_abcdefghijklmnop1234";
        let m = mask_api_key(Some(k));
        assert_eq!(
            mask_secrets("break_even and sneak_peek"),
            "break_even and sneak_peek"
        );
        assert_eq!(mask_secrets(&format!("q=%20{k}")), format!("q=%20{m}"));
        assert_eq!(mask_secrets(&format!("a\t{k}")), format!("a\t{m}"));
        assert!(!mask_secrets(&format!("key: {k}")).contains("abcdefghijklmnop"));
    }

    #[test]
    fn mask_secrets_keeps_non_ascii_text_intact() {
        let k = "sk-ar-v1-abcdefghijklmnop1234";
        let m = mask_api_key(Some(k));
        assert_eq!(
            mask_secrets(&format!("lỗi 🔑 {k}é日本語")),
            format!("lỗi 🔑 {m}é日本語")
        );
        assert_eq!(mask_secrets("日本語 ak_"), "日本語 ak_…");
    }

    use super::*;
    use crate::parse::FlagValue;

    fn profile_with_key(key: &str) -> Profile {
        Profile {
            api_key: Some(key.into()),
            ..Default::default()
        }
    }

    #[test]
    fn dummy_literals_are_detected_trimmed_and_case_insensitive() {
        for dummy in DUMMY_KEYS {
            assert!(is_dummy_key(dummy), "{dummy}");
            assert!(is_dummy_key(&format!("  {dummy}  ")), "{dummy}");
            assert!(is_dummy_key(&dummy.to_ascii_uppercase()), "{dummy}");
        }
        assert!(!is_dummy_key("sk-ar-v1-test-extra"));
        assert!(!is_dummy_key("sk-ar-v1-real0123456789"));
    }

    #[test]
    fn each_dummy_literal_resolves_to_none() {
        let env = BTreeMap::new();
        let mut flags = std::collections::HashMap::new();
        for dummy in DUMMY_KEYS {
            // Stored in the profile.
            let profile = profile_with_key(dummy);
            assert_eq!(
                resolve_api_key(&flags, &env, Some(&profile)),
                None,
                "{dummy}"
            );
            // Passed via --key.
            flags.insert("key".into(), FlagValue::Value(dummy.to_string()));
            assert_eq!(resolve_api_key(&flags, &env, None), None, "{dummy}");
            flags.clear();
            // Exported via ANYROUTER_API_KEY.
            let mut with_env = BTreeMap::new();
            with_env.insert("ANYROUTER_API_KEY".into(), dummy.to_string());
            assert_eq!(resolve_api_key(&flags, &with_env, None), None, "{dummy}");
        }
    }

    #[test]
    fn real_keys_still_resolve_from_every_source() {
        let real = "sk-ar-v1-real0123456789abcdef";
        let mut flags =
            std::collections::HashMap::from([("key".into(), FlagValue::Value(real.to_string()))]);
        assert_eq!(
            resolve_api_key(&flags, &BTreeMap::new(), None).as_deref(),
            Some(real)
        );
        flags.clear();
        let mut env = BTreeMap::new();
        env.insert("ANYROUTER_API_KEY".into(), real.to_string());
        assert_eq!(resolve_api_key(&flags, &env, None).as_deref(), Some(real));
        let profile = profile_with_key(real);
        assert_eq!(
            resolve_api_key(&flags, &BTreeMap::new(), Some(&profile)).as_deref(),
            Some(real)
        );
    }

    #[test]
    fn dummy_env_falls_through_to_real_profile_key() {
        let mut env = BTreeMap::new();
        env.insert("ANYROUTER_API_KEY".into(), "SK-AR-V1-TESTKEY".into());
        let profile = profile_with_key("sk-ar-v1-real0123456789");
        let flags = std::collections::HashMap::new();
        assert_eq!(
            resolve_api_key(&flags, &env, Some(&profile)).as_deref(),
            Some("sk-ar-v1-real0123456789")
        );
    }

    fn cfg_two_agents() -> Config {
        crate::config::parse_config(
            "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-default-key-aaaa
    default_model: auto
  work:
    api_key: sk-ar-v1-work-key-bbbb
    default_model: anthropic/claude-sonnet-4.6
agents:
  claude:
    api_key: sk-ar-v1-claude-key-cccc
    default_model: stealth/ox-alpha
  grok:
    profile: work
",
        )
    }

    fn launch(
        cfg: &Config,
        flags: &[(&str, &str)],
        env: &[(&str, &str)],
        tool: &str,
    ) -> Option<(String, KeySource)> {
        let flags: HashMap<String, FlagValue> = flags
            .iter()
            .map(|(k, v)| (k.to_string(), FlagValue::Value(v.to_string())))
            .collect();
        let env: BTreeMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        resolve_launch_key(&flags, &env, Some(cfg), tool)
    }

    #[test]
    fn launch_key_uses_agent_key_then_agent_account_then_active_account() {
        let cfg = cfg_two_agents();
        assert_eq!(
            launch(&cfg, &[], &[], "claude"),
            Some(("sk-ar-v1-claude-key-cccc".into(), KeySource::AgentKey))
        );
        assert_eq!(
            launch(&cfg, &[], &[], "grok"),
            Some((
                "sk-ar-v1-work-key-bbbb".into(),
                KeySource::AgentAccount("work".into())
            ))
        );
        assert_eq!(
            launch(&cfg, &[], &[], "codex"),
            Some((
                "sk-ar-v1-default-key-aaaa".into(),
                KeySource::Account("default".into())
            ))
        );
    }

    #[test]
    fn launch_key_flag_and_env_beat_every_agent_binding() {
        let cfg = cfg_two_agents();
        let flag = "sk-ar-v1-from-the-flag-dddd";
        let env = "sk-ar-v1-from-the-env-eeee";
        for tool in ["claude", "grok", "codex"] {
            assert_eq!(
                launch(&cfg, &[("key", flag)], &[("ANYROUTER_API_KEY", env)], tool),
                Some((flag.into(), KeySource::Flag))
            );
            assert_eq!(
                launch(&cfg, &[], &[("ANYROUTER_API_KEY", env)], tool),
                Some((env.into(), KeySource::Env))
            );
        }
    }

    #[test]
    fn launch_key_explicit_profile_beats_agent_account_but_not_agent_key() {
        // WHY: `--profile` names an account for this run, so it overrides the
        // agent's bound account. An agent key is narrower still and wins.
        let cfg = cfg_two_agents();
        assert_eq!(
            launch(&cfg, &[("profile", "default")], &[], "grok"),
            Some((
                "sk-ar-v1-default-key-aaaa".into(),
                KeySource::Account("default".into())
            ))
        );
        assert_eq!(
            launch(&cfg, &[], &[("ANYROUTER_PROFILE", "work")], "codex"),
            Some((
                "sk-ar-v1-work-key-bbbb".into(),
                KeySource::Account("work".into())
            ))
        );
        assert_eq!(
            launch(&cfg, &[("profile", "work")], &[], "claude"),
            Some(("sk-ar-v1-claude-key-cccc".into(), KeySource::AgentKey))
        );
    }

    #[test]
    fn launch_key_skips_a_dummy_agent_key() {
        let mut cfg = cfg_two_agents();
        cfg.agent_binding_mut("claude").api_key = Some("sk-ar-v1-test".into());
        assert_eq!(
            launch(&cfg, &[], &[], "claude"),
            Some((
                "sk-ar-v1-default-key-aaaa".into(),
                KeySource::Account("default".into())
            ))
        );
    }

    #[test]
    fn mask_api_key_never_shows_a_short_key_whole() {
        assert_eq!(mask_api_key(Some("sk-ar-v1-abc")), "sk-ar-v1-…");
        assert_eq!(mask_api_key(Some("sk-ar-v1-abcdefgh1234")), "sk-ar-v1-…");
        assert_eq!(mask_api_key(Some("ak_short12345")), "ak_…");
        assert_eq!(mask_api_key(Some("short-secret")), "…");
        assert_eq!(
            mask_api_key(Some("sk-ar-v1-abcdefghijklmnop1234")),
            "sk-ar-v1-abcde…1234"
        );
    }

    #[test]
    fn launch_model_uses_agent_id_then_profile_default() {
        let cfg = cfg_two_agents();
        let flags = HashMap::new();
        let claude_profile = profile_for_agent(&cfg, &flags, &BTreeMap::new(), "claude").unwrap();
        let grok_profile = profile_for_agent(&cfg, &flags, &BTreeMap::new(), "grok").unwrap();
        assert_eq!(
            resolve_launch_model(&flags, Some(&cfg), claude_profile, "claude"),
            "stealth/ox-alpha"
        );
        assert_eq!(
            resolve_launch_model(&flags, Some(&cfg), grok_profile, "grok"),
            "anthropic/claude-sonnet-4.6"
        );
        let mut with_flag = HashMap::new();
        with_flag.insert(
            "model".into(),
            FlagValue::Value("stealth/ox-alpha[1m]".into()),
        );
        assert_eq!(
            resolve_launch_model(&with_flag, Some(&cfg), claude_profile, "claude"),
            "stealth/ox-alpha"
        );
        let mut auto_flag = HashMap::new();
        auto_flag.insert("model".into(), FlagValue::Value("auto".into()));
        assert_eq!(
            resolve_launch_model(&auto_flag, Some(&cfg), claude_profile, "claude"),
            "anyrouter/auto"
        );
        let mut auto_1m = HashMap::new();
        auto_1m.insert(
            "model".into(),
            FlagValue::Value("anyrouter/auto[1m]".into()),
        );
        assert_eq!(
            resolve_launch_model(&auto_1m, Some(&cfg), claude_profile, "claude"),
            "anyrouter/auto[1m]"
        );
        let mut free_1m = HashMap::new();
        free_1m.insert(
            "model".into(),
            FlagValue::Value("anyrouter/free[1m]".into()),
        );
        assert_eq!(
            resolve_launch_model(&free_1m, Some(&cfg), claude_profile, "claude"),
            "anyrouter/free[1m]"
        );
        let empty = Profile::default();
        assert_eq!(
            resolve_launch_model(&flags, None, &empty, "claude"),
            "anyrouter/auto"
        );
    }
}
