//! Background self-update: when to check, the state file, and the binary swap.
//!
//! A normal command never waits on the network. At most once per interval it
//! spawns a detached `anyr update --auto`, which downloads and verifies the
//! new build into a staging file beside the binary and records it as pending.
//! The next run swaps it in with a rename, keeping the previous build for
//! `anyr update --rollback`. A running process keeps its own image, so an
//! agent launched by an older `anyr` is never disturbed.
//!
//! Lifecycle, for the two long-lived contexts (the launcher from bare `anyr`
//! and coding-agent launches such as `anyr claude`):
//!
//! * Check timing: `on_startup` runs before either context starts (see
//!   `commands.rs`, the menu path and the main dispatch). It spawns the
//!   detached `anyr update --auto` child only when the persisted last check
//!   is older than the interval (default a day) and records `last_check`
//!   before any network work, so one spawn per interval total.
//! * While running: the menu and the agent parent also run a ticker
//!   (`spawn_periodic_check`) that re-fires the same detached child on each
//!   interval boundary about once every 60 seconds. The child has its own
//!   process group and stdio, so the parent neither waits nor inherits it.
//! * Staging: the child only ever downloads beside the binary
//!   (`.anyr.pending`) under `update.lock`, verifies the checksum, and
//!   records it as pending. It never renames over the binary, so a running
//!   launcher or agent keeps its own image untouched.
//! * Apply: on the next start (launcher restart, next `anyr claude`, or a
//!   menu "Launch" click) the staged build is re-hashed and swapped in with
//!   a hard link plus rename, keeping the old build as `.anyr.prev` for
//!   `update --rollback`. A stale, tampered, cross-channel, or not-newer
//!   build is discarded instead of installed.
//! * Opt-outs: CI, `ANYR_NO_AUTO_UPDATE`, `ANYR_NO_UPDATE`, config
//!   `update.auto`, non-terminal output (`--json`, `--quiet`, piped stdio
//!   without `ANYR_AUTO_UPDATE=1`), cargo `target/` dev builds, and any
//!   `anyr` run inside a launched agent (`ANYR_LAUNCHER_CHILD`). Failures
//!   never print; `update --status` shows them.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::channel::Channel;
use crate::config::resolve_config_path;
use crate::key::load_config_if_present;
use crate::parse::{get_string_flag, ParsedArgs};
use crate::VERSION;

/// How often a command may start a background check.
pub const DEFAULT_UPDATE_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// Set on the detached checker so it never spawns another one.
pub const ENV_AUTO_CHILD: &str = "ANYR_AUTO_CHILD";
/// Set in every coding agent `anyr` launches, so an `anyr` the agent runs
/// (Pi reads its key through one) never updates or prints mid-session.
pub const ENV_LAUNCHER_CHILD: &str = "ANYR_LAUNCHER_CHILD";
/// Mock release server for tests. Honoured only for loopback hosts.
pub const ENV_UPDATE_BASE_URL: &str = "ANYR_UPDATE_BASE_URL";

const STATE_FILE: &str = "update-state.json";

/// What the updater remembers between runs (`~/.anyrouter/update-state.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateState {
    /// Unix seconds of the last background check, successful or not.
    pub last_check: u64,
    /// Newest installable version the last check saw on the channel.
    pub latest_seen: Option<String>,
    /// Verified build waiting beside the binary, applied on the next run.
    pub pending_path: Option<String>,
    pub pending_version: Option<String>,
    pub pending_sha256: Option<String>,
    /// Channel the last check used.
    pub channel: Option<String>,
    /// Why the last check or apply failed. Shown by `anyr update --status`.
    pub last_error: Option<String>,
    /// Version rolled back from; auto-update skips it until a newer one ships.
    pub skip_version: Option<String>,
    /// Manual command for installs anyr must not replace (Homebrew, npm,
    /// a read-only dir).
    pub hint: Option<String>,
    /// Version the hint was last printed for, so it prints once.
    pub notified: Option<String>,
}

fn opt(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl UpdateState {
    pub fn path(env: &BTreeMap<String, String>) -> PathBuf {
        state_dir(env).join(STATE_FILE)
    }

    pub fn load(env: &BTreeMap<String, String>) -> Self {
        fs::read_to_string(Self::path(env))
            .ok()
            .map(|raw| Self::from_json(&raw))
            .unwrap_or_default()
    }

    pub fn from_json(raw: &str) -> Self {
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
            return Self::default();
        };
        Self {
            last_check: map.get("last_check").and_then(Value::as_u64).unwrap_or(0),
            latest_seen: opt(&map, "latest_seen"),
            pending_path: opt(&map, "pending_path"),
            pending_version: opt(&map, "pending_version"),
            pending_sha256: opt(&map, "pending_sha256"),
            channel: opt(&map, "channel"),
            last_error: opt(&map, "last_error"),
            skip_version: opt(&map, "skip_version"),
            hint: opt(&map, "hint"),
            notified: opt(&map, "notified"),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "last_check": self.last_check,
            "latest_seen": self.latest_seen,
            "pending_path": self.pending_path,
            "pending_version": self.pending_version,
            "pending_sha256": self.pending_sha256,
            "channel": self.channel,
            "last_error": self.last_error,
            "skip_version": self.skip_version,
            "hint": self.hint,
            "notified": self.notified,
        })
    }

    pub fn save(&self, env: &BTreeMap<String, String>) {
        let dir = state_dir(env);
        let _ = crate::config::ensure_private_dir(&dir);
        let body = format!(
            "{}\n",
            serde_json::to_string_pretty(&self.to_json()).unwrap_or_default()
        );
        let path = dir.join(STATE_FILE);
        // Temp file + rename: a reader never sees a half-written file, and a
        // crash mid-write leaves the previous state intact.
        let tmp = dir.join(format!("{STATE_FILE}.tmp.{}", std::process::id()));
        let _ = fs::remove_file(&tmp);
        let written = crate::config::open_private(&tmp, true).and_then(|mut f| {
            use std::io::Write;
            f.write_all(body.as_bytes())
        });
        if written.is_err() || fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }

    pub fn clear_pending(&mut self) {
        self.pending_path = None;
        self.pending_version = None;
        self.pending_sha256 = None;
    }

    fn checked_within(&self, interval: u64, now: u64) -> bool {
        interval > 0 && self.last_check > 0 && now.saturating_sub(self.last_check) < interval
    }
}

