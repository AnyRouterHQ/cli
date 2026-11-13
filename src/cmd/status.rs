//! `anyr whoami` / `anyr status` / `anyr auth status`: who is signed in, what
//! they can spend, which key is active, plus a small action picker on a TTY.

use std::collections::BTreeMap;
use std::path::Path;

use crate::cmd::account::run_logout;
use crate::cmd::auth::run_auth_switch;
use crate::cmd::dispatch::{config_path, hint};
use crate::cmd::models::{models_for_picker, pick_model, save_profile_model_slot};
use crate::config::{Config, Profile};
use crate::http::{fetch_key_within, fetch_me_within, format_usd, KeyInfo, MeInfo};
use crate::key::{
    config_for_credential, load_config_if_present, mask_api_key, profile_or_default,
    resolve_api_key, resolve_base_url, resolve_launch_key, KeySource,
};
use crate::parse::{get_string_flag, ParsedArgs};
use crate::spawn::{catalog_model_id, display_model_id, session_model_label};
use crate::term;

const DASHBOARD_URL: &str = "https://anyrouter.dev/dashboard";

/// Account lookups must not hang an offline `status`: it answers within 5 s
/// even when the gateway never replies. Measured on prod, `/v1/key` answers in
/// about 0.25 s warm and 1 s cold, so 4 s leaves room for a slow first
/// DNS + TLS handshake that overran the old 3 s.
const IDENTITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

/// One agent's launch credential as `anyr <agent>` would resolve it now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AgentKeyRow {
    pub agent: String,
    /// `None` when nothing resolves (a bound account with no key).
    pub source: Option<KeySource>,
    pub key_masked: String,
}

impl AgentKeyRow {
    fn text(&self) -> String {
        match &self.source {
            Some(source) => format!("{} · {}", self.key_masked, source.label()),
            None => "no key".into(),
        }
    }
}

/// Agents that carry their own key or account binding, resolved the same way
/// launch resolves them.
pub(crate) fn agent_key_rows(
    cfg: &Config,
    flags: &std::collections::HashMap<String, crate::parse::FlagValue>,
    env: &BTreeMap<String, String>,
) -> Vec<AgentKeyRow> {
    let set = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
    cfg.agents
        .iter()
        .filter(|(_, b)| set(&b.api_key) || set(&b.profile))
        .map(|(agent, _)| {
            let resolved = resolve_launch_key(flags, env, Some(cfg), agent);
            AgentKeyRow {
                agent: agent.clone(),
                key_masked: mask_api_key(resolved.as_ref().map(|(k, _)| k.as_str())),
                source: resolved.map(|(_, source)| source),
            }
        })
        .collect()
}

/// What the status view shows. Model tier mappings stay in `config`; they are
/// not part of this summary.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct StatusView {
    pub account: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub username: Option<String>,
    pub balance: Option<f64>,
    /// Plan name from `/me`, when the server sends one.
    pub plan_name: Option<String>,
    /// Why name and email are missing, when they are.
    pub identity_note: Option<String>,
    /// Set only when the account lookup itself failed.
    pub identity_error: Option<String>,
    pub key: Option<KeyInfo>,
    /// Set only when the key lookup failed.
    pub key_error: Option<String>,
    pub key_masked: String,
    pub default_model: String,
    pub default_agent: Option<String>,
    /// Agents with their own key or account, and the key each launches with.
    pub agents: Vec<AgentKeyRow>,
    pub config_path: String,
    pub base_url: String,
}

impl StatusView {
    pub(crate) fn identity_line(&self) -> String {
        match (self.name.as_deref(), self.email.as_deref()) {
            (Some(n), Some(e)) if !n.eq_ignore_ascii_case(e) => format!("{n} · {e}"),
            (Some(n), _) => n.to_string(),
            (None, Some(e)) => e.to_string(),
            (None, None) => self
                .identity_note
                .clone()
                .unwrap_or_else(|| "name and email unavailable".into()),
        }
    }

    /// The plan name from `/me` when the server sends it, else `free` or
    /// `paid` from the key's `is_free_tier`. `None` when neither answered.
    pub(crate) fn plan(&self) -> Option<String> {
        self.plan_name.clone().or_else(|| {
            self.key
                .as_ref()
                .and_then(|k| k.is_free_tier)
                .map(|free| if free { "free" } else { "paid" }.to_string())
        })
    }

