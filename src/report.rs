//! What a crash report or bug report may say about this machine, and the one
//! redaction function every outgoing payload and printed report goes through.
//!
//! `redact` keeps the shape of a message and drops anything that could
//! identify a person, a machine or a secret: key- and token-shaped words,
//! paths and file names, flags, quoted text (arguments, model ids, prompts),
//! e-mail addresses, URLs on hosts other than ours, and the literal home
//! directory, temp directory, username and hostname. The result is capped at
//! 200 characters.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Longest free text that leaves the machine.
pub const MAX_TEXT: usize = 200;

/// Hosts a URL may keep; any other becomes `<url>`.
const HOSTS: &[&str] = &["anyrouter.dev", "github.com", "posthog.com"];

/// Machine-specific strings to replace wherever they appear.
#[derive(Debug, Clone, Default)]
pub struct Scrub {
    literals: Vec<(String, &'static str)>,
}

impl Scrub {
    /// Keep `value` (trimmed) unless it is too short to be specific, longest
    /// first so a home dir is replaced before the username inside it.
    fn add(&mut self, value: &str, label: &'static str) {
        let v = value.trim().trim_end_matches(['/', '\\']);
        // A one- or two-letter name would mangle ordinary words.
        if v.chars().count() < 3 || self.literals.iter().any(|(l, _)| l == v) {
            return;
        }
        let at = self
            .literals
            .iter()
            .position(|(l, _)| l.len() < v.len())
            .unwrap_or(self.literals.len());
        self.literals.insert(at, (v.to_string(), label));
    }

    /// From an environment lookup (`HOME`, temp dirs, user and host names).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut s = Self::default();
        for (key, label) in [
            ("HOME", "~"),
            ("USERPROFILE", "~"),
            ("TMPDIR", "<tmp>"),
            ("TEMP", "<tmp>"),
            ("TMP", "<tmp>"),
            ("USER", "<user>"),
            ("USERNAME", "<user>"),
            ("LOGNAME", "<user>"),
            ("HOSTNAME", "<host>"),
            ("COMPUTERNAME", "<host>"),
        ] {
            if let Some(v) = get(key) {
                s.add(&v, label);
            }
        }
        s
    }

    /// This process: its environment, the OS temp dir and, on Linux, the
    /// hostname file.
    pub fn process() -> Self {
        let mut s = Self::from_lookup(|k| std::env::var(k).ok());
        #[cfg(feature = "native")]
        s.add(&std::env::temp_dir().to_string_lossy(), "<tmp>");
        #[cfg(all(feature = "native", unix))]
        {
            let mut buf = [0u8; 256];
            // SAFETY: the buffer is valid for its length; on success the
            // name is NUL-terminated within it.
            if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0 {
                let n = buf.iter().position(|b| *b == 0).unwrap_or(0);
                let host = String::from_utf8_lossy(&buf[..n]).into_owned();
                // `alice-mbp.local` also appears as `alice-mbp`.
                if let Some((short, _)) = host.split_once('.') {
                    s.add(short, "<host>");
                }
                s.add(&host, "<host>");
            }
        }
        s
    }
}

/// `redact_with` for this process.
pub fn redact(text: &str) -> String {
    redact_with(text, &Scrub::process())
}

/// The one redaction pass. Safe to run twice; the output never contains a
/// key prefix, a path separator inside a word, or more than `MAX_TEXT`
/// characters.
pub fn redact_with(text: &str, scrub: &Scrub) -> String {
    let mut s = text.to_string();
    for (literal, label) in &scrub.literals {
        if s.contains(literal.as_str()) {
            s = s.replace(literal.as_str(), label);
        }
    }
    let s = strip_quoted(&s);
    let mut out = String::with_capacity(s.len());
    let mut word = String::new();
    for c in s.chars() {
        if c.is_whitespace() {
            out.push_str(&redact_word(&word));
            word.clear();
            out.push(c);
        } else {
            word.push(c);
        }
    }
    out.push_str(&redact_word(&word));
    cap(&out, MAX_TEXT)
}

