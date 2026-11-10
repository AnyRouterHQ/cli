//! Anonymous install, update and daily ping to PostHog.
//!
//! Sent at most once per install, once per version change, and at most
//! once per UTC day, so a count of `cli_install`, `cli_update` and
//! `cli_daily` events answers "how many installs, on which versions, and
//! how many active users per day". It carries a random install id, the
//! version, the OS and the CPU architecture. It never carries a key, a
//! path, a hostname, an IP address, a prompt or a subcommand, and it
//! asks PostHog not to build a person profile or look up a location.
//!
//! The install and update pings send synchronously (they are rare). The
//! daily ping is handed to a detached `anyr __telemetry-daily` child that
//! reads the payload from stdin, so the first command of each day never
//! waits on the network.
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

/// Seconds in one UTC day; the daily ping fires when the day moves on.
pub const SECS_PER_DAY: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Install,
    Update { from: String },
    Daily,
}

impl Kind {
    pub fn event(&self) -> &'static str {
        match self {
            Kind::Install => "cli_install",
            Kind::Update { .. } => "cli_update",
            Kind::Daily => "cli_daily",
        }
    }
}

/// What was last recorded on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub install_id: Option<String>,
    pub last_version: Option<String>,
    /// UTC day index (`now_secs() / SECS_PER_DAY`) of the last ping.
    pub last_daily: Option<u64>,
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
            last_daily: map.get("last_daily").and_then(Value::as_u64),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "install_id": self.install_id,
            "last_version": self.last_version,
            "last_daily": self.last_daily,
        })
    }
}

/// Which event, if any, this run owes. A state with no id is a fresh install;
/// a different version is an update; the same version fires only the daily
/// ping, and only when the UTC day moved on (an install or update sent today
/// already proves the machine was active, so it suppresses the daily ping).
pub fn decide(state: &State, version: &str, today: u64) -> Option<Kind> {
    match (&state.install_id, &state.last_version) {
        (Some(_), Some(last)) if last == version => match state.last_daily {
            Some(day) if day >= today => None,
            _ => Some(Kind::Daily),
        },
        (Some(_), Some(last)) => Some(Kind::Update { from: last.clone() }),
        _ => Some(Kind::Install),
    }
}

/// OTLP/HTTP/JSON body for a single metric data point. Adds `kind` and `os` as
/// attributes so the collector can distinguish install/update/daily.
pub fn otlp_payload(install_id: &str, kind: &Kind, version: &str, os: &str, arch: &str) -> Value {
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let update_from = match kind {
        Kind::Update { from } => Some(from.clone()),
        _ => None,
    };
    json!({
        "resourceMetrics": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "anyr-cli" } },
                    { "key": "service.version", "value": { "stringValue": version } },
                    { "key": "os.type", "value": { "stringValue": os } },
                    { "key": "host.arch", "value": { "stringValue": arch } },
                    { "key": "cli.install_id", "value": { "stringValue": install_id } },
                ],
            },
            "scopeMetrics": [{
                "scope": { "name": "anyr.telemetry" },
                "metrics": [{
                    "name": kind.event(),
                    "unit": "1",
                    "sum": {
                        "aggregationTemporality": 1,
                        "isMonotonic": true,
                        "dataPoints": [{
                            "attributes": [
                                { "key": "channel", "value": { "stringValue": if version.contains("beta") { "beta" } else { "stable" } } },
                                { "key": "from_version", "value": { "stringValue": update_from.as_deref().unwrap_or("") } },
                            ],
                            "startTimeUnixNano": now_ns,
                            "timeUnixNano": now_ns,
                            "asInt": "1",
                        }],
                    },
                }],
            }],
        }],
    })
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
    // `create + truncate` (not exclusive): the file exists after the first
    // install, and every later version change and daily ping must overwrite it.
    let _ = crate::config::write_private(&state_path(env), body.as_bytes());
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

/// Runs before every command. Never fails. State is saved before the send,
/// so a failed send is lost rather than retried on every run. The daily ping
/// is the exception: it is handed to a detached child, so the command never
/// waits on the network for it.
#[cfg(feature = "native")]
pub fn on_startup(env: &BTreeMap<String, String>) {
    let cfg = crate::key::load_config_if_present(&crate::config::resolve_config_path(None, env));
    let config_flag = cfg.as_ref().and_then(|c| c.telemetry);
    let otlp_ep = cfg.as_ref().and_then(|c| c.telemetry_endpoint.as_deref());
    let dev_build = is_dev_run(
        std::env::current_exe()
            .map(|exe| crate::autoupdate::is_cargo_target_build(&exe))
            .unwrap_or(false),
        env,
    );
    let custom = endpoint(env);
    // A cargo build only stays quiet toward the real PostHog endpoint.
    if !enabled(
        env,
        config_flag,
        dev_build && custom.is_none() && otlp_ep.is_none(),
    ) || env.contains_key(crate::autoupdate::ENV_AUTO_CHILD)
        || env.contains_key(crate::autoupdate::ENV_LAUNCHER_CHILD)
    {
        return;
    }
    let today = crate::autoupdate::now_secs() / SECS_PER_DAY;
    let mut state = fs::read_to_string(state_path(env))
        .ok()
        .map(|raw| State::from_json(&raw))
        .unwrap_or_default();
    let Some(kind) = decide(&state, crate::VERSION, today) else {
        return;
    };
    let id = state.install_id.clone().unwrap_or_else(new_install_id);
    state.install_id = Some(id.clone());
    state.last_version = Some(crate::VERSION.to_string());
    // Any ping sent proves the machine was active today, so it suppresses
    // the daily ping for the rest of the UTC day.
    state.last_daily = Some(today);
    save(env, &state);
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    if let Some(url) = otlp_ep {
        let body = otlp_payload(&id, &kind, crate::VERSION, os, arch);
        match kind {
            Kind::Daily => spawn_daily_sender(&body, url),
            Kind::Install | Kind::Update { .. } => send(url, &body),
        }
        return;
    }
    let url = custom.as_deref().unwrap_or(POSTHOG_CAPTURE_URL);
    let body = payload(&id, &kind, crate::VERSION, os, arch);
    match kind {
        Kind::Daily => spawn_daily_sender(&body, url),
        Kind::Install | Kind::Update { .. } => send(url, &body),
    }
}