    pub(crate) fn json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "signed_in": true,
            "account": self.account,
            "user": { "name": self.name, "email": self.email, "username": self.username },
            "credits": self.balance,
            "plan": self.plan(),
            "key_masked": self.key_masked,
            "key": self.key.as_ref().map(|k| serde_json::json!({
                "name": k.name,
                "last_used_at": k.last_used_at,
                "allowed_endpoints": k.allowed_endpoints,
                "allowed_models": k.allowed_models,
            })),
            "default_model": self.default_model,
            "default_agent": self.default_agent,
            "agents": self.agents.iter().map(|a| serde_json::json!({
                "agent": a.agent,
                "key_masked": a.key_masked,
                "source": a.source.as_ref().map(KeySource::code),
                "account": a.source.as_ref().and_then(KeySource::account),
            })).collect::<Vec<_>>(),
            "config_path": self.config_path,
            "base_url": self.base_url,
        });
        if let Some(err) = &self.identity_error {
            v["identity_error"] = serde_json::json!(err);
        }
        if let Some(err) = &self.key_error {
            v["key_error"] = serde_json::json!(err);
        }
        v
    }

    /// Plain label/value lines from column 0, cut to the terminal width.
    /// First byte is visible text.
    pub(crate) fn render(&self) -> String {
        let dash = || "—".to_string();
        let identity = if self.name.is_some() || self.email.is_some() {
            self.identity_line()
        } else {
            format!("{} ({})", self.account, self.identity_line())
        };
        let mut account = vec![
            ("account", identity),
            ("credits", self.balance.map(format_usd).unwrap_or_else(dash)),
        ];
        // An unknown plan is left out: a bare dash read as "no plan".
        if let Some(plan) = self.plan() {
            account.push(("plan", plan));
        }
        let mut key_rows = vec![(
            "key",
            match self.key.as_ref().and_then(|k| k.name.as_deref()) {
                Some(name) => format!("{name} · {}", self.key_masked),
                None => self.key_masked.clone(),
            },
        )];
        if let Some(k) = &self.key {
            key_rows.push((
                "scopes",
                k.allowed_endpoints
                    .as_ref()
                    .map(|e| e.join(", "))
                    .unwrap_or_else(|| "all endpoints".into()),
            ));
            if let Some(models) = &k.allowed_models {
                key_rows.push(("models", models.join(", ")));
            }
            key_rows.push((
                "last use",
                k.last_used_at
                    .as_deref()
                    .map(short_time)
                    .unwrap_or_else(|| "never".into()),
            ));
        } else if let Some(err) = &self.key_error {
            key_rows.push(("scopes", format!("unavailable ({})", short_reason(err))));
        }
        key_rows.push(("profile", self.account.clone()));
        key_rows.push(("model", session_model_label(&self.default_model)));
        if let Some(agent) = &self.default_agent {
            key_rows.push(("agent", agent.clone()));
        }
        key_rows.push(("base url", self.base_url.clone()));
        key_rows.push(("config", self.config_path.clone()));
        let agent_rows: Vec<(&str, String)> = self
            .agents
            .iter()
            .map(|a| (a.agent.as_str(), a.text()))
            .collect();
        if agent_rows.is_empty() {
            term::label_rows(&[&account, &key_rows])
        } else {
            term::label_rows(&[&account, &key_rows, &agent_rows])
        }
    }
}

/// A failed lookup in a few words for a status row: `timed out`, `offline`,
/// `HTTP 500`. The full error stays in `--json` and `--verbose`.
pub(crate) fn short_reason(err: &str) -> String {
    let lower = err.to_ascii_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        return "timed out".into();
    }
    if let Some(at) = err.find("HTTP ") {
        let code: String = err[at + 5..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if !code.is_empty() {
            return format!("HTTP {code}");
        }
    }
    if lower.contains("restricted key") {
        return "key lacks this scope".into();
    }
    if lower.starts_with("could not reach") {
        return "offline".into();
    }
    // Anything else: its first clause, which is the subject of the error.
    let first = err.split([':', '.']).next().unwrap_or(err).trim();
    if first.is_empty() {
        "error".into()
    } else {
        first.to_string()
    }
}

/// `2026-10-27T15:31:41Z` → `2026-10-27 15:31 UTC`; anything else unchanged.
fn short_time(ts: &str) -> String {
    match (ts.get(..10), ts.get(11..16)) {
        (Some(day), Some(hm)) if ts.ends_with('Z') && ts.as_bytes()[10] == b'T' => {
            format!("{day} {hm} UTC")
        }
        _ => ts.to_string(),
    }
}