/// Quoted spans become `"…"`: they hold arguments, model ids, file names,
/// prompts and command lines. `"`, `` ` ``, `“…”` always open a quote; `'`
/// and `‘…’` only at the start of a word, so "don't" stays a word.
fn strip_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut close: Option<char> = None;
    let mut prev = ' ';
    for c in s.chars() {
        match close {
            Some(q) if c == q => {
                out.push('…');
                out.push('"');
                close = None;
            }
            Some(_) => {}
            None => {
                let starts_word = prev.is_whitespace() || "([{=:".contains(prev);
                close = match c {
                    '"' | '`' => Some(c),
                    '“' => Some('”'),
                    '\'' if starts_word => Some('\''),
                    '‘' if starts_word => Some('’'),
                    _ => None,
                };
                out.push(if close.is_some() { '"' } else { c });
            }
        }
        prev = c;
    }
    if close.is_some() {
        out.push('…');
        out.push('"');
    }
    out
}

fn redact_word(word: &str) -> String {
    let edge = |c: char| "()[]{}'\",;:.!?".contains(c);
    let core = word.trim_matches(edge);
    if core.is_empty() {
        return word.to_string();
    }
    let start = word.find(core).unwrap_or(0);
    let (head, tail) = (&word[..start], &word[start + core.len()..]);
    match classify(core) {
        Some(r) => format!("{head}{r}{tail}"),
        None => word.to_string(),
    }
}

/// Placeholders this module writes; they pass through a second run.
const PLACEHOLDERS: &[&str] = &[
    "<key>", "<path>", "<url>", "<email>", "<flag>", "<file>", "<id>", "<user>", "<host>", "<tmp>",
    "~", "…",
];

fn ours(host: &str) -> bool {
    HOSTS
        .iter()
        .any(|h| host == *h || host.strip_suffix(h).is_some_and(|p| p.ends_with('.')))
}

/// What a word becomes, or `None` to keep it. Only plain words survive:
/// letters (up to 20), numbers and versions, our own hosts. Anything mixing
/// letters with digits, `-`, `_`, `.`, `:`, `=`, `+` is an id, a token, a
/// host, a model or a value, and is replaced.
fn classify(core: &str) -> Option<String> {
    let lower = core.to_ascii_lowercase();
    if let Some(at) = lower.find("://") {
        let host = lower[at + 3..]
            .split(['/', ':', '?', '#'])
            .next()
            .unwrap_or_default();
        return Some(if ours(host) && !host.contains('@') {
            format!("{}://{host}", &lower[..at])
        } else {
            "<url>".into()
        });
    }
    if PLACEHOLDERS.contains(&core) || ours(&lower) {
        return None;
    }
    let keyish = [
        "sk-", "ak_", "rk_", "pk_", "ghp_", "gho_", "xox", "bearer", "eyj",
    ]
    .iter()
    .any(|m| lower.contains(m));
    let label = if keyish {
        "<key>"
    } else if core.contains(['/', '\\']) || core.starts_with('~') {
        "<path>"
    } else if core.contains('@') {
        "<email>"
    } else if core.starts_with('-')
        && core[1..].starts_with(|c: char| c.is_alphabetic() || c == '-')
    {
        "<flag>"
    } else if core.chars().all(char::is_alphabetic) && core.chars().count() <= 20 {
        return None;
    } else if core.chars().all(|c| c.is_ascii_digit() || c == '.') && core.matches('.').count() <= 2
    {
        // Numbers and versions; four dotted parts is an IP address.
        return None;
    } else if looks_like_file(core) {
        "<file>"
    } else {
        "<id>"
    };
    Some(label.into())
}

/// `name.ext` with a short alphabetic-led extension.
fn looks_like_file(core: &str) -> bool {
    let Some((stem, ext)) = core.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && (1..=5).contains(&ext.len())
        && ext.starts_with(|c: char| c.is_ascii_alphabetic())
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
}

