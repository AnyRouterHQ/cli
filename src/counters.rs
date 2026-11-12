//! Local usage counters, one aggregate per UTC day, carried by the next
//! day's `cli_daily` ping and then deleted.
//!
//! Recorded once at process exit, never before argument parsing, and never
//! over the network. Every key comes from a closed set compiled into the
//! binary: command names from the registry (anything else is `other`), exit
//! code names, error codes, launcher ids and five duration buckets. A file
//! edited by hand cannot add a key: unknown ones are dropped on load.

use serde_json::{json, Map, Value};

/// Wall time from argument parsing to exit; for a launcher, to the spawn.
pub const BUCKETS: [&str; 5] = ["<100ms", "<1s", "<10s", "<1m", ">=1m"];

/// HTTP statuses counted on their own; any other is `http_other`.
const HTTP: &[&str] = &[
    "http_400",
    "http_401",
    "http_402",
    "http_403",
    "http_404",
    "http_408",
    "http_409",
    "http_413",
    "http_422",
    "http_429",
    "http_500",
    "http_502",
    "http_503",
    "http_504",
    "http_other",
];

pub fn bucket(ms: u128) -> usize {
    match ms {
        0..=99 => 0,
        100..=999 => 1,
        1_000..=9_999 => 2,
        10_000..=59_999 => 3,
        _ => 4,
    }
}

/// The registry name for `command`, or `other`.
pub fn command_slot(command: &str) -> &'static str {
    let canonical = crate::cmd::dispatch::canonical_command(command);
    crate::cmd::dispatch::COMMANDS
        .iter()
        .map(|(n, _)| *n)
        .find(|n| *n == canonical)
        .unwrap_or("other")
}

/// Exit class: the exit-code name, `agent` for a launched agent's own
/// non-zero code, else `other`.
pub fn exit_slot(code: i32, agent: bool) -> &'static str {
    if agent && code != 0 {
        return "agent";
    }
    crate::exit::CODES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, n, _)| *n)
        .unwrap_or("other")
}

/// Error code for a failed run: the HTTP status the message names, else the
/// exit class.
pub fn error_slot(code: i32, message: &str) -> &'static str {
    if let Some(at) = message.find("HTTP ") {
        let status: String = message[at + 5..].chars().take(3).collect();
        let key = format!("http_{status}");
        return HTTP
            .iter()
            .find(|k| **k == key)
            .copied()
            .unwrap_or("http_other");
    }
    exit_slot(code, false)
}

fn intern(set: impl Iterator<Item = &'static str>, key: &str) -> Option<&'static str> {
    set.into_iter().find(|k| *k == key)
}

fn commands_set() -> impl Iterator<Item = &'static str> {
    crate::cmd::dispatch::COMMANDS
        .iter()
        .map(|(n, _)| *n)
        .chain(["other"])
}

fn exit_set() -> impl Iterator<Item = &'static str> {
    crate::exit::CODES
        .iter()
        .map(|(_, n, _)| *n)
        .chain(["agent", "other"])
}

fn error_set() -> impl Iterator<Item = &'static str> {
    exit_set().chain(HTTP.iter().copied())
}

fn agent_set() -> impl Iterator<Item = &'static str> {
    crate::registry::LAUNCHERS.iter().copied()
}

/// One UTC day of counts, kept as the JSON object it is sent as (`commands`,
/// `exit`, `errors`, `agents`, `slowest`), so no second map type is compiled
/// in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Day(Map<String, Value>);

const FIELDS: [&str; 4] = ["commands", "exit", "errors", "agents"];

impl Default for Day {
    fn default() -> Self {
        let mut m = Map::new();
        for f in FIELDS {
            m.insert(f.into(), Value::Object(Map::new()));
        }
        m.insert("slowest".into(), Value::Null);
        Day(m)
    }
}

/// What one run adds.
#[derive(Debug, Clone, Copy)]
pub struct Run {
    pub command: &'static str,
    pub exit: &'static str,
    pub error: Option<&'static str>,
    pub agent: Option<&'static str>,
    pub bucket: usize,
}

impl Day {
    fn bump(&mut self, field: &str, key: &str, n: u64) {
        if let Some(Value::Object(m)) = self.0.get_mut(field) {
            let now = m.get(key).and_then(Value::as_u64).unwrap_or(0);
            m.insert(key.into(), json!(now + n));
        }
    }

    fn slowest(&self) -> Option<usize> {
        let s = self.0.get("slowest")?.as_str()?;
        BUCKETS.iter().position(|b| *b == s)
    }

    fn set_slowest(&mut self, bucket: Option<usize>) {
        let worst = self.slowest().max(bucket.map(|b| b.min(BUCKETS.len() - 1)));
        self.0
            .insert("slowest".into(), json!(worst.map(|i| BUCKETS[i])));
    }

    /// Count of `key` under `field`, 0 when absent.
    pub fn count(&self, field: &str, key: &str) -> u64 {
        self.0[field][key].as_u64().unwrap_or(0)
    }