/// What a signed-out `status` prints: one line, then the login hint.
pub(crate) fn render_signed_out() -> String {
    format!(
        "Not signed in.\n{}\n",
        term::dim(&format!(
            "Run `{} auth login` to sign in.",
            crate::help::invoked_bin()
        ))
    )
}

type Identity = (Option<String>, Option<String>, Option<String>);

/// (name, email, note on why identity is missing) from the account lookup.
pub(crate) fn identity_from_me(me: Result<MeInfo, String>) -> Identity {
    match me {
        Ok(me) => {
            let name = me
                .name
                .filter(|s| !s.is_empty())
                .or(me.username.filter(|s| !s.is_empty()));
            let email = me.email.filter(|s| !s.is_empty());
            let note = (name.is_none() && email.is_none())
                .then(|| "this key has no name or email on file".to_string());
            (name, email, note)
        }
        Err(err) => (
            None,
            None,
            Some(format!(
                "name and email unavailable ({})",
                short_reason(&err)
            )),
        ),
    }
}

/// The account this command targets: `--profile`, then `ANYROUTER_PROFILE`,
/// then the config's active profile. Every status action uses this one result.
fn resolve_account(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
) -> Result<(Config, String, Profile), String> {
    let cfg = config_for_credential(path, &parsed.flags, env)?;
    let account = get_string_flag(&parsed.flags, "profile")
        .or_else(|| env.get("ANYROUTER_PROFILE").cloned())
        .unwrap_or_else(|| cfg.active_profile.clone());
    let profile = profile_or_default(&cfg, &account, &parsed.flags, env)?;
    Ok((cfg, account, profile))
}

fn build_view(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
) -> Result<StatusView, String> {
    let (cfg, account, profile) = resolve_account(parsed, env, path)?;
    let key = resolve_api_key(&parsed.flags, env, Some(&profile)).ok_or_else(|| {
        crate::exit::tag(crate::exit::AUTH, hint("Not signed in. Run: {bin} login"))
    })?;
    let base_url = resolve_base_url(&parsed.flags, Some(&profile));
    let (me, key_info) = std::thread::scope(|s| {
        let key_info = s.spawn(|| fetch_key_within(&base_url, &key, IDENTITY_TIMEOUT));
        let me = fetch_me_within(&base_url, &key, IDENTITY_TIMEOUT);
        let key_info = key_info
            .join()
            .unwrap_or_else(|_| Err("key lookup panicked".into()));
        (me, key_info)
    });
    let me = me.map_err(|err| crate::exit::classify(&err).1);
    let key_info = key_info.map_err(|err| crate::exit::classify(&err).1);
    let identity_error = me.as_ref().err().cloned();
    let username = me.as_ref().ok().and_then(|m| m.username.clone());
    let balance = me.as_ref().ok().and_then(|m| m.balance);
    let plan_name = me.as_ref().ok().and_then(|m| m.plan.clone());
    let (name, email, identity_note) = identity_from_me(me);
    Ok(StatusView {
        account,
        name,
        email,
        username,
        balance,
        plan_name,
        identity_note,
        identity_error,
        key_error: key_info.as_ref().err().cloned(),
        key: key_info.ok(),
        key_masked: mask_api_key(Some(&key)),
        default_model: display_model_id(profile.default_model()),
        default_agent: cfg.preferred_agent(get_string_flag(&parsed.flags, "tool").as_deref()),
        agents: agent_key_rows(&cfg, &parsed.flags, env),
        config_path: path.display().to_string(),
        base_url,
    })
}

