//! Crash capture: the panic hook, the breadcrumb ring and the crash files
//! the next run uploads as `cli_crash`.
//!
//! `install` is the one call `main` makes before argument parsing (a single
//! `panic::set_hook`). The release build has `panic = "abort"`, so the hook
//! writes its file before it returns. It writes only when telemetry was
//! already decided on for this run; a panic before that leaves nothing.
//!
//! A crash file holds the version, os, arch, channel, the registry name of
//! the command, the panic location cut to `crate/src/file.rs:line`, the
//! redacted message and the last 16 breadcrumbs: fixed strings from
//! `CRUMBS`, never a URL, path or argument.

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use serde_json::{json, Value};

/// Every breadcrumb there is. A crash file can only name these.
pub const CRUMBS: &[&str] = &[
    "config.read",
    "spawn:agent",
    "update.apply",
    "http:other",
    "http:github",
    "http:/v1/models",
    "http:/v1/chat",
    "http:/v1/messages",
    "http:/v1/responses",
    "http:/v1/embeddings",
    "http:/v1/keys",
    "http:/v1/key",
    "http:/v1/credits",
    "http:/v1/auth",
    "http:/v1/presets",
    "http:/v1/byok",
    "http:/v1/logs",
    "http:/v1/generation",
    "http:/v1/transactions",
    "http:/v1/relay",
    "http:/v1/artifacts",
    "http:/v1/apps",
    "http:/v1/mcp",
    "http:/v1/pool",
    "http:/v1/me",
    "http:/v1/decisions",
    "http:/v1/connections",
    "http:/v1/providers",
];

const RING: usize = 16;
/// Index + 1 into `CRUMBS`; 0 is an empty slot.
static TRAIL: [AtomicU8; RING] = [const { AtomicU8::new(0) }; RING];
static HEAD: AtomicUsize = AtomicUsize::new(0);

/// Most crash files kept; a new fingerprint past this is dropped.
pub const MAX_FILES: usize = 20;
/// Most `cli_crash` events one install sends per UTC day.
pub const MAX_PER_DAY: u64 = 5;

/// Leave a breadcrumb. No allocation, no lock: two atomic stores.
pub fn crumb(name: &str) {
    if let Some(i) = CRUMBS.iter().position(|c| *c == name) {
        let slot = HEAD.fetch_add(1, Ordering::Relaxed) % RING;
        TRAIL[slot].store(i as u8 + 1, Ordering::Relaxed);
    }
}

/// The breadcrumb for a request to `url`: its route, never the URL.
pub fn http_crumb(url: &str) {
    crumb(route(url));
}

fn route(url: &str) -> &'static str {
    if url.contains("github.com") || url.contains("githubusercontent.com") {
        return "http:github";
    }
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let seg = path
        .find("/v1/")
        .map(|at| path[at + 4..].split('/').next().unwrap_or_default());
    CRUMBS
        .iter()
        .find(|c| seg.is_some() && c.strip_prefix("http:/v1/") == seg)
        .copied()
        .unwrap_or("http:other")
}

/// The trail, oldest first.
pub fn trail() -> Vec<&'static str> {
    let head = HEAD.load(Ordering::Relaxed);
    (0..RING)
        .map(|k| TRAIL[(head + k) % RING].load(Ordering::Relaxed))
        .filter(|i| *i > 0)
        .filter_map(|i| CRUMBS.get(i as usize - 1).copied())
        .collect()
}

/// A crash record: what the hook writes and what is uploaded.
pub fn record(
    command: &str,
    location: &str,
    message: &str,
    crumbs: &[&str],
    version: &str,
) -> Value {
    let message = crate::report::redact(message);
    let location = crate::report::location_checked(location);
    json!({
        "fingerprint": crate::report::fingerprint(&location, &message),
        "count": 1,
        "sent": false,
        "version": version,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "channel": if version.contains("beta") { "beta" } else { "stable" },
        "command": crate::counters::command_slot(command),
        "location": location,
        "message": message,
        "breadcrumbs": crumbs.iter().filter(|c| CRUMBS.contains(c)).collect::<Vec<_>>(),
    })
}

