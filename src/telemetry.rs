//! Anonymous install, update and daily ping to PostHog.
//!
//! Sent at most once per install, once per version change, and at most
//! once per UTC day, so a count of `cli_install`, `cli_update` and
//! `cli_daily` events answers "how many installs, on which versions, and
//! how many active users per day". It carries a random install id, the
//! version, the OS and the CPU architecture. It never carries a key, a
//! path, a hostname, an IP address or a prompt, and it asks PostHog not to
//! build a person profile or look up a location.
//!
//! The install and update pings send synchronously (they are rare). The
//! daily ping is handed to a detached `anyr __telemetry-daily` child that
//! reads the payload from stdin, so the first command of each day never
//! waits on the network.
//!
//! On by default; the first run on a terminal prints one line saying so.
//! Off with `anyr telemetry off` (`telemetry: false` in the config),
//! `ANYR_TELEMETRY=0`, `ANYR_NO_TELEMETRY=1` or `DO_NOT_TRACK=1`. It also
//! stays off in CI, in dev builds and when `ANYR_NO_UPDATE` is set (the
//! switch every test uses), and nothing is written while it is off, not even
//! an install id.

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
    /// The one-line notice was printed on a terminal; it never prints again.
    pub notice_shown: bool,
    /// UTC day of the last crash upload and how many went out that day.
    pub crash_day: Option<u64>,
    pub crash_sent: u64,
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
        // The file is user-writable and `last_version` rides `cli_update` as
        // `from_version`: only a version-shaped string may come back.
        let version_like = |v: &String| crate::report::clean_version(v);
        Self {
            install_id: get("install_id")
                .filter(|id| id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit())),
            last_version: get("last_version").filter(version_like),
            last_daily: map.get("last_daily").and_then(Value::as_u64),
            notice_shown: map.get("notice_shown").and_then(Value::as_bool) == Some(true),
            crash_day: map.get("crash_day").and_then(Value::as_u64),
            crash_sent: map.get("crash_sent").and_then(Value::as_u64).unwrap_or(0),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "install_id": self.install_id,
            "last_version": self.last_version,
            "last_daily": self.last_daily,
            "notice_shown": self.notice_shown,
            "crash_day": self.crash_day,
            "crash_sent": self.crash_sent,
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
    let from = match kind {
        Kind::Update { from } => from.as_str(),
        _ => "",
    };
    let channel = if version.contains("beta") {
        "beta"
    } else {
        "stable"
    };
    // A parsed template plus a fill costs far less code than the nested
    // `json!` it replaced; the body is the same value.
    let mut body: Value = serde_json::from_str(OTLP_TEMPLATE).unwrap_or_default();
    fill(
        &mut body,
        &[
            ("$version", version.into()),
            ("$os", os.into()),
            ("$arch", arch.into()),
            ("$id", install_id.into()),
            ("$event", kind.event().into()),
            ("$channel", channel.into()),
            ("$from", from.into()),
            ("$now", now_ns.into()),
        ],
    );
    body
}

const OTLP_TEMPLATE: &str = r#"{"resourceMetrics":[{"resource":{"attributes":[
{"key":"service.name","value":{"stringValue":"anyr-cli"}},
{"key":"service.version","value":{"stringValue":"$version"}},
{"key":"os.type","value":{"stringValue":"$os"}},
{"key":"host.arch","value":{"stringValue":"$arch"}},
{"key":"cli.install_id","value":{"stringValue":"$id"}}]},
"scopeMetrics":[{"scope":{"name":"anyr.telemetry"},"metrics":[{"name":"$event","unit":"1",
"sum":{"aggregationTemporality":1,"isMonotonic":true,"dataPoints":[{"attributes":[
{"key":"channel","value":{"stringValue":"$channel"}},
{"key":"from_version","value":{"stringValue":"$from"}}],
"startTimeUnixNano":"$now","timeUnixNano":"$now","asInt":"1"}]}}]}]}]}"#;