pub(crate) fn run_whoami(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let path = config_path(parsed, env);
    let json = parsed.flag_true("json");
    let view = match build_view(parsed, env, &path) {
        Ok(view) => view,
        // A missing key is an answer, not a failure: say so on stdout and
        // keep exit 4 so scripts can branch on it. JSON keeps the error shape.
        Err(err) if !json && crate::exit::classify(&err).0 == crate::exit::AUTH => {
            print!("{}", render_signed_out());
            return Ok(crate::exit::AUTH);
        }
        Err(err) => return Err(err),
    };
    if json {
        let mut v = view.json();
        if let Some(cfg) = load_config_if_present(&path) {
            v["profiles"] = crate::cmd::auth::profiles_json(&cfg, env);
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into())
        );
        return Ok(0);
    }
    print!("{}", view.render());
    if parsed.flag_true("verbose") {
        for (what, err) in [
            ("account lookup", &view.identity_error),
            ("key lookup", &view.key_error),
        ] {
            if let Some(err) = err {
                eprintln!("{}", term::dim(&format!("{what}: {err}")));
            }
        }
    }
    if term::machine_output(parsed, env) || !term::is_interactive() {
        return Ok(0);
    }
    status_picker(parsed, env, &path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusAction {
    SwitchAccount,
    ChangeModel,
    Logout,
    Dashboard,
    Quit,
}

pub(crate) const STATUS_ACTIONS: [(StatusAction, &str); 5] = [
    (StatusAction::SwitchAccount, "Switch account"),
    (StatusAction::ChangeModel, "Change default model"),
    (StatusAction::Logout, "Log out"),
    (StatusAction::Dashboard, "Open dashboard"),
    (StatusAction::Quit, "Quit"),
];

pub(crate) fn status_action_at(index: usize) -> StatusAction {
    STATUS_ACTIONS
        .get(index)
        .map(|(a, _)| *a)
        .unwrap_or(StatusAction::Quit)
}

fn status_picker(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
) -> Result<i32, String> {
    let labels: Vec<String> = STATUS_ACTIONS.iter().map(|(_, l)| l.to_string()).collect();
    loop {
        // Inline, not the full-screen picker: the alternate screen hid the
        // summary printed just above it.
        let Ok(idx) =
            term::pick_inline(&[], "", &labels, "↑↓ move · enter select · esc quit", None)
        else {
            return Ok(0);
        };
        let action = status_action_at(idx);
        if action == StatusAction::Quit {
            return Ok(0);
        }
        match run_status_action(action, parsed, env, path) {
            Ok(()) => {}
            Err(err) if err == "Cancelled." => {}
            Err(err) => eprintln!("{}", term::err(&err)),
        }
        if action == StatusAction::Logout {
            return Ok(0);
        }
        if let Ok(view) = build_view(parsed, env, path) {
            print!("\n{}", view.render());
        }
    }
}

fn run_status_action(
    action: StatusAction,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
) -> Result<(), String> {
    let bare = ParsedArgs {
        command: parsed.command.clone(),
        flags: parsed.flags.clone(),
        passthrough: Vec::new(),
        raw_from: None,
    };
    match action {
        StatusAction::SwitchAccount => run_auth_switch(&bare, env).map(|_| ()),
        StatusAction::Logout => {
            if !term::confirm("Log out of this account?") {
                return Ok(());
            }
            run_logout(&bare, env).map(|_| ())
        }
        StatusAction::Dashboard => {
            if !crate::auth::open_url(DASHBOARD_URL) {
                println!("Open {}", term::link(DASHBOARD_URL));
            }
            Ok(())
        }
        StatusAction::ChangeModel => {
            change_default_model(parsed, env, path, |profile, key, base| {
                let models = models_for_picker(path.parent(), base, key, env);
                pick_model(&models, Some(profile.default_model()), "Default model")
            })
        }
        StatusAction::Quit => Ok(()),
    }
}

/// Resolve the account once, let `pick` choose a model for it, and save the
/// choice into that same profile (not whichever one the config marks active).
fn change_default_model(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    path: &Path,
    pick: impl FnOnce(&Profile, Option<&str>, &str) -> Result<String, String>,
) -> Result<(), String> {
    let (_, account, profile) = resolve_account(parsed, env, path)?;
    let key = resolve_api_key(&parsed.flags, env, Some(&profile));
    let base = resolve_base_url(&parsed.flags, Some(&profile));
    let id = pick(&profile, key.as_deref(), &base)?;
    save_profile_model_slot(
        load_config_if_present(path),
        path,
        Some(&account),
        "default",
        &catalog_model_id(&id),
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn view(name: Option<&str>, email: Option<&str>) -> StatusView {
        StatusView {
            account: "default".into(),
            name: name.map(Into::into),
            email: email.map(Into::into),
            identity_note: None,
            identity_error: None,
            key_masked: "sk-ar-v1-qjd4c…e5bu".into(),
            default_model: "poolside/laguna-s-2.1".into(),
            config_path: "/home/u/.anyrouter/config.yaml".into(),
            base_url: "https://anyrouter.dev/api".into(),
            ..Default::default()
        }
    }

    #[test]
    fn text_shows_name_email_and_no_tier_rows() {
        let out = view(Some("Duyet Le"), Some("me@duyet.net")).render();
        assert!(out.contains("Duyet Le"), "{out}");
        assert!(out.contains("me@duyet.net"), "{out}");
        assert!(out.contains("poolside/laguna-s-2.1"), "{out}");
        for gone in [
            "claude_",
            "default_tool",
            "haiku",
            "sonnet",
            "opus",
            "fable",
        ] {
            assert!(!out.contains(gone), "{gone} still shown:\n{out}");
        }
    }

    #[test]
    fn first_byte_is_visible_text() {
        let out = view(Some("A"), Some("a@b.c")).render();
        assert!(
            !out.starts_with('\r') && !out.starts_with('\x1b'),
            "{out:?}"
        );
        let anon = view(None, None).render();
        assert!(anon.starts_with("account"), "{anon:?}");
        assert!(render_signed_out().starts_with("Not signed in."));
    }

    #[test]
    fn rows_cover_credits_plan_key_and_scopes() {
        let mut v = view(Some("duyet"), Some("me@duyet.net"));
        v.balance = Some(133.834);
        v.key = Some(KeyInfo {
            name: Some("laptop".into()),
            is_free_tier: Some(false),
            last_used_at: Some("2026-10-27T15:31:41Z".into()),
            allowed_endpoints: Some(vec!["/v1/chat/completions".into()]),
            allowed_models: None,
        });
        let out = v.render();
        for want in [
            "account   duyet · me@duyet.net",
            "credits   $133.83",
            "plan      paid",
            "key       laptop · sk-ar-v1-qjd4c…e5bu",
            "scopes    /v1/chat/completions",
            "last use  2026-10-27 15:31 UTC",
            "base url  https://anyrouter.dev/api",
        ] {
            assert!(out.contains(want), "missing {want:?}:\n{out}");
        }
        // WHY: unrestricted keys must not print an empty models row.
        assert!(!out.contains("models"), "{out}");
        let sep = out.lines().filter(|l| l.starts_with('─')).count();
        assert_eq!(sep, 1, "one separator line:\n{out}");
    }

    #[test]
    fn failed_key_lookup_leaves_plan_unknown_and_says_why() {
        let mut v = view(Some("a"), None);
        v.key_error = Some("HTTP 500".into());
        let out = v.render();
        // WHY: a bare dash read as "no plan" on a paid account; an unknown
        // plan is left out instead.
        assert!(!out.contains("plan"), "{out}");
        assert!(out.contains("unavailable (HTTP 500)"), "{out}");
        assert_eq!(v.json()["key_error"], "HTTP 500");
        assert!(v.json()["plan"].is_null());
    }

    #[test]
    fn a_timed_out_lookup_is_one_short_row() {
        // The exact error 0.1.34 printed into the scopes row on a paid account.
        let err = "Could not reach AnyRouter: https://anyrouter.dev/api/v1/key: Network Error: Network Error: Error encountered in the status line: timed out reading response";
        let mut v = view(Some("duyet"), Some("me@duyet.net"));
        v.balance = Some(133.83);
        v.key_error = Some(err.into());
        let out = v.render();
        assert!(out.contains("scopes    unavailable (timed out)"), "{out}");
        assert!(!out.contains("Network Error"), "{out}");
        // WHY: a bare dash read as "no plan" on a paid account.
        assert!(!out.contains("plan"), "{out}");
        assert!(!out.contains('—'), "{out}");
        // The full error stays available to scripts.
        assert_eq!(v.json()["key_error"], err);
    }

    #[test]
    fn short_reason_names_the_cause_in_a_few_words() {
        for (err, want) in [
            (
                "Could not reach AnyRouter: https://x/api/v1/key: timed out",
                "timed out",
            ),
            (
                "Could not reach AnyRouter: https://x/api/v1/key: could not connect",
                "offline",
            ),
            ("Could not fetch key info (HTTP 500).", "HTTP 500"),
            (
                "Invalid /key response: expected value",
                "Invalid /key response",
            ),
        ] {
            assert_eq!(short_reason(err), want, "{err}");
        }
    }

    #[test]
    fn plan_prefers_the_server_plan_name_over_the_free_flag() {
        let mut v = view(Some("a"), None);
        v.key = Some(KeyInfo {
            is_free_tier: Some(false),
            ..Default::default()
        });
        assert_eq!(v.plan().as_deref(), Some("paid"));
        v.plan_name = Some("payg".into());
        assert_eq!(v.plan().as_deref(), Some("payg"));
        assert!(v.render().contains("plan      payg"), "{}", v.render());
    }

    #[test]
    fn missing_identity_says_why() {
        let (n, e, note) = identity_from_me(Err("HTTP 401".into()));
        assert!(n.is_none() && e.is_none());
        let mut v = view(None, None);
        v.identity_note = note;
        assert!(v.render().contains("HTTP 401"), "{}", v.render());
        let (_, _, note) = identity_from_me(Ok(MeInfo::default()));
        assert!(note.unwrap().contains("no name or email"));
    }

    #[test]
    fn username_stands_in_for_a_missing_name() {
        let me = MeInfo {
            username: Some("duyet".into()),
            email: Some("me@duyet.net".into()),
            ..Default::default()
        };
        let (n, e, _) = identity_from_me(Ok(me));
        assert_eq!(n.as_deref(), Some("duyet"));
        assert_eq!(e.as_deref(), Some("me@duyet.net"));
    }

    #[test]
    fn json_has_the_documented_shape() {
        let v = view(Some("Duyet"), Some("me@duyet.net")).json();
        assert_eq!(v["account"], "default");
        assert_eq!(v["user"]["name"], "Duyet");
        assert_eq!(v["user"]["email"], "me@duyet.net");
        assert_eq!(v["key_masked"], "sk-ar-v1-qjd4c…e5bu");
        assert_eq!(v["default_model"], "poolside/laguna-s-2.1");
        assert!(v["config_path"].is_string());
        assert!(v.get("claude_haiku").is_none() && v.get("default_tool").is_none());
    }

    #[test]
    fn json_reports_identity_error_only_when_the_lookup_failed() {
        let ok = view(Some("A"), None).json();
        assert!(ok.get("identity_error").is_none(), "{ok}");
        let mut v = view(None, None);
        v.identity_error = Some("timed out".into());
        assert_eq!(v.json()["identity_error"], "timed out");
    }

    fn two_profile_config(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("anyr-status-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(
            &path,
            "active_profile: default\nprofiles:\n  default:\n    api_key: sk-ar-v1-defaultdefaultdefault\n    default_model: a/one\n  work:\n    api_key: sk-ar-v1-workworkworkwork\n    default_model: b/two\n",
        )
        .unwrap();
        path
    }

    fn saved_default(path: &Path, profile: &str) -> String {
        load_config_if_present(path).unwrap().profiles[profile]
            .default_model()
            .to_string()
    }

    #[test]
    fn model_change_saves_to_the_profile_the_view_shows() {
        // WHY: the picker used to save into the config's active profile even
        // when --profile or ANYROUTER_PROFILE pointed the view at another one.
        for via_env in [false, true] {
            let path = two_profile_config(if via_env { "env" } else { "flag" });
            let mut parsed = ParsedArgs {
                command: "status".into(),
                flags: HashMap::new(),
                passthrough: Vec::new(),
                raw_from: None,
            };
            let mut env = BTreeMap::new();
            if via_env {
                env.insert("ANYROUTER_PROFILE".to_string(), "work".to_string());
            } else {
                parsed.flags.insert(
                    "profile".into(),
                    crate::parse::FlagValue::Value("work".into()),
                );
            }
            let mut seen = None;
            change_default_model(&parsed, &env, &path, |profile, key, _| {
                seen = Some((profile.default_model().to_string(), key.map(str::to_string)));
                Ok("c/three".to_string())
            })
            .unwrap();
            let (shown, key) = seen.unwrap();
            assert_eq!(shown, "b/two");
            assert_eq!(key.as_deref(), Some("sk-ar-v1-workworkworkwork"));
            assert_eq!(saved_default(&path, "work"), "c/three");
            assert_eq!(saved_default(&path, "default"), "a/one");
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
    }

    #[test]
    fn picker_indexes_map_to_the_documented_actions() {
        let labels: Vec<_> = STATUS_ACTIONS.iter().map(|(_, l)| *l).collect();
        assert_eq!(
            labels,
            [
                "Switch account",
                "Change default model",
                "Log out",
                "Open dashboard",
                "Quit"
            ]
        );
        assert_eq!(status_action_at(0), StatusAction::SwitchAccount);
        assert_eq!(status_action_at(1), StatusAction::ChangeModel);
        assert_eq!(status_action_at(2), StatusAction::Logout);
        assert_eq!(status_action_at(3), StatusAction::Dashboard);
        assert_eq!(status_action_at(4), StatusAction::Quit);
        assert_eq!(status_action_at(99), StatusAction::Quit);
    }
}