/// A crash file read back from disk, with every field checked again: a
/// hand-edited file cannot put free text, a path or an unknown key in an
/// upload.
pub fn sanitize(v: &Value) -> Option<Value> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default();
    let crumbs: Vec<&str> = v
        .get("breadcrumbs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|c| CRUMBS.iter().find(|k| **k == c).copied())
        .collect();
    let version = s("version");
    let version_ok = !version.is_empty()
        && version.len() <= 40
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c));
    if !version_ok {
        return None;
    }
    let mut out = record(s("command"), s("location"), s("message"), &crumbs, version);
    out["count"] = json!(v.get("count").and_then(Value::as_u64).unwrap_or(1).max(1));
    out["sent"] = json!(v.get("sent").and_then(Value::as_bool) == Some(true));
    Some(out)
}

/// The `cli_crash` event for one sanitized record.
pub fn payload(install_id: &str, rec: &Value) -> Value {
    let mut props = json!({
        "$lib": "anyr-cli",
        "$process_person_profile": false,
        "$geoip_disable": true,
    });
    for k in [
        "version",
        "os",
        "arch",
        "channel",
        "fingerprint",
        "command",
        "location",
        "message",
        "breadcrumbs",
        "count",
    ] {
        props[k] = rec[k].clone();
    }
    json!({
        "api_key": crate::telemetry::POSTHOG_KEY,
        "event": "cli_crash",
        "distinct_id": install_id,
        "properties": props,
    })
}

#[cfg(feature = "native")]
pub use native::*;

#[cfg(feature = "native")]
mod native {
    use super::*;
    use std::path::{Path, PathBuf};

    pub fn dir(state_dir: &Path) -> PathBuf {
        state_dir.join("crash")
    }

    /// The one call before argument parsing.
    pub fn install() {
        std::panic::set_hook(Box::new(on_panic));
    }

    fn on_panic(info: &std::panic::PanicHookInfo<'_>) {
        let raw = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or_default();
        // Redaction works on a bounded copy; the file keeps 200 chars anyway.
        let message: String = raw.chars().take(4096).collect();
        let location = info
            .location()
            .map(|l| crate::report::location(l.file(), l.line()))
            .unwrap_or_default();
        // Save before printing: a closed stderr must not lose the report.
        let saved = crate::telemetry::gate_if_known()
            .filter(|_| still_on())
            .is_some_and(|state| {
                let command = crate::counters::command().unwrap_or("other");
                let rec = record(command, &location, &message, &trail(), crate::VERSION);
                save(&dir(state), &rec)
            });
        eprintln!("anyr crashed at {location}: {message}");
        if saved {
            eprintln!(
                "A redacted crash report was saved and goes out with the next run (`anyr telemetry off` stops that)."
            );
        }
    }

    /// `telemetry off` may have run since this run decided; and an OTLP
    /// endpoint has no crash form, so nothing is kept for it.
    fn still_on() -> bool {
        let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
        crate::key::load_config_if_present(&crate::config::resolve_config_path(None, &env))
            .is_none_or(|c| c.telemetry != Some(false) && c.telemetry_endpoint.is_none())
    }

    /// Write `rec`, or bump the count of the same fingerprint. One bug is
    /// sent once per version: a repeat keeps `sent` unless the version
    /// changed. A full directory drops a sent file to make room. Returns
    /// false when nothing was written.
    pub fn save(dir: &Path, rec: &Value) -> bool {
        if crate::config::ensure_private_dir(dir).is_err() {
            return false;
        }
        let fp = rec["fingerprint"].as_str().unwrap_or_default();
        let path = dir.join(format!("{fp}.json"));
        let now = crate::autoupdate::now_secs();
        let mut rec = rec.clone();
        if let Some(old) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        {
            rec["count"] = json!(old["count"].as_u64().unwrap_or(1) + 1);
            rec["first_seen"] = old["first_seen"].clone();
            rec["sent"] = json!(old["sent"] == true && old["version"] == rec["version"]);
        } else {
            let all = files(dir);
            if all.len() >= MAX_FILES {
                let Some(old) = all
                    .iter()
                    .find(|p| read(p).is_some_and(|v| v["sent"] == true))
                else {
                    return false;
                };
                let _ = std::fs::remove_file(old);
            }
            rec["first_seen"] = json!(now);
        }
        rec["last_seen"] = json!(now);
        write(&path, &rec)
    }