fn cap(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

/// `file:line` of a panic, cut down to `crate/src/...`: a registry checkout,
/// a `/rustc/<hash>/library` path or a build directory never leaves.
pub fn location(file: &str, line: u32) -> String {
    let p = file.replace('\\', "/");
    // Only toolchain and cargo-managed dirs keep their crate dir; any other
    // absolute path keeps its file name alone.
    // A relative `crate/src/...` is already cut (a crash file read back).
    let relative = !p.starts_with(['/', '~', '.']) && !p.contains(':');
    let managed =
        relative || p.starts_with("/rustc/") || p.contains("/.cargo/") || p.contains("/.rustup/");
    let short = if let Some(rest) = p.strip_prefix("src/") {
        format!("anyr-cli/src/{rest}")
    } else if let Some(at) = p.rfind("/src/").filter(|_| managed) {
        let krate = p[..at].rsplit('/').next().unwrap_or_default();
        format!("{krate}/src/{}", &p[at + 5..])
    } else {
        p.rsplit('/').next().unwrap_or_default().to_string()
    };
    // Whatever is left must be a plain relative source path that names no
    // user or machine (`/home/alice/src/x.rs` would keep `alice`).
    let scrub = Scrub::process();
    let named = scrub
        .literals
        .iter()
        .any(|(l, _)| !l.contains(['/', '\\']) && short.contains(l.as_str()));
    let safe = !named
        && !short.is_empty()
        && !short.contains(':')
        && !short.starts_with(['/', '.', '~'])
        && short
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c));
    if safe {
        format!("{short}:{line}")
    } else {
        format!("<path>:{line}")
    }
}

/// `location` for a `file:line` string read back from a file.
pub fn location_checked(loc: &str) -> String {
    match loc
        .rsplit_once(':')
        .and_then(|(f, l)| Some((f, l.parse().ok()?)))
    {
        Some((file, line)) => location(file, line),
        None => "<path>:0".into(),
    }
}

/// 12 hex chars naming one bug across versions and machines: the location
/// plus the redacted message with every number folded to `0`.
pub fn fingerprint(location: &str, redacted: &str) -> String {
    let mut shape = String::with_capacity(redacted.len());
    let mut in_digits = false;
    for c in redacted.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                shape.push('0');
            }
            in_digits = true;
        } else {
            shape.push(c);
            in_digits = false;
        }
    }
    let mut h = Sha256::new();
    h.update(location.as_bytes());
    h.update(b"\n");
    h.update(shape.as_bytes());
    h.finalize()[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A `last-error.json` record, every field from a closed set except the
/// redacted message.
pub fn last_error(command: &str, code: i32, message: &str, version: &str, time: u64) -> Value {
    let message = redact(message);
    let command = crate::counters::command_slot(command);
    json!({
        "command": command,
        "exit": code,
        "code": crate::counters::exit_slot(code, false),
        "error": crate::counters::error_slot(code, &message),
        "message": message,
        "version": version,
        "time": time,
        "fingerprint": fingerprint(&format!("error:{command}:{code}"), &message),
    })
}

/// A record read back from disk, rebuilt so an edited file cannot add text.
pub fn sanitize_last_error(v: &Value) -> Option<Value> {
    let version = v.get("version")?.as_str()?;
    if !clean_version(version) {
        return None;
    }
    Some(last_error(
        v.get("command")?.as_str()?,
        v.get("exit")?.as_i64()? as i32,
        v.get("message").and_then(Value::as_str).unwrap_or_default(),
        version,
        v.get("time").and_then(Value::as_u64).unwrap_or(0),
    ))
}

/// A release version read back from a user-writable file: `0.1.44`,
/// `0.1.44-beta.7`, `0.1.44-rc.1` or this build's own version, nothing
/// else (a prerelease part could otherwise carry any text).
pub fn clean_version(s: &str) -> bool {
    let core = s.strip_prefix('v').unwrap_or(s);
    let (head, pre) = match core.split_once('-') {
        Some((h, p)) => (h, Some(p)),
        None => (core, None),
    };
    let parts: Vec<&str> = head.split('.').collect();
    let shaped = !s.is_empty()
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c))
        && parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        && pre.is_none_or(|p| p.starts_with("beta") || p.starts_with("rc"));
    shaped || s == crate::VERSION
}