pub fn state_dir(env: &BTreeMap<String, String>) -> PathBuf {
    resolve_config_path(None, env)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn update_interval_secs(env: &BTreeMap<String, String>) -> u64 {
    env.get("ANYR_UPDATE_INTERVAL_SECS")
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_UPDATE_INTERVAL_SECS)
}

/// True when the background check ran within the interval.
pub fn checked_recently(state: &UpdateState, env: &BTreeMap<String, String>) -> bool {
    state.checked_within(update_interval_secs(env), now_secs())
}

pub fn env_flag(env: &BTreeMap<String, String>, key: &str) -> Option<bool> {
    let raw = env.get(key)?.trim().to_ascii_lowercase();
    match raw.as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// `CI` counts when set to anything but an explicit false.
fn in_ci(env: &BTreeMap<String, String>) -> bool {
    env.get("CI")
        .map(|v| !v.trim().is_empty() && env_flag(env, "CI") != Some(false))
        .unwrap_or(false)
}

/// Auto-update is on unless turned off. `ANYR_NO_AUTO_UPDATE`,
/// `ANYR_NO_UPDATE` and `CI` always win; `ANYR_AUTO_UPDATE` beats the config.
pub fn auto_update_enabled(env: &BTreeMap<String, String>) -> bool {
    if env_flag(env, "ANYR_NO_AUTO_UPDATE") == Some(true)
        || env_flag(env, "ANYR_NO_UPDATE") == Some(true)
        || in_ci(env)
    {
        return false;
    }
    if let Some(v) = env_flag(env, "ANYR_AUTO_UPDATE") {
        return v;
    }
    let path = resolve_config_path(None, env);
    load_config_if_present(&path)
        .map(|c| c.auto_update())
        .unwrap_or(true)
}

/// Facts about this run that decide whether auto-update may touch anything.
#[derive(Debug, Clone, Copy)]
pub struct RunContext {
    /// stdin, stdout and stderr are all terminals.
    pub tty: bool,
    /// The running binary is a cargo `target/` build.
    pub dev_build: bool,
}

impl RunContext {
    #[cfg(feature = "native")]
    pub fn detect() -> Self {
        use std::io::IsTerminal;
        Self {
            tty: crate::term::is_interactive() && std::io::stderr().is_terminal(),
            dev_build: std::env::current_exe()
                .map(|exe| is_cargo_target_build(&exe))
                .unwrap_or(false),
        }
    }
}

/// Why auto-update must stay out of this run, or `None` when it may proceed.
pub fn skip_reason(
    command: &str,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    ctx: RunContext,
) -> Option<&'static str> {
    if matches!(
        command,
        "upgrade" | "update" | "help" | "--help" | "-h" | "--version" | "-v" | "completion"
    ) || parsed.flag_true("help")
    {
        return Some("command");
    }
    if env.contains_key(ENV_AUTO_CHILD) {
        return Some("update child");
    }
    if env.contains_key(ENV_LAUNCHER_CHILD) {
        return Some("launched by anyr");
    }
    if env.contains_key("ANYR_RELAY_DETACHED") {
        return Some("relay");
    }
    if !auto_update_enabled(env) {
        return Some("disabled");
    }
    if crate::term::machine_output(parsed, env)
        || get_string_flag(&parsed.flags, "output").as_deref() == Some("json")
    {
        return Some("agent mode");
    }
    // A pipe or an agent shell: stay quiet unless explicitly allowed.
    if !ctx.tty && env_flag(env, "ANYR_AUTO_UPDATE") != Some(true) {
        return Some("not a tty");
    }
    if ctx.dev_build {
        return Some("dev build");
    }
    None
}

/// A binary under a cargo `target/{debug,release}` dir is a dev build; the
/// updater must never overwrite it.
pub fn is_cargo_target_build(exe: &Path) -> bool {
    let parts: Vec<String> = exe
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.iter().enumerate().any(|(i, p)| {
        p == "target"
            && parts[i + 1..]
                .iter()
                .take(2)
                .any(|n| n == "debug" || n == "release")
    })
}