    /// Owner-only, replaced in one rename so a reader never sees half a file.
    fn write(path: &Path, rec: &Value) -> bool {
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        let ok = crate::config::write_private(&tmp, format!("{rec}\n").as_bytes())
            .and_then(|()| std::fs::rename(&tmp, path))
            .is_ok();
        if !ok {
            let _ = std::fs::remove_file(&tmp);
        }
        ok
    }

    /// Crash files, in directory order.
    pub fn files(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect()
    }

    pub fn read(path: &Path) -> Option<Value> {
        let raw = std::fs::read_to_string(path).ok()?;
        sanitize(&serde_json::from_str(&raw).ok()?)
    }

    /// The most recent crash (by `last_seen`), sanitized.
    pub fn newest(dir: &Path) -> Option<Value> {
        files(dir)
            .iter()
            .filter_map(|p| Some((read(p)?, p)))
            .max_by_key(|(_, p)| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH)
            })
            .map(|(v, _)| v)
    }

    /// Up to `room` unsent crashes as `cli_crash` events, each marked sent
    /// before it is handed over (a failed send is lost, never retried in a
    /// loop).
    pub fn take_unsent(dir: &Path, install_id: &str, room: u64) -> Vec<Value> {
        let mut out = Vec::new();
        for path in files(dir) {
            if out.len() as u64 >= room {
                break;
            }
            let Some(rec) = read(&path) else {
                continue;
            };
            if rec["sent"] == true {
                continue;
            }
            let Some(mut marked) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            else {
                continue;
            };
            marked["sent"] = json!(true);
            if write(&path, &marked) {
                out.push(payload(install_id, &rec));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_are_templates_never_urls() {
        assert_eq!(
            route("https://anyrouter.dev/api/v1/models?x=1"),
            "http:/v1/models"
        );
        assert_eq!(
            route("https://anyrouter.dev/api/v1/keys/9f2c"),
            "http:/v1/keys"
        );
        assert_eq!(route("http://10.0.0.5:8080/v1/secret-thing"), "http:other");
        assert_eq!(route("https://api.github.com/repos/x/y"), "http:github");
        assert_eq!(route("https://example.com/home/alice"), "http:other");
    }

    #[test]
    fn the_trail_keeps_the_last_sixteen_known_crumbs() {
        for _ in 0..20 {
            crumb("config.read");
        }
        crumb("not a crumb");
        crumb("spawn:agent");
        let t = trail();
        assert_eq!(t.len(), RING);
        assert_eq!(t.last(), Some(&"spawn:agent"));
    }

    #[test]
    fn a_hand_edited_crash_file_cannot_smuggle_text_out() {
        let evil = json!({
            "version": "0.1.44",
            "command": "sk-ar-v1-abcdefghijklmnop1234",
            "location": "/home/alice/src/x.rs:3",
            "message": "key sk-ar-v1-abcdefghijklmnop1234 at /Users/bob/a.txt",
            "breadcrumbs": ["config.read", "/home/alice", "http:/v1/models"],
            "extra": "rk_secret",
            "count": 2,
        });
        let rec = sanitize(&evil).unwrap();
        let body = payload("id", &rec).to_string();
        assert_eq!(crate::report::leaks(&body), None, "{body}");
        assert_eq!(rec["command"], "other");
        assert_eq!(
            rec["breadcrumbs"],
            json!(["config.read", "http:/v1/models"])
        );
        assert!(rec.get("extra").is_none());
        assert_eq!(rec["count"], 2);
        assert!(sanitize(&json!({"version": "/home/alice"})).is_none());
    }
}