#[cfg(feature = "native")]
pub use native::*;

#[cfg(feature = "native")]
mod native {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    /// A version string or nothing.
    fn version_or_null(v: Option<&str>) -> Value {
        v.filter(|s| clean_version(s))
            .map_or(Value::Null, |s| json!(s))
    }

    pub fn last_error_path(dir: &Path) -> PathBuf {
        dir.join("last-error.json")
    }

    pub fn save_last_error(dir: &Path, run: &crate::counters::Run, code: i32, message: &str) {
        let rec = last_error(
            run.command,
            code,
            message,
            crate::VERSION,
            crate::autoupdate::now_secs(),
        );
        let _ = crate::config::write_private(&last_error_path(dir), format!("{rec}\n").as_bytes());
    }

    fn read(path: &Path) -> Option<Value> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    /// The one report bundle (`anyr report`): this machine's anyr facts and,
    /// with `last`, the last error and the newest crash. Everything is
    /// rebuilt from closed sets or passed through `redact`.
    pub fn bundle(env: &BTreeMap<String, String>, last: bool) -> Value {
        let dir = crate::autoupdate::state_dir(env);
        let update = crate::autoupdate::UpdateState::load(env);
        let mut agents = serde_json::Map::new();
        if let Some(Value::Object(rows)) = read(&dir.join("tool-versions.json")) {
            for (key, row) in rows {
                let base = key.rsplit(['/', '\\']).next().unwrap_or_default();
                let base = base.strip_suffix(".exe").unwrap_or(base);
                if let (Some(name), Some(major)) = (
                    crate::registry::LAUNCHERS.iter().find(|l| **l == base),
                    row.get("major").and_then(Value::as_u64),
                ) {
                    agents.insert(name.to_string(), json!(major));
                }
            }
        }
        let (last_error, crash) = if last {
            (
                read(&last_error_path(&dir)).and_then(|v| sanitize_last_error(&v)),
                crate::crash::newest(&crate::crash::dir(&dir)),
            )
        } else {
            (None, None)
        };
        let fingerprint = crash
            .as_ref()
            .or(last_error.as_ref())
            .map(|v| v["fingerprint"].clone());
        json!({
            "version": crate::VERSION,
            "channel": if crate::VERSION.contains("beta") { "beta" } else { "stable" },
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "install_method": std::env::current_exe()
                .map(|exe| crate::telemetry::install_method(&exe))
                .unwrap_or("manual"),
            "fingerprint": fingerprint,
            "last_error": last_error,
            "crash": crash,
            "update": {
                "channel": update.channel.as_deref().filter(|c| matches!(*c, "stable" | "beta")),
                "latest_seen": version_or_null(update.latest_seen.as_deref()),
                "pending_version": version_or_null(update.pending_version.as_deref()),
                "skip_version": version_or_null(update.skip_version.as_deref()),
                "last_error": update.last_error.as_deref().map(redact),
                "last_check_age_secs": (update.last_check > 0)
                    .then(|| crate::autoupdate::now_secs().saturating_sub(update.last_check)),
            },
            "agents": agents,
        })
    }
}

