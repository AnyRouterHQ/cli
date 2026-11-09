//! On-disk relay state under `<config dir>/relay/`: the pidfile, the live
//! snapshot the running relay writes, the background log, and saved prefs.
//! One source of truth, so `status`, `stop` and `logs` work from any
//! terminal, for foreground and background runs alike.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// A snapshot older than this, from a live pid, means the relay is hung or
/// the pid was reused by another process.
pub(super) const STALE_AFTER_SECS: u64 = 15;

pub(super) struct Paths {
    pub dir: PathBuf,
    pub pid: PathBuf,
    pub state: PathBuf,
    pub log: PathBuf,
    pub prefs: PathBuf,
}

pub(super) fn paths(env: &BTreeMap<String, String>) -> Paths {
    let config = crate::config::resolve_config_path(None, env);
    let dir = config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("relay");
    Paths {
        pid: dir.join("relay.pid"),
        state: dir.join("state.json"),
        log: dir.join("relay.log"),
        prefs: dir.join("prefs.json"),
        dir,
    }
}

pub(super) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Write via a temp file and rename, so a reader never sees half a file.
pub(super) fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, contents)
        .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("could not write {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Prefs: what `relay pool` and `relay models` saved — the pool opt-in, the
// on-device model choice, the runtime local-model watch (`auto_detect`) and
// the local-server exposure whitelist (`local_models`).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Prefs {
    /// None = never set (default on): re-poll local servers while connected.
    pub auto_detect: Option<bool>,
    /// None = never set (default on).
    pub pool: Option<bool>,
    /// Catalog ids of on-device models to serve. None = never chosen.
    pub models: Option<Vec<String>>,
    /// None = expose every detected local server model (current behavior).
    /// Some(list) = expose only these raw local ids (e.g. "qwen2.5-coder:32b").
    pub local_models: Option<Vec<String>>,
}

impl Prefs {
    pub fn to_json(&self) -> String {
        let mut v = json!({});
        if let Some(p) = self.pool {
            v["pool"] = json!(p);
        }
        if let Some(m) = &self.models {
            v["models"] = json!(m);
        }
        if let Some(a) = &self.auto_detect {
            v["auto_detect"] = json!(a);
        }
        if let Some(l) = &self.local_models {
            v["local_models"] = json!(l);
        }
        v.to_string()
    }

    pub fn from_json(raw: &str) -> Prefs {
        let v: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
        Prefs {
            auto_detect: v.get("auto_detect").and_then(Value::as_bool),
            pool: v.get("pool").and_then(Value::as_bool),
            models: v.get("models").and_then(Value::as_array).map(|a| {
                a.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            }),
            local_models: v.get("local_models").and_then(Value::as_array).map(|a| {
                a.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            }),
        }
    }
}

pub(super) fn load_prefs(p: &Paths) -> Prefs {
    std::fs::read_to_string(&p.prefs)
        .map(|s| Prefs::from_json(&s))
        .unwrap_or_default()
}

pub(super) fn save_prefs(p: &Paths, prefs: &Prefs) -> Result<(), String> {
    write_atomic(&p.prefs, &prefs.to_json())
}

/// Where the effective pool setting came from, for `status` and `pool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PoolSource {
    Flag,
    Saved,
    Default,
}

impl PoolSource {
    pub fn name(self) -> &'static str {
        match self {
            PoolSource::Flag => "flag",
            PoolSource::Saved => "saved",
            PoolSource::Default => "default",
        }
    }
}

/// `--pool` / `--no-pool`, then the saved pref, then on. A saved `off` must
/// survive a bare `start`: defaulting back to on would donate silently.
pub(super) fn effective_pool(flag: Option<bool>, prefs: &Prefs) -> (bool, PoolSource) {
    match (flag, prefs.pool) {
        (Some(v), _) => (v, PoolSource::Flag),
        (None, Some(v)) => (v, PoolSource::Saved),
        (None, None) => (true, PoolSource::Default),
    }
}

/// None = never set (default on): re-poll local servers while connected.
pub(super) fn effective_auto_detect(prefs: &Prefs) -> bool {
    prefs.auto_detect.unwrap_or(true)
}

