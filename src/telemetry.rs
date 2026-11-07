//! Anonymous install and update ping to PostHog.
//!
//! Sent at most once per install and once per version change, so a count of
//! `cli_install` and `cli_update` events answers "how many installs, on which
//! versions". It carries a random install id, the version, the OS and the CPU
//! architecture. It never carries a key, a path, a hostname, an IP address or a
//! prompt, and it asks PostHog not to build a person profile or look up a
//! location.
//!
//! On by default. Off with `DO_NOT_TRACK=1`, `ANYR_NO_TELEMETRY=1` or
//! `telemetry: false` in the config file. It also stays off in CI, in dev
//! builds and when `ANYR_NO_UPDATE` is set (the switch every test uses), and
//! nothing is written while it is off, not even an install id.

use std::collections::BTreeMap;

use serde_json::{json, Value};
#[cfg(feature = "native")]
use std::fs;
#[cfg(feature = "native")]
use std::path::PathBuf;

use crate::autoupdate::env_flag;

/// Project 392700 public project key, the one the website already ships.
pub const POSTHOG_KEY: &str = "phc_zDtvVmy8TQ257DSQ9kRm54s4bmUw8PvUnXCa9LhmhYNZ";
pub const POSTHOG_CAPTURE_URL: &str = "https://us.i.posthog.com/i/v0/e/";
#[cfg(feature = "native")]
const STATE_FILE: &str = "telemetry.json";
#[cfg(feature = "native")]
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

pub const NOTICE: &str = "anyr sends one anonymous ping per install or update (random id, version, OS, CPU). Turn it off with ANYR_NO_TELEMETRY=1, DO_NOT_TRACK=1 or `telemetry: false` in the config.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Install,
    Update { from: String },
}

impl Kind {
    pub fn event(&self) -> &'static str {
        match self {
            Kind::Install => "cli_install",
            Kind::Update { .. } => "cli_update",
        }
    }
}

/// What was last recorded on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub install_id: Option<String>,
    pub last_version: Option<String>,
}

impl State {
    pub fn from_json(raw: &str) -> Self {
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
            return Self::default();
        };
        let get = |k: &str| {
            map.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Self {
            install_id: get("install_id"),
            last_version: get("last_version"),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "install_id": self.install_id, "last_version": self.last_version })
    }
}

/// Which event, if any, this run owes. A state with no id is a fresh install;
/// a different version is an update; the same version sends nothing.
pub fn decide(state: &State, version: &str) -> Option<Kind> {
    match (&state.install_id, &state.last_version) {
        (Some(_), Some(last)) if last == version => None,
        (Some(_), Some(last)) => Some(Kind::Update { from: last.clone() }),
        _ => Some(Kind::Install),
    }
}

/// True when the ping may be sent. `config_flag` is the file's `telemetry:`.
pub fn enabled(env: &BTreeMap<String, String>, config_flag: Option<bool>, dev_build: bool) -> bool {
    let on = |key: &str| env_flag(env, key) == Some(true);
    if on("DO_NOT_TRACK")
        || on("ANYR_NO_TELEMETRY")
        || on("ANYR_NO_UPDATE")
        || dev_build
        || env
            .get("CI")
            .is_some_and(|v| !v.trim().is_empty() && env_flag(env, "CI") != Some(false))
    {
        return false;
    }
    config_flag.unwrap_or(true)
}

/// The exact JSON sent to PostHog. Nothing else leaves the machine.
pub fn payload(install_id: &str, kind: &Kind, version: &str, os: &str, arch: &str) -> Value {
    let mut props = json!({
        "$lib": "anyr-cli",
        "$process_person_profile": false,
        "$geoip_disable": true,
        "version": version,
        "os": os,
        "arch": arch,
        "channel": if version.contains("beta") { "beta" } else { "stable" },
    });
    if let Kind::Update { from } = kind {
        props["from_version"] = json!(from);
    }
    json!({
        "api_key": POSTHOG_KEY,
        "event": kind.event(),
        "distinct_id": install_id,
        "properties": props,
    })
}

/// 32 hex chars from the OS random source, mixed with the clock and pid so a
/// missing `/dev/urandom` (Windows) still yields a distinct id per install.
#[cfg(feature = "native")]
fn new_install_id() -> String {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut h = Sha256::new();
    let mut buf = [0u8; 32];
    if fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok()
    {
        h.update(buf);
    }
    h.update(crate::autoupdate::now_secs().to_le_bytes());
    h.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    h.update(std::process::id().to_le_bytes());
    h.finalize()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(feature = "native")]
fn state_path(env: &BTreeMap<String, String>) -> PathBuf {
    crate::autoupdate::state_dir(env).join(STATE_FILE)
}

#[cfg(feature = "native")]
fn save(env: &BTreeMap<String, String>, state: &State) {
    let dir = crate::autoupdate::state_dir(env);
    let _ = crate::config::ensure_private_dir(&dir);
    let body = format!("{}\n", state.to_json());
    let _ = crate::config::open_private(&state_path(env), true).and_then(|mut f| {
        use std::io::Write;
        f.write_all(body.as_bytes())
    });
}

/// A cargo build, or any process started by `cargo test` / `cargo run`. Tests
/// copy the binary out of `target/` (the self-update tests do), so the path
/// alone is not enough: cargo also leaves `CARGO_MANIFEST_DIR` in the env of
/// everything it spawns.
pub fn is_dev_run(exe_in_target: bool, env: &BTreeMap<String, String>) -> bool {
    exe_in_target || env.contains_key("CARGO_MANIFEST_DIR")
}