/// What no payload, crash file or report may contain, as a check tests and
/// the uploader share.
pub fn leaks(text: &str) -> Option<&'static str> {
    const NEEDLES: &[&str] = &["sk-ar-", "/home/", "/Users/", "\\Users\\", "C:\\Users"];
    if let Some(n) = NEEDLES.iter().find(|n| text.contains(**n)) {
        return Some(n);
    }
    // `ak_` / `rk_` followed by a token character, anywhere.
    for marker in ["ak_", "rk_"] {
        let mut rest = text;
        while let Some(at) = rest.find(marker) {
            let next = rest[at + 3..].chars().next();
            if next.is_some_and(|c| c.is_ascii_alphanumeric()) {
                return Some(if marker == "ak_" { "ak_" } else { "rk_" });
            }
            rest = &rest[at + 3..];
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrub() -> Scrub {
        Scrub::from_lookup(|k| match k {
            "HOME" => Some("/home/alice".into()),
            "TMPDIR" => Some("/var/folders/xy/T/".into()),
            "USER" => Some("alice".into()),
            "HOSTNAME" => Some("alice-laptop".into()),
            _ => None,
        })
    }

    #[test]
    fn keys_paths_flags_and_quoted_text_are_removed() {
        let r = |t: &str| redact_with(t, &scrub());
        assert_eq!(
            r("bad key sk-ar-v1-abcdefghijklmnop1234 for alice@alice-laptop"),
            "bad key <key> for <email>"
        );
        assert_eq!(
            r("could not read /home/alice/notes.txt: denied"),
            "could not read <path>: denied"
        );
        assert_eq!(r("open C:\\Users\\bob\\x.yaml"), "open <path>");
        assert_eq!(
            r("unknown model \"acme/secret-model\""),
            "unknown model \"…\""
        );
        assert_eq!(
            r("Flag --model requires a value."),
            "Flag <flag> requires a value."
        );
        assert_eq!(r("wrote notes.txt"), "wrote <file>");
        assert_eq!(
            r("ak_live123 and rk_abc and break_even"),
            "<key> and <key> and <key>"
        );
        assert_eq!(r("in /var/folders/xy/T/run"), "in <path>");
        assert_eq!(r("user alice on alice-laptop"), "user <user> on <host>");
    }

    #[test]
    fn our_hosts_and_plain_words_survive() {
        let r = |t: &str| redact_with(t, &scrub());
        assert_eq!(
            r("Could not reach https://anyrouter.dev/api/v1/keys/9f: timed out"),
            "Could not reach https://anyrouter.dev: timed out"
        );
        assert_eq!(
            r("Could not reach http://10.0.0.5:8080/v1"),
            "Could not reach <url>"
        );
        assert_eq!(
            r("HTTP 404 not found on anyrouter.dev, version 0.1.44"),
            "HTTP 404 not found on anyrouter.dev, version 0.1.44"
        );
        assert_eq!(
            r("called `Option::unwrap()` on a `None` value"),
            "called \"…\" on a \"…\" value"
        );
    }

    #[test]
    fn text_is_capped_and_redaction_is_idempotent() {
        let long = "word ".repeat(100);
        let once = redact_with(&long, &scrub());
        assert_eq!(once.chars().count(), MAX_TEXT + 1);
        let msg = "x sk-ar-v1-abc /home/alice/a \"q\" --flag";
        let once = redact_with(msg, &scrub());
        assert_eq!(redact_with(&once, &scrub()), once);
    }

    #[test]
    fn locations_keep_only_the_crate_relative_path() {
        assert_eq!(location("src/commands.rs", 7), "anyr-cli/src/commands.rs:7");
        assert_eq!(
            location(
                "/home/alice/.cargo/registry/src/index.crates.io-6f17d22bba15001f/ureq-2.12.1/src/response.rs",
                3
            ),
            "ureq-2.12.1/src/response.rs:3"
        );
        assert_eq!(
            location("/rustc/48a229cea/library/core/src/option.rs", 9),
            "core/src/option.rs:9"
        );
        assert_eq!(
            location("C:\\Users\\bob\\cli\\src\\main.rs", 2),
            "main.rs:2"
        );
        assert_eq!(location("/home/alice/proj/src/main.rs", 2), "main.rs:2");
        assert_eq!(location("/home/alice/x.rs", 1), "x.rs:1");
        assert_eq!(location("weird:path", 1), "<path>:1");
        // Cutting twice changes nothing (a crash file read back).
        assert_eq!(
            location_checked("ureq-2.12.1/src/response.rs:3"),
            "ureq-2.12.1/src/response.rs:3"
        );
        assert_eq!(
            location_checked("anyr-cli/src/commands.rs:7"),
            "anyr-cli/src/commands.rs:7"
        );
    }

    #[test]
    fn one_bug_is_one_fingerprint_whatever_the_numbers() {
        let a = fingerprint("anyr-cli/src/x.rs:1", "index 3 out of range for 10");
        let b = fingerprint("anyr-cli/src/x.rs:1", "index 41 out of range for 7");
        let c = fingerprint("anyr-cli/src/x.rs:2", "index 3 out of range for 10");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 12);
    }
}