// ---------------------------------------------------------------------------
// Snapshot: what the running relay is doing right now.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Conn {
    Connecting,
    Connected,
    /// Waiting to retry after a failed or dropped connection.
    Reconnecting {
        attempt: u32,
        retry_at: u64,
    },
    Stopping,
}

impl Conn {
    fn name(&self) -> &'static str {
        match self {
            Conn::Connecting => "connecting",
            Conn::Connected => "connected",
            Conn::Reconnecting { .. } => "reconnecting",
            Conn::Stopping => "stopping",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub pid: u32,
    pub started_at: u64,
    pub updated_at: u64,
    pub detached: bool,
    pub device: String,
    pub conn: Conn,
    pub last_error: Option<String>,
    pub pool: bool,
    pub models: Vec<String>,
    /// (label, base URL) of each local endpoint.
    pub endpoints: Vec<(String, String)>,
    pub requests: u64,
    pub in_flight: u64,
    pub errors: u64,
    /// None until a response reports `usage`; fm serve may never send it.
    pub tokens: Option<u64>,
}

impl Snapshot {
    pub fn to_value(&self) -> Value {
        let mut conn = json!({ "state": self.conn.name() });
        if let Conn::Reconnecting { attempt, retry_at } = self.conn {
            conn["attempt"] = json!(attempt);
            conn["retry_at"] = json!(retry_at);
        }
        json!({
            "pid": self.pid,
            "started_at": self.started_at,
            "updated_at": self.updated_at,
            "detached": self.detached,
            "device": self.device,
            "connection": conn,
            "last_error": self.last_error,
            "pool": self.pool,
            "models": self.models,
            "endpoints": self.endpoints.iter().map(|(l, b)| json!({"label": l, "base": b})).collect::<Vec<_>>(),
            "requests": self.requests,
            "in_flight": self.in_flight,
            "errors": self.errors,
            "tokens": self.tokens,
        })
    }