    pub fn add(&mut self, run: &Run) {
        self.bump("commands", run.command, 1);
        self.bump("exit", run.exit, 1);
        if let Some(e) = run.error {
            self.bump("errors", e, 1);
        }
        if let Some(a) = run.agent {
            self.bump("agents", a, 1);
        }
        self.set_slowest(Some(run.bucket));
    }

    pub fn merge(&mut self, other: &Day) {
        for f in FIELDS {
            for (k, n) in other.0[f].as_object().into_iter().flatten() {
                self.bump(f, k, n.as_u64().unwrap_or(0));
            }
        }
        self.set_slowest(other.slowest());
    }

    /// The `cli_daily` properties: always all five, empty when idle.
    pub fn to_json(&self) -> Value {
        Value::Object(self.0.clone())
    }

    /// Reads a day back, keeping only keys from the closed sets.
    pub fn from_json(v: &Value) -> Day {
        let mut day = Day::default();
        for f in FIELDS {
            for (k, n) in v.get(f).and_then(Value::as_object).into_iter().flatten() {
                let known = match f {
                    "commands" => intern(commands_set(), k),
                    "exit" => intern(exit_set(), k),
                    "errors" => intern(error_set(), k),
                    _ => intern(agent_set(), k),
                };
                if let (Some(k), Some(n)) = (known, n.as_u64()) {
                    day.bump(f, k, n);
                }
            }
        }
        day.set_slowest(
            v.get("slowest")
                .and_then(Value::as_str)
                .and_then(|s| BUCKETS.iter().position(|b| *b == s)),
        );
        day
    }
}

/// The counters file: UTC day index (as a string key) -> that day's counts.
/// Keys that are not day numbers are dropped.
pub fn parse(raw: &str) -> Map<String, Value> {
    let mut out = Map::new();
    if let Ok(Value::Object(days)) = serde_json::from_str::<Value>(raw) {
        for (k, v) in &days {
            if k.parse::<u64>().is_ok() {
                out.insert(k.clone(), Day::from_json(v).to_json());
            }
        }
    }
    out
}

fn day_of(key: &str) -> u64 {
    key.parse().unwrap_or(u64::MAX)
}

/// Every day before `today`, merged; `None` when there is none.
pub fn before(days: &Map<String, Value>, today: u64) -> Option<Day> {
    let mut all: Option<Day> = None;
    for (k, v) in days {
        if day_of(k) < today {
            all.get_or_insert_with(Day::default)
                .merge(&Day::from_json(v));
        }
    }
    all
}

#[cfg(feature = "native")]
pub use native::*;

#[cfg(feature = "native")]
mod native {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    const FILE: &str = "usage.json";