/// The command to run when a package manager owns the binary, or `None`
/// when anyr installed it and may replace it.
pub fn package_manager_hint(exe: &Path) -> Option<String> {
    let p = exe.to_string_lossy().replace('\\', "/");
    if p.contains("/Cellar/") || p.contains("/homebrew/") || p.contains("linuxbrew/") {
        return Some("brew upgrade anyr".into());
    }
    // npm publishes every build under the `next` dist-tag only.
    if p.contains("/node_modules/") || p.contains("/.anyrouter/binaries/") {
        return Some("npm install -g @anyr/cli@next".into());
    }
    None
}

/// `/dir/anyr` -> `/dir/.anyr.<kind>`: staging files live beside the binary
/// so the final rename never crosses a filesystem.
pub fn sibling(dest: &Path, kind: &str) -> PathBuf {
    let name = dest
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "anyr".into());
    dest.with_file_name(format!(".{name}.{kind}"))
}

/// Can we create files beside the binary? Checked before any download.
pub fn dir_writable(dest: &Path) -> bool {
    let probe = sibling(dest, "probe");
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Move `new` over `dest`, keeping the old binary at `.anyr.prev`.
/// The old build is first kept at `.anyr.old`, and `.anyr.prev` is replaced
/// only after the swap worked, so a failed swap keeps the previous rollback.
/// Unix: hard-link the old one aside, then one atomic rename; `dest` exists
/// at every instant. Windows cannot replace a running exe, only rename it, so
/// the old exe moves to `.anyr.old` first and `new` takes its name. A crash
/// between those two renames leaves no `anyr`, with the complete old build
/// at `.anyr.old`; renaming it back recovers.
pub fn swap_in(new: &Path, dest: &Path) -> Result<(), String> {
    // Sweep Windows `.trash-*` slots left by clear_slot: a file still
    // running fails to delete and stays for the next run.
    clean_trash(dest);
    let old = sibling(dest, "old");
    clear_slot(&old);
    #[cfg(windows)]
    {
        fs::rename(dest, &old)
            .map_err(|e| format!("could not move {} aside: {e}", dest.display()))?;
        if let Err(e) = fs::rename(new, dest) {
            let _ = fs::rename(&old, dest);
            return Err(format!("could not replace {}: {e}", dest.display()));
        }
    }
    #[cfg(not(windows))]
    {
        if fs::hard_link(dest, &old).is_err() {
            fs::copy(dest, &old).map_err(|e| format!("could not keep the previous build: {e}"))?;
        }
        if let Err(e) = fs::rename(new, dest) {
            let _ = fs::remove_file(&old);
            return Err(format!("could not replace {}: {e}", dest.display()));
        }
    }
    let prev = sibling(dest, "prev");
    #[cfg(windows)]
    clear_slot(&prev);
    // Best effort: the new build is in place either way, and `.anyr.old`
    // still holds the old one if this rename fails.
    let _ = fs::rename(&old, &prev);
    Ok(())
}

/// Free a slot. A Windows exe that is still running cannot be deleted, but
/// it can be renamed out of the way.
fn clear_slot(path: &Path) {
    if fs::remove_file(path).is_ok() || !path.exists() {
        return;
    }
    let _ = fs::rename(path, sibling(path, &format!("trash-{}", now_secs())));
}

/// Delete `.trash-*` files left by `clear_slot` (Windows renames a running
/// exe out of the way because it cannot delete it). One still running stays
/// until a later sweep.
pub fn clean_trash(dest: &Path) {
    let Some(name) = dest.file_name().map(|s| s.to_string_lossy().into_owned()) else {
        return;
    };
    let prefix = format!("..{name}.");
    let Ok(entries) = fs::read_dir(dest.parent().unwrap_or(Path::new("."))) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if file.starts_with(&prefix) && file.contains(".trash-") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Delete `.anyr.download.<pid>` files a killed stager left behind. Only call
/// while holding the update lock, when no live process owns one.
pub fn clear_stale_downloads(dest: &Path) {
    let Some(name) = dest.file_name().map(|s| s.to_string_lossy().into_owned()) else {
        return;
    };
    let prefix = format!(".{name}.download");
    let Ok(entries) = fs::read_dir(dest.parent().unwrap_or(Path::new("."))) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Delete a staged build only if it is this install's own `.anyr.pending`.
/// The state file lives in a shared `~/.anyrouter`, so a pending path naming
/// another install's staging file must never be deleted by this one.
pub fn remove_own_pending(dest: &Path, pending_path: &str) {
    let candidate = PathBuf::from(pending_path);
    if candidate == sibling(dest, "pending") {
        let _ = fs::remove_file(&candidate);
    }
}

/// Put `.anyr.prev` back. The build being replaced becomes the new prev, so
/// a second rollback undoes the first.
pub fn rollback(dest: &Path) -> Result<(), String> {
    let prev = sibling(dest, "prev");
    if !prev.is_file() {
        return Err(format!(
            "no previous version beside {} to roll back to",
            dest.display()
        ));
    }
    let staged = sibling(dest, "rollback");
    clear_slot(&staged);
    fs::rename(&prev, &staged).map_err(|e| format!("could not stage the rollback: {e}"))?;
    swap_in(&staged, dest).inspect_err(|_| {
        let _ = fs::rename(&staged, &prev);
    })
}

/// Swap in a verified pending build. Returns the version now installed.
/// The staged file is re-hashed, and a build not newer than this one, from
/// another channel, or a prerelease on stable is discarded, so a stale or
/// tampered pending file can never land. A failed swap discards the build
/// too, so the next command does not re-hash and retry it.
pub fn apply_pending(
    dest: &Path,
    state: &mut UpdateState,
    channel: Channel,
) -> Result<Option<String>, String> {
    let (Some(path), Some(version), Some(sha)) = (
        state.pending_path.clone(),
        state.pending_version.clone(),
        state.pending_sha256.clone(),
    ) else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    // Only ever delete this install's own staging file: the state file is
    // shared across installs, and a foreign path must be left on disk.
    let own_pending = sibling(dest, "pending");
    let discard = |state: &mut UpdateState, path: &Path| {
        if *path == own_pending {
            let _ = fs::remove_file(path);
        }
        state.clear_pending();
    };
    if path != own_pending || !path.is_file() {
        discard(state, &path);
        return Ok(None);
    }
    if !crate::upgrade::needs_upgrade(VERSION, &version) {
        discard(state, &path);
        return Ok(None);
    }
    let prerelease =
        crate::channel::parse_version(&version).is_some_and(|v| v.prerelease.is_some());
    if state.channel.as_deref() != Some(channel.as_str())
        || (prerelease && channel == Channel::Stable)
    {
        discard(state, &path);
        return Ok(None);
    }
    let actual = crate::upgrade::sha256_hex_file(&path)?;
    if !actual.eq_ignore_ascii_case(sha.trim()) {
        discard(state, &path);
        return Err(format!(
            "pending {version} changed on disk since it was verified; discarded"
        ));
    }
    if let Err(err) = swap_in(&path, dest) {
        discard(state, &path);
        state.hint = Some(format!("sudo {} update", crate::help::invoked_bin()));
        return Err(err);
    }
    state.clear_pending();
    state.last_error = None;
    Ok(Some(version))
}

/// One quiet line on stderr, only for a person at a terminal.
#[cfg(feature = "native")]
fn note(line: &str) {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        eprintln!("{}", crate::term::dim(line));
    }
}

/// Runs before every command: apply a pending build, print a package-manager
/// hint once, and maybe start a detached check. Never touches the network.
#[cfg(feature = "native")]
pub fn on_startup(command: &str, parsed: &ParsedArgs, env: &BTreeMap<String, String>) {
    crate::telemetry::on_startup(env);
    if skip_reason(command, parsed, env, RunContext::detect()).is_some() {
        return;
    }
    let mut state = UpdateState::load(env);
    let before = state.clone();
    if state.pending_path.is_some() {
        if let Ok(_lock) = crate::upgrade::try_lock(env) {
            // Reload under the lock: the checker may have just written it.
            state = UpdateState::load(env);
            let channel = crate::upgrade::configured_channel(env);
            match crate::upgrade::install_dest()
                .and_then(|d| apply_pending(&d, &mut state, channel))
            {
                Ok(Some(to)) => note(&format!("anyr updated {VERSION} → {to}")),
                Ok(None) => {}
                Err(err) => state.last_error = Some(err),
            }
            if state != before {
                state.save(env);
            }
        }
    }
    if let (Some(hint), Some(latest)) = (state.hint.clone(), state.latest_seen.clone()) {
        if crate::upgrade::needs_upgrade(VERSION, &latest)
            && state.notified.as_deref() != Some(&latest)
        {
            notify_once(env, &mut state, &hint, &latest);
        }
    }
    maybe_spawn_detached_check(env);
}

/// Spawn the detached checker when the last recorded check is older than the
/// interval. Reloads the state first: the checker saves `last_check` before it
/// touches the network, so a fresh read keeps two contexts from firing twice.
#[cfg(feature = "native")]
pub(crate) fn maybe_spawn_detached_check(env: &BTreeMap<String, String>) {
    if !checked_recently(&UpdateState::load(env), env) {
        spawn_detached_check(env);
    }
}

/// One tick step between interval probes for a long-lived parent (the menu or
/// the agent parent that blocks in `spawn_child`). Clamped so the probe never
/// tight-loops, whatever interval override is set.
#[cfg(feature = "native")]
fn ticker_step_secs(env: &BTreeMap<String, String>) -> u64 {
    update_interval_secs(env).clamp(5, 60)
}

/// A thread for the two long-lived contexts that re-fires the detached
/// `update --auto` child once each interval elapses, for the whole session.
/// Only ever stages (never swaps), so the running launcher or agent is never
/// disturbed; the staged build applies on the next start. The thread lives
/// until the parent process exits. Callers who already checked the opt-outs
/// (via `on_startup`) just keep the handle for their lifetime.
#[cfg(feature = "native")]
pub(crate) fn spawn_periodic_check(env: BTreeMap<String, String>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let step = std::time::Duration::from_secs(ticker_step_secs(&env));
        loop {
            std::thread::sleep(step);
            maybe_spawn_detached_check(&env);
        }
    })
}