/// The privacy property, fuzzed: key-shaped strings and paths injected into
/// argv, env, config values and panic or error messages never reach a
/// serialized payload of any event type.
#[cfg(test)]
mod fuzz {
    use super::*;
    use crate::counters::{command_slot, error_slot, exit_slot, Day, Run};
    use crate::telemetry::{payload, Kind};
    use serde_json::Value;

    pub(crate) const CASES: usize = 600;

    /// xorshift64*: deterministic, no dev-dependency.
    pub(crate) struct Rng(u64);
    impl Rng {
        pub(crate) fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        pub(crate) fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
            &xs[(self.next() % xs.len() as u64) as usize]
        }
        pub(crate) fn word(&mut self, min: usize, max: usize) -> String {
            const A: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            let n = min + (self.next() as usize) % (max - min + 1);
            (0..n).map(|_| *self.pick(A) as char).collect()
        }
    }

    /// One injected secret or path, and the text it hides in.
    pub(crate) fn inject(rng: &mut Rng, secrets: &mut Vec<String>) -> String {
        let w = rng.word(8, 30);
        let secret = match rng.next() % 19 {
            9 => format!("eyJhbGciOiJIUzI1NiJ9.eyJ{w}.sig{w}"),
            10 => format!("AKIA{w}+/abc={w}=="),
            11 => format!("password={w}"),
            12 => format!("{}-mbp", rng.word(5, 9)),
            13 => format!("192.168.{}.{}:8080", rng.next() % 255, rng.next() % 255),
            14 => format!(
                "10.{}.{}.{}",
                rng.next() % 255,
                rng.next() % 255,
                rng.next() % 255
            ),
            15 => format!("gpt-{}-mini", rng.word(3, 6)),
            16 => format!("{:x}", rng.next()),
            17 => format!("{}_private-repo", rng.word(5, 9)),
            18 => format!(
                "{}.{}",
                rng.word(8, 12),
                rng.pick(&["env", "pem", "yaml", "txt"])
            ),
            0 => format!("sk-ar-v1-{w}"),
            1 => format!("ak_{w}"),
            2 => format!("rk_{w}"),
            3 => format!("/home/{}/{}.txt", rng.word(3, 8), w),
            4 => format!("/Users/{}/{}", rng.word(3, 8), w),
            5 => format!("C:\\Users\\{}\\{}", rng.word(3, 8), w),
            6 => format!("Bearer sk-ar-v1-{w}"),
            7 => format!("{}/{}", std::env::temp_dir().display(), w),
            _ => format!("https://{}.internal/{w}?key=ak_{w}", rng.word(3, 8)),
        };
        secrets.push(secret.clone());
        let wrap = *rng.pick(&[
            "{}", "\"{}\"", "`{}`", "({})", "x={}", "{}:", "key:{}", "'{}'", "<{}>", "{},next",
            "“{}”", "‘{}’", "token {}", "{}é",
        ]);
        wrap.replace("{}", &secret)
    }

    pub(crate) fn sentence(rng: &mut Rng, secrets: &mut Vec<String>) -> String {
        let n = 1 + rng.next() % 6;
        (0..n)
            .map(|_| match rng.next() % 3 {
                0 => inject(rng, secrets),
                _ => rng
                    .pick(&[
                        "could not",
                        "read",
                        "HTTP 404",
                        "failed",
                        "model",
                        "at",
                        "é日本",
                    ])
                    .to_string(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Fails with the offending payload and the secret it carried.
    pub(crate) fn assert_clean(body: &str, secrets: &[String]) {
        assert_eq!(leaks(body), None, "leak in {body}");
        for s in secrets {
            // The bare token too: a secret without its marker still leaks.
            let tail = s.rsplit(['/', '\\', '_', '-']).next().unwrap_or(s);
            assert!(!body.contains(s.as_str()), "secret {s} in {body}");
            assert!(
                tail.len() < 8 || !body.contains(tail),
                "tail of {s} in {body}"
            );
        }
    }

    #[test]
    fn no_payload_of_any_event_carries_an_injected_secret_or_path() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut checked = 0;
        for case in 0..CASES {
            let mut secrets = Vec::new();
            // argv: a command that may itself be a secret, flags, values.
            let argv: Vec<String> = (0..1 + rng.next() % 5)
                .map(|i| {
                    if i == 0 && rng.next().is_multiple_of(2) {
                        rng.pick(&["models", "claude", "keys", "chat"]).to_string()
                    } else {
                        inject(&mut rng, &mut secrets)
                    }
                })
                .collect();
            // env and config values: never read into a payload, but a run
            // with them set must still produce clean payloads.
            let env_value = inject(&mut rng, &mut secrets);
            let config_value = inject(&mut rng, &mut secrets);
            let message = format!(
                "{} {} {}",
                sentence(&mut rng, &mut secrets),
                env_value,
                config_value
            );
            let code = *rng.pick(&[1, 2, 3, 4, 7, 130]);
            let mut day = Day::default();
            day.add(&Run {
                command: command_slot(&argv[0]),
                exit: exit_slot(code, false),
                error: Some(error_slot(code, &message)),
                agent: None,
                bucket: (rng.next() % 5) as usize,
            });
            let user = rng.word(10, 14);
            secrets.push(user.clone());
            let panic_file = rng
                .pick(&[
                    "src/commands.rs".to_string(),
                    format!("/home/{user}/src/x.rs"),
                    format!("/home/x/{user}/src/x.rs"),
                    format!("/Users/{user}/.cargo/registry/src/i/ureq-2/src/a.rs"),
                    format!("C:\\Users\\{user}\\x\\src\\b.rs"),
                ])
                .clone();
            let crash = crate::crash::record(
                &argv[0],
                &location(&panic_file, 7),
                &message,
                &["config.read", "http:/v1/models"],
                crate::VERSION,
            );
            let id = "0123456789abcdef0123456789abcdef";
            let bodies = [
                payload(
                    id,
                    &Kind::Install,
                    crate::VERSION,
                    "linux",
                    "x86_64",
                    Value::Null,
                ),
                payload(
                    id,
                    &Kind::Update {
                        from: crate::telemetry::State::from_json(
                            &serde_json::json!({ "install_id": id, "last_version": argv[0] })
                                .to_string(),
                        )
                        .last_version
                        .unwrap_or_default(),
                    },
                    crate::VERSION,
                    "linux",
                    "x86_64",
                    Value::Null,
                ),
                payload(
                    id,
                    &Kind::Daily,
                    crate::VERSION,
                    "linux",
                    "x86_64",
                    day.to_json(),
                ),
                crate::crash::payload(id, &crash),
                crate::crash::payload(id, &crate::crash::sanitize(&crash).unwrap()),
                Value::String(redact(&message)),
                last_error(&argv[0], code, &message, crate::VERSION, 0),
                sanitize_last_error(&last_error(&argv[0], code, &message, crate::VERSION, 0))
                    .unwrap(),
            ];
            for b in &bodies {
                assert_clean(&b.to_string(), &secrets);
                checked += 1;
            }
            let _ = case;
        }
        assert!(checked >= CASES * 8);
    }
}