    /// The command and the moment argument parsing finished.
    static STARTED: OnceLock<(&'static str, Instant)> = OnceLock::new();
    /// The launcher and the moment its agent was spawned.
    static SPAWNED: OnceLock<(&'static str, Instant)> = OnceLock::new();
    static AGENT_RETURNED: AtomicBool = AtomicBool::new(false);
    /// The last error printed (exit code, message), from `exit::print`.
    static LAST_ERROR: Mutex<Option<(i32, String)>> = Mutex::new(None);

    pub fn path(dir: &Path) -> PathBuf {
        dir.join(FILE)
    }

    /// Called right after argument parsing; nothing is counted before it.
    pub fn start(command: &str) {
        let _ = STARTED.set((command_slot(command), Instant::now()));
    }

    /// The registry name of this run's command, once parsed.
    pub fn command() -> Option<&'static str> {
        STARTED.get().map(|(c, _)| *c)
    }

    /// A launcher is about to spawn its agent: anyr's own time ends here.
    pub fn spawning(launcher: &str) {
        if let Some(id) = intern(
            agent_set(),
            crate::cmd::dispatch::canonical_command(launcher),
        ) {
            let _ = SPAWNED.set((id, Instant::now()));
        }
    }

    /// The agent exited; the run's code is now the agent's own.
    pub fn agent_returned() {
        AGENT_RETURNED.store(true, Ordering::Relaxed);
    }

    pub fn agent_exit() -> bool {
        AGENT_RETURNED.load(Ordering::Relaxed)
    }

    pub fn note_error(code: i32, message: &str) {
        if let Ok(mut slot) = LAST_ERROR.lock() {
            *slot = Some((code, message.to_string()));
        }
    }

    pub fn last_error() -> Option<(i32, String)> {
        LAST_ERROR.lock().ok().and_then(|s| s.clone())
    }

    /// This run as a counter row, or `None` before argument parsing.
    pub fn this_run(code: i32) -> Option<Run> {
        let (command, started) = *STARTED.get()?;
        let agent = SPAWNED.get().copied();
        let ended = agent.map(|(_, at)| at).unwrap_or_else(Instant::now);
        let from_agent = agent_exit();
        let message = last_error().map(|(_, m)| m).unwrap_or_default();
        Some(Run {
            command,
            exit: exit_slot(code, from_agent),
            error: (code != 0 && !from_agent).then(|| error_slot(code, &message)),
            agent: agent.map(|(a, _)| a),
            bucket: bucket(ended.saturating_duration_since(started).as_millis()),
        })
    }

    pub fn load(dir: &Path) -> Map<String, Value> {
        std::fs::read_to_string(path(dir))
            .map(|raw| parse(&raw))
            .unwrap_or_default()
    }

    /// Replace the file in one rename, so a parallel run never reads half of
    /// it; an empty map deletes it.
    pub fn store(dir: &Path, days: &Map<String, Value>) {
        let target = path(dir);
        if days.is_empty() {
            let _ = std::fs::remove_file(&target);
            return;
        }
        let tmp = dir.join(format!(".{FILE}.{}", std::process::id()));
        let _ = crate::config::ensure_private_dir(dir);
        let body = format!("{}\n", Value::Object(days.clone()));
        let written = crate::config::write_private(&tmp, body.as_bytes());
        if written
            .and_then(|()| std::fs::rename(&tmp, &target))
            .is_err()
        {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Run `f` holding the OS lock on `usage.lock`, so a parallel run never
    /// drops counts or resends a day it read before this one took it. `None`
    /// when the lock cannot be taken; nothing is written then.
    pub fn locked<T>(dir: &Path, f: impl FnOnce() -> T) -> Option<T> {
        let _ = crate::config::ensure_private_dir(dir);
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(dir.join("usage.lock")).ok()?;
        file.lock().ok()?;
        Some(f())
    }

    /// Callers hold `locked`.
    pub fn record(dir: &Path, today: u64, run: &Run) {
        let mut days = load(dir);
        let key = today.to_string();
        let mut day = days.get(&key).map(Day::from_json).unwrap_or_default();
        day.add(run);
        days.insert(key, day.to_json());
        store(dir, &days);
    }

    /// Every finished day, merged, removed from the file. Callers hold
    /// `locked`.
    pub fn take_before(dir: &Path, today: u64) -> Option<Day> {
        let mut days = load(dir);
        let past = before(&days, today)?;
        days.retain(|d, _| day_of(d) >= today);
        store(dir, &days);
        Some(past)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(command: &'static str, exit: &'static str, bucket: usize) -> Run {
        Run {
            command,
            exit,
            error: None,
            agent: None,
            bucket,
        }
    }

    #[test]
    fn only_registry_names_are_counted_and_the_rest_is_other() {
        assert_eq!(command_slot("models"), "models");
        assert_eq!(command_slot("cc"), "claude");
        assert_eq!(command_slot("model"), "models");
        assert_eq!(command_slot("sk-ar-v1-abcdef0123456789"), "other");
        assert_eq!(command_slot("/home/me/secret.txt"), "other");
    }

    #[test]
    fn exit_and_error_slots_come_from_closed_sets() {
        assert_eq!(exit_slot(0, false), "ok");
        assert_eq!(exit_slot(4, false), "auth");
        assert_eq!(exit_slot(130, false), "other");
        assert_eq!(exit_slot(1, true), "agent");
        assert_eq!(exit_slot(0, true), "ok");
        assert_eq!(error_slot(1, "upstream said HTTP 503: busy"), "http_503");
        assert_eq!(error_slot(1, "HTTP 418 teapot"), "http_other");
        assert_eq!(error_slot(7, "Could not reach anyrouter.dev"), "network");
    }

    #[test]
    fn buckets_split_at_the_documented_edges() {
        assert_eq!(bucket(99), 0);
        assert_eq!(bucket(100), 1);
        assert_eq!(bucket(9_999), 2);
        assert_eq!(bucket(59_999), 3);
        assert_eq!(bucket(60_000), 4);
    }

    #[test]
    fn a_hand_edited_file_cannot_add_keys() {
        // WHY: the file is user-writable; anything in it rides the next ping,
        // so only the compiled-in closed sets may come back out.
        let raw = r#"{"20000":{"commands":{"models":2,"my-secret-repo":5},
            "exit":{"ok":2,"/home/me":1},"errors":{"sk-ar-v1-x":1,"http_404":1},
            "agents":{"claude":1,"evil":1},"slowest":"forever","extra":"x"},"junk":{}}"#;
        let days = parse(raw);
        assert_eq!(days.len(), 1);
        let d = &days["20000"];
        assert_eq!(
            *d,
            json!({
                "commands": {"models": 2},
                "exit": {"ok": 2},
                "errors": {"http_404": 1},
                "agents": {"claude": 1},
                "slowest": null,
            })
        );
    }

    #[test]
    fn past_days_merge_and_today_stays() {
        let mut days = Map::new();
        for (d, r) in [
            (10, run("models", "ok", 1)),
            (11, run("models", "usage", 3)),
            (12, run("keys", "ok", 0)),
        ] {
            let mut day = Day::default();
            day.add(&r);
            days.insert(d.to_string(), day.to_json());
        }
        let past = before(&days, 12).unwrap();
        assert_eq!(past.count("commands", "models"), 2);
        assert_eq!(past.count("commands", "keys"), 0);
        assert_eq!(past.to_json()["slowest"], "<1m");
        assert_eq!(parse(&Value::Object(days.clone()).to_string()), days);
        assert!(before(&days, 10).is_none());
    }
}