    pub fn from_value(v: &Value) -> Option<Snapshot> {
        let u = |k: &str| v.get(k).and_then(Value::as_u64);
        let conn = v.get("connection")?;
        let conn = match conn.get("state")?.as_str()? {
            "connected" => Conn::Connected,
            "reconnecting" => Conn::Reconnecting {
                attempt: conn.get("attempt").and_then(Value::as_u64).unwrap_or(0) as u32,
                retry_at: conn.get("retry_at").and_then(Value::as_u64).unwrap_or(0),
            },
            "stopping" => Conn::Stopping,
            _ => Conn::Connecting,
        };
        Some(Snapshot {
            pid: u("pid")? as u32,
            started_at: u("started_at").unwrap_or(0),
            updated_at: u("updated_at").unwrap_or(0),
            detached: v.get("detached").and_then(Value::as_bool).unwrap_or(false),
            device: v.get("device")?.as_str()?.to_string(),
            conn,
            last_error: v
                .get("last_error")
                .and_then(Value::as_str)
                .map(str::to_string),
            pool: v.get("pool").and_then(Value::as_bool).unwrap_or(false),
            models: v
                .get("models")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            endpoints: v
                .get("endpoints")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|e| {
                            Some((
                                e.get("label")?.as_str()?.to_string(),
                                e.get("base")?.as_str()?.to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            requests: u("requests").unwrap_or(0),
            in_flight: u("in_flight").unwrap_or(0),
            errors: u("errors").unwrap_or(0),
            tokens: u("tokens"),
        })
    }
}

pub(super) fn write_snapshot(p: &Paths, s: &Snapshot) -> Result<(), String> {
    write_atomic(&p.state, &s.to_value().to_string())
}

// ---------------------------------------------------------------------------
// Pidfile + liveness
// ---------------------------------------------------------------------------

pub(super) fn write_pid(p: &Paths, pid: u32) -> Result<(), String> {
    write_atomic(&p.pid, &format!("{pid}\n"))
}

pub(super) fn read_pid(p: &Paths) -> Option<u32> {
    std::fs::read_to_string(&p.pid).ok()?.trim().parse().ok()
}

/// Remove the pidfile and snapshot, but only when they still belong to `pid`
/// (a newer relay may have taken over).
pub(super) fn clear_if_owned(p: &Paths, pid: u32) {
    if read_pid(p) == Some(pid) {
        let _ = std::fs::remove_file(&p.pid);
        let _ = std::fs::remove_file(&p.state);
    }
}

#[cfg(unix)]
pub(super) fn pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // Signal 0 checks existence. EPERM means it exists but is not ours.
    let rc = unsafe { libc::kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
pub(super) fn pid_alive(_pid: u32) -> bool {
    false
}

/// What `status` and `stop` see on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Running {
    /// Live pid with a fresh snapshot.
    Yes(Snapshot),
    /// Live pid, but the snapshot is missing or old: hung, or pid reused.
    Unresponsive(u32),
    No,
}

/// Classify the on-disk state. Pure apart from `alive`, which tests inject.
pub(super) fn classify(
    pid: Option<u32>,
    snapshot: Option<Snapshot>,
    now: u64,
    alive: impl Fn(u32) -> bool,
    is_relay: impl Fn(u32) -> bool,
) -> Running {
    let Some(pid) = pid else { return Running::No };
    if !alive(pid) {
        return Running::No;
    }
    match snapshot {
        Some(s) if s.pid == pid && now.saturating_sub(s.updated_at) <= STALE_AFTER_SECS => {
            Running::Yes(s)
        }
        // A pidfile that outlived a crash or reboot may name a reused pid.
        // Only call it ours if that process really is `anyr relay start`,
        // or `stop` would signal a stranger.
        _ if is_relay(pid) => Running::Unresponsive(pid),
        _ => Running::No,
    }
}

/// Whether `pid` runs `… relay start` (via `ps`, on macOS and Linux).
#[cfg(unix)]
pub(super) fn looks_like_relay(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .map(|o| command_is_relay(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub(super) fn looks_like_relay(_pid: u32) -> bool {
    false
}

pub(super) fn command_is_relay(cmdline: &str) -> bool {
    let words: Vec<&str> = cmdline.split_whitespace().collect();
    words.windows(2).any(|w| w == ["relay", "start"])
}

pub(super) fn read_snapshot(p: &Paths) -> Option<Snapshot> {
    let raw = std::fs::read_to_string(&p.state).ok()?;
    Snapshot::from_value(&serde_json::from_str(&raw).ok()?)
}

pub(super) fn running(p: &Paths) -> Running {
    let pid = read_pid(p);
    let r = classify(
        pid,
        read_snapshot(p),
        now_secs(),
        pid_alive,
        looks_like_relay,
    );
    if r == Running::No && pid.is_some() {
        // Stale pidfile from a crash: tidy it so the next start is clean.
        let _ = std::fs::remove_file(&p.pid);
        let _ = std::fs::remove_file(&p.state);
    }
    r
}

#[cfg(test)]
pub(super) fn sample_snapshot() -> Snapshot {
    Snapshot {
        pid: 4242,
        started_at: 1_000,
        updated_at: 1_200,
        detached: true,
        device: "Desk Mac".into(),
        conn: Conn::Connected,
        last_error: None,
        pool: true,
        models: vec!["foundation-model".into(), "pplx-decider-flash-v1.2".into()],
        endpoints: vec![(
            "fm serve (Apple Foundation Models)".into(),
            "http://127.0.0.1:1976/v1".into(),
        )],
        requests: 12,
        in_flight: 1,
        errors: 0,
        tokens: Some(3456),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_flag_beats_saved_pref_and_saved_off_survives_a_bare_start() {
        let off = Prefs {
            pool: Some(false),
            models: None,
            ..Default::default()
        };
        // WHY: a user who ran `relay pool off` must not start donating again
        // because a later `relay start` omitted --no-pool.
        assert_eq!(effective_pool(None, &off), (false, PoolSource::Saved));
        assert_eq!(effective_pool(Some(true), &off), (true, PoolSource::Flag));
        assert_eq!(
            effective_pool(None, &Prefs::default()),
            (true, PoolSource::Default)
        );
        assert_eq!(
            effective_pool(Some(false), &Prefs::default()),
            (false, PoolSource::Flag)
        );
    }

    #[test]
    fn prefs_and_snapshot_round_trip_through_json() {
        let prefs = Prefs {
            pool: Some(false),
            models: Some(vec!["apple/foundation-model".into()]),
            ..Default::default()
        };
        assert_eq!(Prefs::from_json(&prefs.to_json()), prefs);
        assert_eq!(Prefs::from_json("garbage"), Prefs::default());

        let mut s = sample_snapshot();
        assert_eq!(Snapshot::from_value(&s.to_value()), Some(s.clone()));
        s.conn = Conn::Reconnecting {
            attempt: 3,
            retry_at: 1_210,
        };
        s.tokens = None;
        assert_eq!(Snapshot::from_value(&s.to_value()), Some(s));
    }

    #[test]
    fn auto_detect_and_local_models_round_trip() {
        // Test auto_detect default-on (None -> true)
        let prefs = Prefs {
            auto_detect: None,
            local_models: None,
            ..Default::default()
        };
        assert_eq!(Prefs::from_json(&prefs.to_json()), prefs);

        // Test auto_detect explicitly off
        let prefs_off = Prefs {
            auto_detect: Some(false),
            local_models: Some(vec!["qwen2.5-coder:32b".into()]),
            ..Default::default()
        };
        assert_eq!(Prefs::from_json(&prefs_off.to_json()), prefs_off);

        // Test effective_auto_detect
        assert!(effective_auto_detect(&Prefs::default()), "default on");
        assert!(
            !effective_auto_detect(&Prefs {
                auto_detect: Some(false),
                ..Default::default()
            }),
            "explicit off"
        );
        assert!(
            effective_auto_detect(&Prefs {
                auto_detect: Some(true),
                ..Default::default()
            }),
            "explicit on"
        );
    }

    #[test]
    fn classify_detects_dead_stale_and_reused_pids() {
        let s = sample_snapshot();
        let alive = |_| true;
        let dead = |_| false;
        let relay = |_| true;
        let stranger = |_| false;
        assert_eq!(
            classify(None, Some(s.clone()), 1_200, alive, relay),
            Running::No
        );
        assert_eq!(
            classify(Some(4242), Some(s.clone()), 1_200, dead, relay),
            Running::No
        );
        assert_eq!(
            classify(Some(4242), Some(s.clone()), 1_205, alive, stranger),
            Running::Yes(s.clone())
        );
        let late = 1_200 + STALE_AFTER_SECS + 1;
        // Snapshot not refreshed for a while and the pid is still a relay: hung.
        assert_eq!(
            classify(Some(4242), Some(s.clone()), late, alive, relay),
            Running::Unresponsive(4242)
        );
        // WHY: after a crash or reboot the pid may belong to another program;
        // treating it as ours would let `relay stop` SIGTERM it.
        assert_eq!(
            classify(Some(4242), Some(s.clone()), late, alive, stranger),
            Running::No
        );
        assert_eq!(
            classify(Some(7), Some(s), 1_200, alive, stranger),
            Running::No
        );
    }

    #[test]
    fn relay_command_lines_are_recognized() {
        assert!(command_is_relay(
            "/usr/local/bin/anyr relay start --no-pool"
        ));
        assert!(!command_is_relay("/usr/bin/vim relay.txt"));
        assert!(!command_is_relay(""));
    }

    #[test]
    fn paths_follow_anyrouter_home() {
        let mut env = BTreeMap::new();
        env.insert("ANYROUTER_HOME".into(), "/tmp/anyr-x".into());
        let p = paths(&env);
        assert_eq!(p.pid, PathBuf::from("/tmp/anyr-x/relay/relay.pid"));
        assert_eq!(p.log, PathBuf::from("/tmp/anyr-x/relay/relay.log"));
    }

    #[cfg(unix)]
    #[test]
    fn this_process_is_alive_and_pid_zero_is_not() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(0));
    }
}