/// Detached sender for the daily ping: payload on stdin (never argv), and
/// only the endpoint is forwarded. The child inherits no config, no key, and
/// no home directory, so nothing but the exact JSON leaves this machine.
#[cfg(feature = "native")]
fn spawn_daily_sender(body: &Value, url: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    cmd.arg("__telemetry-daily")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_clear()
        .env("ANYR_TELEMETRY_URL", url);
    #[cfg(windows)]
    {
        // Windows loads shared libraries through SYSTEMROOT; an env_clear()
        // child can fail to start at all, which looks like a silent timeout.
        for key in ["SYSTEMROOT", "WINDIR", "PATH"] {
            if let Ok(value) = std::env::var(key) {
                cmd.env(key, value);
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    if let Ok(mut child) = cmd.spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(body.to_string().as_bytes());
        }
        // Detached: the child finishes its one POST after we exit.
    }
}

/// Body of the detached child: one POST of the JSON read from stdin, one
/// attempt, response ignored. Silent and never panics.
#[cfg(feature = "native")]
pub fn send_from_stdin(env: &BTreeMap<String, String>) {
    use std::io::Read;
    let Some(url) = endpoint(env) else {
        return;
    };
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    let Ok(body) = serde_json::from_str::<Value>(buf.trim()) else {
        return;
    };
    send(&url, &body);
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
    fn decides_install_update_daily_or_nothing() {
        let fresh = State::default();
        assert_eq!(decide(&fresh, "0.1.40", 100), Some(Kind::Install));
        let seen = State {
            install_id: Some("abc".into()),
            last_version: Some("0.1.39".into()),
            last_daily: Some(100),
        };
        assert_eq!(
            decide(&seen, "0.1.40", 100),
            Some(Kind::Update {
                from: "0.1.39".into()
            })
        );
        // Same version and a ping already sent today: nothing.
        assert_eq!(decide(&seen, "0.1.39", 100), None);
        let no_id = State {
            install_id: None,
            last_version: Some("0.1.39".into()),
            last_daily: None,
        };
        assert_eq!(decide(&no_id, "0.1.39", 100), Some(Kind::Install));
    }

    #[test]
    fn decides_daily_only_when_the_utc_day_moved_on() {
        let seen = State {
            install_id: Some("abc".into()),
            last_version: Some("0.1.39".into()),
            last_daily: Some(100),
        };
        assert_eq!(decide(&seen, "0.1.39", 101), Some(Kind::Daily));
        assert_eq!(decide(&seen, "0.1.39", 102), Some(Kind::Daily));
        // A state written before daily pings existed has no stamp: daily due.
        let legacy = State {
            install_id: Some("abc".into()),
            last_version: Some("0.1.39".into()),
            last_daily: None,
        };
        assert_eq!(decide(&legacy, "0.1.39", 100), Some(Kind::Daily));
        // Clock skew backwards still counts as "already sent today".
        assert_eq!(decide(&seen, "0.1.39", 99), None);
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
    fn daily_payload_holds_only_the_promised_fields() {
        let body = payload("abc", &Kind::Daily, "0.1.40", "linux", "x86_64");
        assert_eq!(body["event"], "cli_daily");
        assert_eq!(body["distinct_id"], "abc");
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
                "os",
                "version"
            ]
        );
        assert!(body["properties"].get("from_version").is_none());
    }

    #[test]
    fn state_roundtrips_and_a_bad_file_reads_as_fresh() {
        let s = State {
            install_id: Some("abc".into()),
            last_version: Some("1".into()),
            last_daily: Some(100),
        };
        assert_eq!(State::from_json(&s.to_json().to_string()), s);
        assert_eq!(State::from_json("not json"), State::default());
        // Files written before the daily ping have no stamp, so daily is due.
        let legacy = State::from_json(r#"{"install_id":"a","last_version":"1"}"#);
        assert_eq!(legacy.last_daily, None);
    }
}