/// Print the package-manager hint once per version. The stager may have
/// written `pending_*` since this run loaded the state, so reload under the
/// lock before saving `notified`; without the lock, skip and print next run.
#[cfg(feature = "native")]
fn notify_once(env: &BTreeMap<String, String>, state: &mut UpdateState, hint: &str, latest: &str) {
    let Ok(_lock) = crate::upgrade::try_lock(env) else {
        return;
    };
    *state = UpdateState::load(env);
    if state.notified.as_deref() == Some(latest) {
        return;
    }
    note(&format!("anyr {latest} is available: run `{hint}`"));
    state.notified = Some(latest.to_string());
    state.save(env);
}

/// Fire-and-forget `anyr update --auto`. Its own process group, no stdio, so
/// the caller exits at normal speed and a closed terminal does not kill it.
#[cfg(feature = "native")]
fn spawn_detached_check(env: &BTreeMap<String, String>) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("update")
        .arg("--auto")
        .env(ENV_AUTO_CHILD, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for (k, v) in env {
        if k.starts_with("ANYR") || k == "ANYROUTER_HOME" || k == "HOME" {
            cmd.env(k, v);
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
    #[cfg(windows)]
    let _guard = NoStdInherit::new();
    if let Ok(child) = cmd.spawn() {
        reap_in_background(child);
    }
}

/// Wait on the detached checker from a throwaway thread so a long-lived
/// parent (the launcher menu) does not keep a zombie. A short-lived parent
/// exits first and init adopts the child.
#[cfg(feature = "native")]
fn reap_in_background(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Windows hands every inheritable handle to a child, including the pipes
/// our caller reads our stdout/stderr from. The detached checker would then
/// hold those pipes open and the caller would wait for it to finish. Clear
/// the inherit flag on our std handles while spawning it, then restore it.
#[cfg(all(feature = "native", windows))]
struct NoStdInherit(Vec<std::os::windows::io::RawHandle>);

#[cfg(all(feature = "native", windows))]
const HANDLE_FLAG_INHERIT: u32 = 1;

#[cfg(all(feature = "native", windows))]
#[link(name = "kernel32")]
extern "system" {
    fn SetHandleInformation(handle: std::os::windows::io::RawHandle, mask: u32, flags: u32) -> i32;
    fn GetHandleInformation(handle: std::os::windows::io::RawHandle, flags: *mut u32) -> i32;
}

#[cfg(all(feature = "native", windows))]
impl NoStdInherit {
    fn new() -> Self {
        use std::os::windows::io::AsRawHandle;
        let handles = [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
        ];
        let cleared = handles
            .into_iter()
            .filter(|h| !h.is_null())
            // SAFETY: std handles of this process; only the inherit flag is
            // read and cleared, and only where it was set.
            .filter(|h| {
                let mut flags = 0u32;
                unsafe {
                    GetHandleInformation(*h, &mut flags) != 0
                        && flags & HANDLE_FLAG_INHERIT != 0
                        && SetHandleInformation(*h, HANDLE_FLAG_INHERIT, 0) != 0
                }
            })
            .collect();
        Self(cleared)
    }
}

#[cfg(all(feature = "native", windows))]
impl Drop for NoStdInherit {
    fn drop(&mut self) {
        for h in &self.0 {
            // SAFETY: restores the flag cleared in `new` on the same handle.
            unsafe { SetHandleInformation(*h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::FlagValue;
    use std::collections::HashMap;

    fn tmp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "anyr-au-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn home_env(dir: &Path) -> BTreeMap<String, String> {
        BTreeMap::from([(
            "ANYROUTER_HOME".to_string(),
            dir.to_string_lossy().into_owned(),
        )])
    }

    fn parsed(command: &str, flags: &[&str]) -> ParsedArgs {
        ParsedArgs {
            command: command.into(),
            flags: flags
                .iter()
                .map(|f| (f.to_string(), FlagValue::Bool(true)))
                .collect::<HashMap<_, _>>(),
            passthrough: Vec::new(),
            raw_from: None,
        }
    }

    const TTY: RunContext = RunContext {
        tty: true,
        dev_build: false,
    };

    #[test]
    fn state_round_trips_through_json() {
        let state = UpdateState {
            last_check: 42,
            latest_seen: Some("0.1.31".into()),
            pending_path: Some("/x/.anyr.pending".into()),
            pending_version: Some("0.1.31".into()),
            pending_sha256: Some("ab".into()),
            channel: Some("beta".into()),
            last_error: Some("boom".into()),
            skip_version: Some("0.1.30".into()),
            hint: Some("brew upgrade anyr".into()),
            notified: Some("0.1.31".into()),
        };
        assert_eq!(UpdateState::from_json(&state.to_json().to_string()), state);
        assert_eq!(UpdateState::from_json("not json"), UpdateState::default());
    }

    #[test]
    fn ticker_step_is_clamped_between_five_and_sixty_seconds() {
        // WHY: a 24h interval must not make the long-lived probe sleep a day,
        // and a tiny override must not rebuild the state on every second.
        let env = BTreeMap::<String, String>::new();
        assert_eq!(ticker_step_secs(&env), 60, "default a day -> 60s probes");
        let env = BTreeMap::from([("ANYR_UPDATE_INTERVAL_SECS".into(), "10".into())]);
        assert_eq!(ticker_step_secs(&env), 10);
        let env = BTreeMap::from([("ANYR_UPDATE_INTERVAL_SECS".into(), "1".into())]);
        assert_eq!(ticker_step_secs(&env), 5);
    }

    #[test]
    fn interval_gates_the_background_check() {
        let mut s = UpdateState::default();
        assert!(!s.checked_within(86_400, 1_000_000), "never checked");
        s.last_check = 1_000_000 - 100;
        assert!(s.checked_within(86_400, 1_000_000));
        assert!(!s.checked_within(0, 1_000_000), "interval 0 always checks");
        s.last_check = 1_000_000 - 90_000;
        assert!(!s.checked_within(86_400, 1_000_000));
    }

    #[test]
    fn every_opt_out_skips_auto_update() {
        // WHY: an update or its notice must never land in CI logs, in JSON an
        // agent parses, or inside an agent session anyr launched.
        let dir = tmp_dir("gate");
        let base = home_env(&dir);
        let run = parsed("models", &[]);
        assert_eq!(skip_reason("models", &run, &base, TTY), None);
        for (k, v) in [
            ("CI", "true"),
            ("CI", "1"),
            ("ANYR_NO_AUTO_UPDATE", "1"),
            ("ANYR_NO_UPDATE", "1"),
            ("ANYR_AUTO_UPDATE", "0"),
            (ENV_LAUNCHER_CHILD, "1"),
            (ENV_AUTO_CHILD, "1"),
            ("ANYR_RELAY_DETACHED", "1"),
            ("ANYR_OUTPUT", "json"),
            ("ANYR_QUIET", "1"),
        ] {
            let mut env = base.clone();
            env.insert(k.into(), v.into());
            assert!(
                skip_reason("models", &run, &env, TTY).is_some(),
                "{k}={v} must skip"
            );
        }
        let json = parsed("models", &["json"]);
        assert_eq!(skip_reason("models", &json, &base, TTY), Some("agent mode"));
        let no_tty = RunContext {
            tty: false,
            dev_build: false,
        };
        assert_eq!(
            skip_reason("models", &run, &base, no_tty),
            Some("not a tty")
        );
        let mut allowed = base.clone();
        allowed.insert("ANYR_AUTO_UPDATE".into(), "1".into());
        assert_eq!(skip_reason("models", &run, &allowed, no_tty), None);
        let dev = RunContext {
            tty: true,
            dev_build: true,
        };
        assert_eq!(skip_reason("models", &run, &base, dev), Some("dev build"));
        assert!(skip_reason("update", &run, &base, TTY).is_some());
        let mut ci_false = base.clone();
        ci_false.insert("CI".into(), "false".into());
        assert_eq!(skip_reason("models", &run, &ci_false, TTY), None);
    }

    #[test]
    fn nested_config_update_auto_false_disables() {
        let dir = tmp_dir("cfg");
        fs::write(
            dir.join("config.yaml"),
            "active_profile: default\nupdate:\n  auto: false\nprofiles:\n  default:\n    api_key: x\n",
        )
        .unwrap();
        let env = home_env(&dir);
        assert!(!auto_update_enabled(&env));
        assert_eq!(
            skip_reason("models", &parsed("models", &[]), &env, TTY),
            Some("disabled")
        );
    }

    #[test]
    fn package_managers_get_a_hint_not_a_swap() {
        let hint = |p: &str| package_manager_hint(Path::new(p));
        assert_eq!(
            hint("/opt/homebrew/Cellar/anyr/0.1.30/bin/anyr").as_deref(),
            Some("brew upgrade anyr")
        );
        assert_eq!(
            hint("/home/u/.linuxbrew/bin/anyr").as_deref(),
            Some("brew upgrade anyr")
        );
        assert_eq!(
            hint("/usr/lib/node_modules/@anyr/cli/binaries/anyr").as_deref(),
            Some("npm install -g @anyr/cli@next")
        );
        assert_eq!(
            hint("/home/u/.anyrouter/binaries/0.1.30/anyr").as_deref(),
            Some("npm install -g @anyr/cli@next")
        );
        assert_eq!(hint("/home/u/.local/bin/anyr"), None);
    }

    #[test]
    fn cargo_target_builds_are_never_self_updated() {
        // WHY: auto-update replaced a dev build in target/ with a release.
        assert!(is_cargo_target_build(Path::new("/w/cli/target/debug/anyr")));
        assert!(is_cargo_target_build(Path::new(
            "/w/cli/target/x86_64-unknown-linux-gnu/release/anyr"
        )));
        assert!(!is_cargo_target_build(Path::new("/home/u/.local/bin/anyr")));
        assert!(!is_cargo_target_build(Path::new("/opt/target/bin/anyr")));
    }

    #[test]
    fn failed_swap_keeps_the_previous_rollback_build() {
        // WHY: `.prev` was cleared before the swap, so a swap that failed
        // left nothing to roll back to.
        let dir = tmp_dir("swap-fail");
        let dest = dir.join("anyr");
        fs::write(&dest, "current").unwrap();
        fs::write(sibling(&dest, "prev"), "older").unwrap();
        let missing = sibling(&dest, "pending");
        assert!(swap_in(&missing, &dest).is_err());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "current");
        assert_eq!(fs::read_to_string(sibling(&dest, "prev")).unwrap(), "older");
        assert!(!sibling(&dest, "old").exists());
    }

    #[test]
    fn swap_keeps_previous_and_rollback_toggles() {
        let dir = tmp_dir("swap");
        let dest = dir.join("anyr");
        fs::write(&dest, "old").unwrap();
        let new = sibling(&dest, "pending");
        fs::write(&new, "new").unwrap();
        swap_in(&new, &dest).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "new");
        assert_eq!(fs::read_to_string(sibling(&dest, "prev")).unwrap(), "old");
        assert!(!new.exists());

        rollback(&dest).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "old");
        assert_eq!(fs::read_to_string(sibling(&dest, "prev")).unwrap(), "new");
        rollback(&dest).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "new");
    }

    #[test]
    fn rollback_without_previous_fails_and_keeps_binary() {
        let dir = tmp_dir("norb");
        let dest = dir.join("anyr");
        fs::write(&dest, "only").unwrap();
        assert!(rollback(&dest).unwrap_err().contains("no previous version"));
        assert_eq!(fs::read_to_string(&dest).unwrap(), "only");
    }

    fn pending_state(dest: &Path, body: &str, version: &str) -> UpdateState {
        let pending = sibling(dest, "pending");
        fs::write(&pending, body).unwrap();
        UpdateState {
            pending_path: Some(pending.to_string_lossy().into_owned()),
            pending_version: Some(version.into()),
            pending_sha256: Some(crate::upgrade::sha256_hex_file(&pending).unwrap()),
            channel: Some("stable".into()),
            ..UpdateState::default()
        }
    }

    #[test]
    fn apply_pending_swaps_verified_newer_build() {
        let dir = tmp_dir("apply");
        let dest = dir.join("anyr");
        fs::write(&dest, "current").unwrap();
        let mut state = pending_state(&dest, "next", "999.0.0");
        assert_eq!(
            apply_pending(&dest, &mut state, Channel::Stable)
                .unwrap()
                .as_deref(),
            Some("999.0.0")
        );
        assert_eq!(fs::read_to_string(&dest).unwrap(), "next");
        assert_eq!(state.pending_path, None);
    }

    #[test]
    fn apply_pending_rejects_tampered_or_older_builds() {
        // WHY: the pending file sits in a user-writable dir for up to a day;
        // it must match the hash taken when it was verified.
        let dir = tmp_dir("tamper");
        let dest = dir.join("anyr");
        fs::write(&dest, "current").unwrap();
        let mut state = pending_state(&dest, "next", "999.0.0");
        fs::write(sibling(&dest, "pending"), "evil").unwrap();
        assert!(apply_pending(&dest, &mut state, Channel::Stable).is_err());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "current");
        assert!(!sibling(&dest, "pending").exists());

        let mut older = pending_state(&dest, "old", "0.0.1");
        assert_eq!(
            apply_pending(&dest, &mut older, Channel::Stable).unwrap(),
            None
        );
        assert_eq!(fs::read_to_string(&dest).unwrap(), "current");
        assert_eq!(older.pending_path, None);
    }

    #[test]
    fn apply_pending_drops_a_build_from_another_channel() {
        // WHY: a beta staged before `update --stable` must never install
        // afterwards, even if the pending record survived the switch.
        let dir = tmp_dir("chan");
        let dest = dir.join("anyr");
        fs::write(&dest, "current").unwrap();

        let mut beta = pending_state(&dest, "next", "999.0.0-beta.1");
        beta.channel = Some("beta".into());
        assert_eq!(
            apply_pending(&dest, &mut beta, Channel::Stable).unwrap(),
            None
        );
        assert_eq!(fs::read_to_string(&dest).unwrap(), "current");
        assert_eq!(beta.pending_path, None);
        assert!(!sibling(&dest, "pending").exists());

        let mut pre_on_stable = pending_state(&dest, "next", "999.0.0-beta.1");
        assert_eq!(
            apply_pending(&dest, &mut pre_on_stable, Channel::Stable).unwrap(),
            None,
            "a prerelease never lands on stable"
        );
        assert_eq!(fs::read_to_string(&dest).unwrap(), "current");

        let mut ok = pending_state(&dest, "next", "999.0.0-beta.1");
        ok.channel = Some("beta".into());
        assert_eq!(
            apply_pending(&dest, &mut ok, Channel::Beta)
                .unwrap()
                .as_deref(),
            Some("999.0.0-beta.1")
        );
    }

    #[test]
    fn failed_swap_discards_pending_and_points_at_sudo() {
        // WHY: a pending build that cannot be swapped in was retried and
        // re-hashed on every command. It must be dropped once.
        let dir = tmp_dir("swapfail");
        let dest = dir.join("anyr"); // never created: the swap cannot keep a prev
        let mut state = pending_state(&dest, "next", "999.0.0");
        assert!(apply_pending(&dest, &mut state, Channel::Stable).is_err());
        assert_eq!(state.pending_path, None);
        assert_eq!(state.pending_sha256, None);
        assert!(!sibling(&dest, "pending").exists());
        assert!(state.hint.as_deref().unwrap_or_default().contains("update"));
    }

    #[test]
    fn state_save_is_atomic() {
        // WHY: the stager, the applier and `update` all write this file; a
        // reader must never see it truncated or half written.
        let dir = tmp_dir("atomic");
        let env = home_env(&dir);
        let state = UpdateState {
            last_check: 1,
            last_error: Some("x".repeat(512 * 1024)),
            ..UpdateState::default()
        };
        state.save(&env);
        let path = UpdateState::path(&env);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let (path, stop) = (path.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut bad = 0;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let raw = fs::read_to_string(&path).unwrap_or_default();
                    if serde_json::from_str::<Value>(&raw).is_err() {
                        bad += 1;
                    }
                }
                bad
            })
        };
        for _ in 0..200 {
            state.save(&env);
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(reader.join().unwrap(), 0, "reader saw a partial state file");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind");
    }

    #[test]
    fn notified_save_keeps_a_concurrently_staged_build() {
        // WHY: this run loaded its state before the stager recorded a pending
        // build; saving `notified` from that copy erased the pending fields.
        let dir = tmp_dir("notify");
        let env = home_env(&dir);
        let stale = UpdateState {
            latest_seen: Some("999.0.0".into()),
            hint: Some("brew upgrade anyr".into()),
            ..UpdateState::default()
        };
        stale.save(&env);
        let mut staged = stale.clone();
        staged.pending_path = Some("/x/.anyr.pending".into());
        staged.pending_version = Some("999.0.0".into());
        staged.pending_sha256 = Some("ab".into());
        staged.save(&env);

        let mut in_memory = stale;
        notify_once(&env, &mut in_memory, "brew upgrade anyr", "999.0.0");

        let disk = UpdateState::load(&env);
        assert_eq!(disk.notified.as_deref(), Some("999.0.0"));
        assert_eq!(disk.pending_version.as_deref(), Some("999.0.0"));
        assert_eq!(disk.pending_sha256.as_deref(), Some("ab"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detached_child_is_reaped_not_left_a_zombie() {
        // WHY: the launcher menu is long-lived; an unwaited child stays <defunct>.
        let child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        reap_in_background(child);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while Path::new(&format!("/proc/{pid}")).exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "pid {pid} still exists: zombie"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn trash_and_stale_downloads_are_swept() {
        let dir = tmp_dir("trash");
        let dest = dir.join("anyr.exe");
        fs::write(&dest, "bin").unwrap();
        let prev = sibling(&dest, "prev");
        fs::write(&prev, "prev").unwrap();
        let trash = sibling(&prev, "trash-123");
        fs::write(&trash, "old").unwrap();
        let dl = sibling(&dest, "download.4242");
        fs::write(&dl, "partial").unwrap();
        clean_trash(&dest);
        clear_stale_downloads(&dest);
        assert!(!trash.exists());
        assert!(!dl.exists());
        assert!(dest.exists() && prev.exists(), "live files must stay");
    }

    #[test]
    fn swap_sweeps_trash_files_left_by_clear_slot() {
        // WHY: on Windows a still-running .prev is renamed to .trash-<secs>
        // and otherwise survives forever; every swap sweeps them (a file
        // still running fails to delete and stays for the next run).
        let dir = tmp_dir("swap-trash");
        let dest = dir.join("anyr.exe");
        fs::write(&dest, "old").unwrap();
        let prev = sibling(&dest, "prev");
        fs::write(&prev, "prev").unwrap();
        let trash = sibling(&prev, "trash-123");
        fs::write(&trash, "old").unwrap();
        let new = dir.join("new-build");
        fs::write(&new, "new").unwrap();
        swap_in(&new, &dest).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "new");
        assert!(
            !trash.exists(),
            "trash file survived the swap that should have swept it"
        );
    }

    #[test]
    fn foreign_pending_path_is_cleared_but_never_deleted() {
        // WHY: two installs share one ~/.anyrouter state file, so the pending
        // path may name the other install's staging file; discarding it must
        // clear the state without deleting that file.
        let dir = tmp_dir("foreign");
        let dest_a = dir.join("a/anyr");
        let dest_b = dir.join("b/anyr");
        fs::create_dir_all(dest_a.parent().unwrap()).unwrap();
        fs::create_dir_all(dest_b.parent().unwrap()).unwrap();
        fs::write(&dest_a, "current-a").unwrap();
        let foreign_pending = sibling(&dest_b, "pending");
        fs::write(&foreign_pending, "staged-for-b").unwrap();
        let mut state = UpdateState {
            pending_path: Some(foreign_pending.to_string_lossy().into_owned()),
            pending_version: Some("999.0.0".into()),
            pending_sha256: Some(crate::upgrade::sha256_hex_file(&foreign_pending).unwrap()),
            channel: Some("stable".into()),
            ..UpdateState::default()
        };
        let applied = apply_pending(&dest_a, &mut state, Channel::Stable).unwrap();
        assert_eq!(applied, None);
        assert_eq!(state.pending_path, None);
        assert!(
            foreign_pending.exists(),
            "applying install A must not delete install B's staged build"
        );
        assert_eq!(fs::read_to_string(&dest_a).unwrap(), "current-a");
        // remove_own_pending guards the upgrade.rs discard sites the same way.
        remove_own_pending(&dest_a, &foreign_pending.to_string_lossy());
        assert!(foreign_pending.exists());
        remove_own_pending(&dest_b, &foreign_pending.to_string_lossy());
        assert!(!foreign_pending.exists());
    }
}