/// Replace every string value that is exactly a `$name` in `vars`. Values put
/// in are not scanned again, so input text can never act as a placeholder.
fn fill(v: &mut Value, vars: &[(&str, Value)]) {
    match v {
        Value::String(s) => {
            if let Some((_, with)) = vars.iter().find(|(name, _)| name == s) {
                *v = with.clone();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|x| fill(x, vars)),
        Value::Object(map) => map.values_mut().for_each(|x| fill(x, vars)),
        _ => {}
    }
}

/// True when the ping may be sent. `config_flag` is the file's `telemetry:`.
pub fn enabled(env: &BTreeMap<String, String>, config_flag: Option<bool>, dev_build: bool) -> bool {
    switch(env, config_flag, dev_build).0
}

/// On or off, and the switch that decided it (what `anyr telemetry status`
/// prints).
pub fn switch(
    env: &BTreeMap<String, String>,
    config_flag: Option<bool>,
    dev_build: bool,
) -> (bool, &'static str) {
    let on = |key: &str| env_flag(env, key) == Some(true);
    let off = if env_flag(env, "ANYR_TELEMETRY") == Some(false) {
        "ANYR_TELEMETRY=0"
    } else if on("ANYR_NO_TELEMETRY") {
        "ANYR_NO_TELEMETRY=1"
    } else if on("DO_NOT_TRACK") {
        "DO_NOT_TRACK=1"
    } else if on("ANYR_NO_UPDATE") {
        "ANYR_NO_UPDATE=1"
    } else if env
        .get("CI")
        .is_some_and(|v| !v.trim().is_empty() && env_flag(env, "CI") != Some(false))
    {
        "CI"
    } else if config_flag == Some(false) {
        "telemetry: false in the config"
    } else if dev_build {
        "dev build"
    } else {
        return (true, "default");
    };
    (false, off)
}

/// The exact JSON sent to PostHog. Nothing else leaves the machine.
/// `extra` adds fixed-shape properties: `method` on install, the day's
/// counters on the daily ping.
pub fn payload(
    install_id: &str,
    kind: &Kind,
    version: &str,
    os: &str,
    arch: &str,
    extra: Value,
) -> Value {
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
        // A version change seen at startup is an update that worked; going
        // back to an older build is a rollback.
        let parse = crate::channel::parse_version;
        let back = matches!((parse(from), parse(version)), (Some(f), Some(t)) if t < f);
        props["result"] = json!(if back { "rolled_back" } else { "ok" });
    }
    if let (Value::Object(props), Value::Object(extra)) = (&mut props, extra) {
        props.extend(extra);
    }
    json!({
        "api_key": POSTHOG_KEY,
        "event": kind.event(),
        "distinct_id": install_id,
        "properties": props,
    })
}

/// How this binary was installed, from where it lives: Homebrew and npm own
/// their paths, the install script writes `~/.local/bin`, anything else is
/// manual. Only the word leaves the machine, never the path.
pub fn install_method(exe: &std::path::Path) -> &'static str {
    match crate::autoupdate::package_manager_hint(exe) {
        Some(h) if h.starts_with("brew") => "brew",
        Some(_) => "npm",
        None if exe
            .to_string_lossy()
            .replace('\\', "/")
            .contains("/.local/bin/") =>
        {
            "script"
        }
        None => "manual",
    }
}

/// The extra properties of one event kind on this machine.
#[cfg(feature = "native")]
pub fn extra(kind: &Kind, usage: Option<&crate::counters::Day>) -> Value {
    match kind {
        Kind::Install => json!({
            "method": std::env::current_exe()
                .map(|exe| install_method(&exe))
                .unwrap_or("manual"),
        }),
        Kind::Update { .. } => Value::Null,
        Kind::Daily => usage.cloned().unwrap_or_default().to_json(),
    }
}