/// `ANYR_TELEMETRY_URL` points the ping at another endpoint (a test server).
#[cfg(feature = "native")]
fn endpoint(env: &BTreeMap<String, String>) -> Option<String> {
    env.get("ANYR_TELEMETRY_URL")
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
}

#[cfg(feature = "native")]
fn send(url: &str, body: &Value) {
    let agent = ureq::AgentBuilder::new()
        .timeout(SEND_TIMEOUT)
        .user_agent(&format!("anyr-cli/{}", crate::VERSION))
        .build();
    let _ = agent
        .post(url)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string());
}

/// Runs before every command. Never fails and never waits more than the send
/// timeout. State is saved before the send, so a failed send is lost rather
/// than retried on every run.
#[cfg(feature = "native")]
pub fn on_startup(env: &BTreeMap<String, String>) {
    let config_flag =
        crate::key::load_config_if_present(&crate::config::resolve_config_path(None, env))
            .and_then(|c| c.telemetry);
    let dev_build = is_dev_run(
        std::env::current_exe()
            .map(|exe| crate::autoupdate::is_cargo_target_build(&exe))
            .unwrap_or(false),
        env,
    );
    let custom = endpoint(env);
    // A cargo build only stays quiet toward the real PostHog endpoint.
    if !enabled(env, config_flag, dev_build && custom.is_none())
        || env.contains_key(crate::autoupdate::ENV_AUTO_CHILD)
        || env.contains_key(crate::autoupdate::ENV_LAUNCHER_CHILD)
    {
        return;
    }
    let mut state = fs::read_to_string(state_path(env))
        .ok()
        .map(|raw| State::from_json(&raw))
        .unwrap_or_default();
    let Some(kind) = decide(&state, crate::VERSION) else {
        return;
    };
    let fresh = state.install_id.is_none();
    let id = state.install_id.clone().unwrap_or_else(new_install_id);
    state.install_id = Some(id.clone());
    state.last_version = Some(crate::VERSION.to_string());
    save(env, &state);
    if fresh {
        use std::io::IsTerminal;
        if std::io::stderr().is_terminal() {
            eprintln!("{}", crate::term::dim(NOTICE));
        }
    }
    send(
        custom.as_deref().unwrap_or(POSTHOG_CAPTURE_URL),
        &payload(
            &id,
            &kind,
            crate::VERSION,
            std::env::consts::OS,
            std::env::consts::ARCH,
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn on_by_default_for_a_release_build() {
        assert!(enabled(&env(&[]), None, false));
    }

    #[test]
    fn every_opt_out_turns_it_off() {
        for e in [
            env(&[("DO_NOT_TRACK", "1")]),
            env(&[("ANYR_NO_TELEMETRY", "1")]),
            env(&[("ANYR_NO_UPDATE", "1")]),
            env(&[("CI", "true")]),
        ] {
            assert!(!enabled(&e, None, false), "{e:?}");
        }
        assert!(!enabled(&env(&[]), Some(false), false), "config flag");
        assert!(!enabled(&env(&[]), None, true), "dev build");
    }

    #[test]
    fn a_binary_copied_out_of_target_under_cargo_test_is_still_a_dev_run() {
        assert!(is_dev_run(true, &env(&[])));
        assert!(is_dev_run(
            false,
            &env(&[("CARGO_MANIFEST_DIR", "/src/cli")])
        ));
        assert!(!is_dev_run(false, &env(&[])));
    }

    #[test]
    fn decides_install_update_or_nothing() {
        let fresh = State::default();
        assert_eq!(decide(&fresh, "0.1.40"), Some(Kind::Install));
        let seen = State {
            install_id: Some("abc".into()),
            last_version: Some("0.1.39".into()),
        };
        assert_eq!(
            decide(&seen, "0.1.40"),
            Some(Kind::Update {
                from: "0.1.39".into()
            })
        );
        assert_eq!(decide(&seen, "0.1.39"), None, "same version sends nothing");
        let no_id = State {
            install_id: None,
            last_version: Some("0.1.39".into()),
        };
        assert_eq!(decide(&no_id, "0.1.39"), Some(Kind::Install));
    }

    #[test]
    fn payload_holds_only_the_promised_fields() {
        let body = payload(
            "abc",
            &Kind::Update {
                from: "0.1.39".into(),
            },
            "0.1.40-beta.7",
            "linux",
            "x86_64",
        );
        let top: Vec<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(top, ["api_key", "distinct_id", "event", "properties"]);
        let mut props: Vec<&str> = body["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        props.sort_unstable();
        assert_eq!(
            props,
            [
                "$geoip_disable",
                "$lib",
                "$process_person_profile",
                "arch",
                "channel",
                "from_version",
                "os",
                "version"
            ]
        );
        assert_eq!(body["event"], "cli_update");
        assert_eq!(body["properties"]["$process_person_profile"], false);
        assert_eq!(body["properties"]["channel"], "beta");
    }

    #[test]
    fn state_roundtrips_and_a_bad_file_reads_as_fresh() {
        let s = State {
            install_id: Some("abc".into()),
            last_version: Some("1".into()),
        };
        assert_eq!(State::from_json(&s.to_json().to_string()), s);
        assert_eq!(State::from_json("not json"), State::default());
    }
}