/// 32 hex chars from the OS random source, mixed with the clock and pid so a
/// missing `/dev/urandom` (Windows) still yields a distinct id per install.
#[cfg(feature = "native")]
pub(crate) fn new_install_id() -> String {
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
pub(crate) fn state_path(env: &BTreeMap<String, String>) -> PathBuf {
    crate::autoupdate::state_dir(env).join(STATE_FILE)
}

#[cfg(feature = "native")]
pub(crate) fn save(env: &BTreeMap<String, String>, state: &State) {
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

/// `ANYR_TELEMETRY_URL` points at this machine (a test server).
pub fn loopback_endpoint(env: &BTreeMap<String, String>) -> bool {
    let Some(rest) = env
        .get("ANYR_TELEMETRY_URL")
        .and_then(|u| u.trim().strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // `[::1]:8080` -> `[::1]`, `127.0.0.1:8080` -> `127.0.0.1`; any `@`
    // (userinfo) or other host is refused.
    let host = match authority.rsplit_once(':') {
        Some((h, port)) if !h.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    };
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
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

/// What decides telemetry on this run and where it goes.
#[cfg(feature = "native")]
pub struct Status {
    pub on: bool,
    /// The switch that decided it (`default`, `DO_NOT_TRACK=1`, ...).
    pub why: &'static str,
    pub url: String,
    /// `url` is the OTLP endpoint from `telemetry_endpoint:`.
    pub otlp: bool,
}

#[cfg(feature = "native")]
pub fn status(env: &BTreeMap<String, String>) -> Status {
    let cfg = crate::key::load_config_if_present(&crate::config::resolve_config_path(None, env));
    let config_flag = cfg.as_ref().and_then(|c| c.telemetry);
    let otlp_ep = cfg.and_then(|c| c.telemetry_endpoint);
    let dev_build = is_dev_run(
        std::env::current_exe()
            .map(|exe| crate::autoupdate::is_cargo_target_build(&exe))
            .unwrap_or(false),
        env,
    );
    let custom = endpoint(env);
    // A cargo build only stays quiet toward the real PostHog endpoint.
    let (on, mut why) = switch(
        env,
        config_flag,
        dev_build && custom.is_none() && otlp_ep.is_none(),
    );
    if on && config_flag == Some(true) {
        why = "telemetry: true in the config";
    }
    let otlp = otlp_ep.is_some();
    let url = otlp_ep
        .or(custom)
        .unwrap_or_else(|| POSTHOG_CAPTURE_URL.to_string());
    Status { on, why, url, otlp }
}

/// The state dir while telemetry is on for this run, decided once. The
/// updater's own detached child never records anything.
#[cfg(feature = "native")]
static GATE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

#[cfg(feature = "native")]
fn decide_gate(env: &BTreeMap<String, String>, st: &Status) -> Option<PathBuf> {
    (st.on && !env.contains_key(crate::autoupdate::ENV_AUTO_CHILD))
        .then(|| crate::autoupdate::state_dir(env))
}

/// Where telemetry files go, or `None` while it is off. Every local write
/// (state, counters, crash files, last error) checks this first.
#[cfg(feature = "native")]
pub fn gate(env: &BTreeMap<String, String>) -> Option<&'static std::path::Path> {
    GATE.get_or_init(|| decide_gate(env, &status(env)))
        .as_deref()
}

/// The decision when one was already made; never computes it.
#[cfg(feature = "native")]
pub fn gate_if_known() -> Option<&'static std::path::Path> {
    GATE.get().and_then(|g| g.as_deref())
}

/// The one-time line on a terminal.
pub fn notice_line() -> String {
    format!(
        "anyr sends anonymous usage counts and crash reports (no keys, paths or prompts). Turn them off with `{} telemetry off`.",
        crate::help::invoked_bin()
    )
}

#[cfg(feature = "native")]
pub(crate) fn load_state(env: &BTreeMap<String, String>) -> State {
    fs::read_to_string(state_path(env))
        .ok()
        .map(|raw| State::from_json(&raw))
        .unwrap_or_default()
}

/// Runs before every command. Never fails. State is saved before the send,
/// so a failed send is lost rather than retried on every run. The daily ping
/// is the exception: it is handed to a detached child, so the command never
/// waits on the network for it. `quiet` is a run asking for machine output
/// (`--json`, `--quiet`); it never gets the notice.
#[cfg(feature = "native")]
pub fn on_startup(env: &BTreeMap<String, String>, quiet: bool) {
    let st = status(env);
    let gate = GATE.get_or_init(|| decide_gate(env, &st));
    // Inside a session anyr launched, the parent already pinged.
    if gate.is_none() || env.contains_key(crate::autoupdate::ENV_LAUNCHER_CHILD) {
        return;
    }
    let Some(dir) = gate.as_deref() else {
        return;
    };
    let today = crate::autoupdate::now_secs() / SECS_PER_DAY;
    // State, counters and crash files change together under one lock, so
    // two runs at once never send the same crash or day twice.
    let Some(Some((kind, id, mut detached))) =
        crate::counters::locked(dir, || prepare(env, &st, dir, quiet, today))
    else {
        return;
    };
    if let Some(kind) = kind {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let body = if st.otlp {
            otlp_payload(&id, &kind.0, crate::VERSION, os, arch)
        } else {
            payload(
                &id,
                &kind.0,
                crate::VERSION,
                os,
                arch,
                extra(&kind.0, kind.1.as_ref()),
            )
        };
        match kind.0 {
            Kind::Daily => detached.insert(0, body),
            Kind::Install | Kind::Update { .. } => send(&st.url, &body),
        }
    }
    if !detached.is_empty() {
        spawn_daily_sender(&Value::Array(detached), &st.url);
    }
}

/// The locked part of `on_startup`: decide, print the notice, take the
/// counters and unsent crashes, save the state. `None` when nothing is due.
#[cfg(feature = "native")]
#[allow(clippy::type_complexity)]
fn prepare(
    env: &BTreeMap<String, String>,
    st: &Status,
    dir: &std::path::Path,
    quiet: bool,
    today: u64,
) -> Option<(
    Option<(Kind, Option<crate::counters::Day>)>,
    String,
    Vec<Value>,
)> {
    let mut state = load_state(env);
    let notice =
        !quiet && !state.notice_shown && std::io::IsTerminal::is_terminal(&std::io::stderr());
    let kind = decide(&state, crate::VERSION, today);
    // Unsent crash files go out with the next run, at most MAX_PER_DAY a
    // day. OTLP has no crash form.
    let crash_dir = crate::crash::dir(dir);
    let sent_today = if state.crash_day == Some(today) {
        state.crash_sent
    } else {
        0
    };
    let room = crate::crash::MAX_PER_DAY.saturating_sub(sent_today);
    let crashes_waiting = !st.otlp && room > 0 && !crate::crash::files(&crash_dir).is_empty();
    if kind.is_none() && !notice && !crashes_waiting {
        return None;
    }
    if notice {
        eprintln!("{}", crate::term::dim(&notice_line()));
        state.notice_shown = true;
    }
    let id = state.install_id.clone().unwrap_or_else(new_install_id);
    state.install_id = Some(id.clone());
    if kind.is_some() {
        state.last_version = Some(crate::VERSION.to_string());
        // Any ping sent proves the machine was active today, so it
        // suppresses the daily ping for the rest of the UTC day.
        state.last_daily = Some(today);
    }
    let detached = if crashes_waiting {
        crate::crash::take_unsent(&crash_dir, &id, room)
    } else {
        Vec::new()
    };
    if !detached.is_empty() {
        state.crash_day = Some(today);
        state.crash_sent = sent_today + detached.len() as u64;
    }
    // Yesterday's counters ride the daily ping and leave the file with it;
    // the OTLP form has no field for them, so they are dropped there.
    let kind = kind.map(|k| {
        let usage = (k == Kind::Daily)
            .then(|| crate::counters::take_before(dir, today))
            .flatten();
        (k, usage)
    });
    save(env, &state);
    Some((kind, id, detached))
}

/// Runs once at process exit: adds this run to today's counters. Nothing
/// before argument parsing is counted, and nothing is sent.
#[cfg(feature = "native")]
pub fn on_exit(code: i32, env: &BTreeMap<String, String>) {
    let Some(run) = crate::counters::this_run(code) else {
        return;
    };
    let Some(dir) = gate(env) else {
        return;
    };
    let config = crate::config::resolve_config_path(None, env);
    crate::counters::locked(dir, || {
        // `anyr telemetry off` may have run since this run started; it
        // deletes the file under this same lock.
        if crate::key::load_config_if_present(&config).and_then(|c| c.telemetry) != Some(false) {
            crate::counters::record(dir, crate::autoupdate::now_secs() / SECS_PER_DAY, &run);
            // A failed run (not an agent's own exit, not `report` itself)
            // leaves a redacted note for `anyr report --last`.
            if run.error.is_some() && run.command != "report" {
                let message = crate::counters::last_error()
                    .map(|(_, m)| m)
                    .unwrap_or_default();
                crate::report::save_last_error(dir, &run, code, &message);
            }
        }
    });
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
    match serde_json::from_str::<Value>(buf.trim()) {
        // The daily ping and any crash reports: one POST each, at most six.
        Ok(Value::Array(bodies)) => {
            for body in bodies.iter().take(1 + crate::crash::MAX_PER_DAY as usize) {
                send(&url, body);
            }
        }
        Ok(body) => send(&url, &body),
        Err(_) => {}
    }
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

    /// The OTLP body as the collector has always received it; a template
    /// typo would change the wire format silently.
    #[test]
    fn otlp_body_keeps_its_wire_shape() {
        let kind = Kind::Update {
            from: "0.1.9\"$os".into(),
        };
        let mut got = otlp_payload("abc", &kind, "0.1.44-beta.1", "linux", "x86_64");
        let points = "/resourceMetrics/0/scopeMetrics/0/metrics/0/sum/dataPoints/0";
        let now = got.pointer(&format!("{points}/timeUnixNano")).cloned();
        assert!(now.as_ref().and_then(Value::as_u64).unwrap_or(0) > 0);
        assert_eq!(
            got.pointer(&format!("{points}/startTimeUnixNano")),
            now.as_ref()
        );
        for field in ["startTimeUnixNano", "timeUnixNano"] {
            if let Some(v) = got.pointer_mut(&format!("{points}/{field}")) {
                *v = json!(7);
            }
        }
        let want = json!({
            "resourceMetrics": [{
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": "anyr-cli" } },
                        { "key": "service.version", "value": { "stringValue": "0.1.44-beta.1" } },
                        { "key": "os.type", "value": { "stringValue": "linux" } },
                        { "key": "host.arch", "value": { "stringValue": "x86_64" } },
                        { "key": "cli.install_id", "value": { "stringValue": "abc" } },
                    ],
                },
                "scopeMetrics": [{
                    "scope": { "name": "anyr.telemetry" },
                    "metrics": [{
                        "name": "cli_update",
                        "unit": "1",
                        "sum": {
                            "aggregationTemporality": 1,
                            "isMonotonic": true,
                            "dataPoints": [{
                                "attributes": [
                                    { "key": "channel", "value": { "stringValue": "beta" } },
                                    { "key": "from_version", "value": { "stringValue": "0.1.9\"$os" } },
                                ],
                                "startTimeUnixNano": 7,
                                "timeUnixNano": 7,
                                "asInt": "1",
                            }],
                        },
                    }],
                }],
            }],
        });
        assert_eq!(got.to_string(), want.to_string());
        let daily = otlp_payload("abc", &Kind::Daily, "0.1.44", "macos", "aarch64");
        assert_eq!(
            daily.pointer("/resourceMetrics/0/scopeMetrics/0/metrics/0/name"),
            Some(&json!("cli_daily"))
        );
        assert_eq!(
            daily.pointer(&format!("{points}/attributes/0/value/stringValue")),
            Some(&json!("stable"))
        );
        assert_eq!(
            daily.pointer(&format!("{points}/attributes/1/value/stringValue")),
            Some(&json!(""))
        );
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
            env(&[("ANYR_TELEMETRY", "0")]),
            env(&[("ANYR_NO_UPDATE", "1")]),
            env(&[("CI", "true")]),
        ] {
            assert!(!enabled(&e, None, false), "{e:?}");
        }
        assert!(!enabled(&env(&[]), Some(false), false), "config flag");
        assert!(!enabled(&env(&[]), None, true), "dev build");
    }

    #[test]
    fn switch_names_the_opt_out_that_won() {
        assert_eq!(switch(&env(&[]), None, false), (true, "default"));
        assert_eq!(
            switch(
                &env(&[("ANYR_TELEMETRY", "0"), ("DO_NOT_TRACK", "1")]),
                None,
                false
            ),
            (false, "ANYR_TELEMETRY=0")
        );
        assert_eq!(
            switch(&env(&[]), Some(false), true),
            (false, "telemetry: false in the config")
        );
        // ANYR_TELEMETRY=1 is not an override: DO_NOT_TRACK still wins.
        assert!(!enabled(
            &env(&[("ANYR_TELEMETRY", "1"), ("DO_NOT_TRACK", "1")]),
            None,
            false
        ));
    }

    #[test]
    fn only_a_real_loopback_url_counts_as_loopback() {
        let lb = |u: &str| loopback_endpoint(&env(&[("ANYR_TELEMETRY_URL", u)]));
        assert!(lb("http://127.0.0.1:4000/i/v0/e/"));
        assert!(lb("http://localhost/x"));
        assert!(lb("http://[::1]:9/"));
        assert!(!lb("http://localhost.evil.com/"));
        assert!(!lb("http://127.0.0.1.nip.io/"));
        assert!(!lb("http://localhost@evil.com/"));
        assert!(!lb("https://us.i.posthog.com/i/v0/e/"));
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
            notice_shown: false,
            crash_day: None,
            crash_sent: 0,
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
            notice_shown: false,
            crash_day: None,
            crash_sent: 0,
        };
        assert_eq!(decide(&no_id, "0.1.39", 100), Some(Kind::Install));
    }

    #[test]
    fn decides_daily_only_when_the_utc_day_moved_on() {
        let seen = State {
            install_id: Some("abc".into()),
            last_version: Some("0.1.39".into()),
            last_daily: Some(100),
            notice_shown: false,
            crash_day: None,
            crash_sent: 0,
        };
        assert_eq!(decide(&seen, "0.1.39", 101), Some(Kind::Daily));
        assert_eq!(decide(&seen, "0.1.39", 102), Some(Kind::Daily));
        // A state written before daily pings existed has no stamp: daily due.
        let legacy = State {
            install_id: Some("abc".into()),
            last_version: Some("0.1.39".into()),
            last_daily: None,
            notice_shown: false,
            crash_day: None,
            crash_sent: 0,
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
            Value::Null,
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
                "result",
                "version"
            ]
        );
        assert_eq!(body["event"], "cli_update");
        assert_eq!(body["properties"]["result"], "ok");
        assert_eq!(body["properties"]["$process_person_profile"], false);
        assert_eq!(body["properties"]["channel"], "beta");
    }

    #[test]
    fn daily_payload_holds_only_the_promised_fields() {
        let body = payload(
            "abc",
            &Kind::Daily,
            "0.1.40",
            "linux",
            "x86_64",
            crate::counters::Day::default().to_json(),
        );
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
                "agents",
                "arch",
                "channel",
                "commands",
                "errors",
                "exit",
                "os",
                "slowest",
                "version"
            ]
        );
        assert!(body["properties"].get("from_version").is_none());
    }

    #[test]
    fn an_older_version_is_reported_as_a_rollback() {
        let kind = Kind::Update {
            from: "0.1.50".into(),
        };
        let body = payload("abc", &kind, "0.1.49", "linux", "x86_64", Value::Null);
        assert_eq!(body["properties"]["result"], "rolled_back");
        let odd = Kind::Update {
            from: "dev-build".into(),
        };
        let body = payload("abc", &odd, "0.1.49", "linux", "x86_64", Value::Null);
        assert_eq!(
            body["properties"]["result"], "ok",
            "unknown order is not a rollback"
        );
    }

    #[test]
    fn install_method_is_a_word_from_the_path_never_the_path() {
        use std::path::Path;
        assert_eq!(
            install_method(Path::new("/opt/homebrew/Cellar/anyr/1/bin/anyr")),
            "brew"
        );
        assert_eq!(
            install_method(Path::new("/usr/lib/node_modules/@anyr/cli/anyr")),
            "npm"
        );
        assert_eq!(
            install_method(Path::new("/home/me/.local/bin/anyr")),
            "script"
        );
        assert_eq!(
            install_method(Path::new("C:\\Users\\me\\.local\\bin\\anyr.exe")),
            "script"
        );
        assert_eq!(install_method(Path::new("/usr/local/bin/anyr")), "manual");
    }

    #[test]
    fn state_roundtrips_and_a_bad_file_reads_as_fresh() {
        let s = State {
            install_id: Some("abc".into()),
            last_version: Some("0.1.44-beta.7".into()),
            last_daily: Some(100),
            notice_shown: true,
            crash_day: Some(100),
            crash_sent: 2,
        };
        assert_eq!(State::from_json(&s.to_json().to_string()), s);
        // `last_version` rides cli_update: anything not version-shaped and
        // any id that is not hex is dropped on load.
        let edited = State::from_json(r#"{"install_id":"/home/alice","last_version":"10.0.0.5"}"#);
        assert_eq!(edited.install_id, None);
        assert_eq!(edited.last_version, None);
        assert_eq!(State::from_json("not json"), State::default());
        // Files written before the daily ping have no stamp, so daily is due.
        let legacy = State::from_json(r#"{"install_id":"a","last_version":"1"}"#);
        assert_eq!(legacy.last_daily, None);
    }
}
