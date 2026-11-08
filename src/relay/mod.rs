//! `anyr relay` — pair this machine with an AnyRouter account and proxy cloud
//! chat-completion requests to a local OpenAI-compatible server. Migrated from
//! packages/agent/src/commands/relay.ts (TS CLI #1099); the cloud side
//! (`MacBridgeDO`) is live, so only this client was missing.
//!
//! One-key auth: `anyr relay start` resolves the WebSocket credential in this
//! order —
//!   1. explicit `--token` (legacy rk_ pairing token)
//!   2. `ANYROUTER_RELAY_TOKEN` env var (legacy)
//!   3. `relay_token` in the active profile of ~/.anyrouter/config.yaml (legacy)
//!   4. the regular sk-ar-v1 inference key (`--key` / `ANYROUTER_API_KEY` /
//!      profile). The cloud pairs IMPLICITLY on connect (`?name=` labels a new
//!      device), so no `POST /relay/devices` round trip happens and no rk_ is
//!      minted. That call is not allowed for scoped keys.
//!   5. DEVICE LOGIN when no credentials exist at all (interactive only) —
//!      RFC 8628 flow mints an sk-ar key (stored), then connect as in 4.
//!
//! `anyr relay pair` still mints and stores an rk_ under `relay_token` in the
//! shared ~/.anyrouter/config.yaml, so the TS CLI keeps working.
//!
//! Transport: one outbound WebSocket to the cloud relay (via
//! `/api/v1/relay/connect`), auto-reconnecting with exponential backoff. For
//! each `request` frame pushed down, the body is forwarded to the local
//! `--target` server and the response streams back up as `head`/`chunk`/
//! `done` frames — chunks are sent incrementally as they arrive from the
//! local server, never buffered whole. `cancel` frames abort the in-flight
//! local request between stream reads.
//!
//! Process model: the socket lives on the main thread (tungstenite is not
//! Sync, so readers and writers share one owner). Each incoming `request`
//! frame spawns a worker thread that streams the local response into an
//! outbound channel; the main loop multiplexes between draining that channel
//! and polling the socket (short read timeout → `WouldBlock`). This keeps
//! concurrent requests streaming without deadlocking the single socket.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(feature = "native")]
use serde_json::json;

mod cli;
mod daemon;
mod devices;
mod devname;
mod doctor;
mod foundation;
mod live;
mod servers;
mod state;
mod view;

use live::Stats;
use state::Conn;
use view::{Paint, DEVICES_URL, POOL_URL};

/// Profile field storing the rk_ pairing token (~/.anyrouter/config.yaml).
pub const RELAY_TOKEN_FIELD: &str = "relay_token";
pub const RELAY_TOKEN_ENV_VAR: &str = "ANYROUTER_RELAY_TOKEN";
/// Cached paired-device id so `--pool` doesn't need another lookup.
pub const RELAY_DEVICE_ID_FIELD: &str = "relay_device_id";

const DEFAULT_TARGET: &str = "http://localhost:8000/v1";
const DEFAULT_API_BASE: &str = "https://anyrouter.dev/api/v1";
const RELAY_DEVICE_NAME_FIELD: &str = "relay_device_name";
/// How long one non-blocking poll of the socket waits before we check the
/// outbound queue again. Bounds cancel latency and reconnect responsiveness.
const SOCKET_POLL_MS: u64 = 250;
/// Read wait while the outbound queue still holds frames: just long enough
/// to collect what the cloud already sent.
const BUSY_POLL: Duration = Duration::from_millis(1);
/// Frames workers may queue ahead of the socket. A peer that stops reading
/// then stalls the local read instead of growing memory: at most this many
/// chunks of up to 8 KiB each.
const OUTBOUND_QUEUE: usize = 64;

/// Liveness and reconnect timings. The cloud answers protocol pings without
/// waking its Durable Object, so pinging an idle socket is free; silence for
/// `dead_after` means the socket is half-open (sleep, NAT timeout, dead edge).
#[derive(Clone, Copy, Debug)]
struct Timing {
    ping_every: Duration,
    dead_after: Duration,
    backoff_base: Duration,
    backoff_max: Duration,
    /// A socket that stayed up this long resets the backoff; a cloud that
    /// accepts and drops at once keeps backing off.
    stable_after: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            ping_every: Duration::from_secs(10),
            dead_after: Duration::from_secs(25),
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            stable_after: Duration::from_secs(30),
        }
    }
}

impl Timing {
    /// Debug builds honour `ANYR_RELAY_TIME_SCALE` (0.001 to 1), which shrinks
    /// every timing so the hermetic tests run in seconds. Release builds
    /// always use the default timings.
    fn from_env(env: &BTreeMap<String, String>) -> Self {
        Self::from_env_with(env, cfg!(debug_assertions))
    }

    fn from_env_with(env: &BTreeMap<String, String>, allow_scale: bool) -> Self {
        let t = Timing::default();
        if !allow_scale {
            return t;
        }
        let Some(k) = env
            .get("ANYR_RELAY_TIME_SCALE")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|k| (0.001..=1.0).contains(k))
        else {
            return t;
        };
        Timing {
            ping_every: t.ping_every.mul_f64(k),
            dead_after: t.dead_after.mul_f64(k),
            backoff_base: t.backoff_base.mul_f64(k),
            backoff_max: t.backoff_max.mul_f64(k),
            stable_after: t.stable_after.mul_f64(k),
        }
    }
}

/// Exponential backoff with jitter: attempt k waits a random time in
/// [d/2, d] where d = base * 2^(k-1), capped at `backoff_max`. Jitter keeps
/// many relays from reconnecting in lockstep after a cloud deploy.
#[derive(Default)]
struct Backoff {
    attempt: u32,
}

impl Backoff {
    fn next(&mut self, t: &Timing) -> Duration {
        use std::hash::{BuildHasher as _, Hasher as _};
        self.attempt = self.attempt.saturating_add(1);
        let full = t
            .backoff_base
            .saturating_mul(1 << (self.attempt - 1).min(20))
            .min(t.backoff_max);
        let unit = (std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
            % 1_000) as f64
            / 1_000.0;
        full.mul_f64(0.5 + unit / 2.0)
    }
}

/// `fm serve`'s loopback-only OpenAI-compatible endpoint (Apple Foundation
/// Models): observed output is `url http://127.0.0.1:1976`,
/// `access loopback-only`, with GET /v1/models and GET /health.
const FM_SERVE_TARGET: &str = "http://127.0.0.1:1976/v1";
const FM_SERVE_HEALTH_URL: &str = "http://127.0.0.1:1976/health";
const OLLAMA_TARGET: &str = "http://localhost:11434/v1";

/// The join key pool routing actually matches on (#1128): the executor sends
/// the UPSTREAM model_name ("foundation-model") from the catalog entry as
/// `body.model`, not the public catalog id "apple/foundation-model". A hello
/// frame advertising only fm serve's own local model ids would never match a
/// pool lookup. Always advertise this id when talking to fm serve.
const FM_SERVE_ADVERTISED_MODEL_ID: &str = "foundation-model";

// ---------------------------------------------------------------------------
// Diagnostics — verbose-gated [relay] lines go to stderr like the TS client.
// ---------------------------------------------------------------------------

static VERBOSE: AtomicBool = AtomicBool::new(false);

fn vlog(msg: &str) {
    if VERBOSE.load(Ordering::Relaxed) {
        live::log(&format!("[relay] {msg}"));
    }
}

fn ulog(msg: &str) {
    live::log(msg);
}

// ---------------------------------------------------------------------------
// Wire protocol — mirror of packages/contracts/src/local-relay/index.ts.
// Frames are JSON text messages sharing a `{ type, id }` envelope. Keep in
// sync with that file; unknown types are ignored gracefully so old/new builds
// stay forward/backward compatible.
// ---------------------------------------------------------------------------

/// Cloud → device: execute a chat completion against the local target.
#[derive(Debug, PartialEq, Eq)]
struct RequestFrame {
    id: String,
    path: String,
    body: String,
}

/// Device → cloud frames.
#[derive(Debug)]
enum ClientFrame {
    Head {
        id: String,
        status: u16,
        content_type: String,
    },
    Chunk {
        id: String,
        data: String,
    },
    Done {
        id: String,
    },
    Error {
        id: String,
        message: String,
    },
    /// Capability advertisement (#1128), sent right after connecting (and on
    /// every reconnect): OpenAI-compatible model ids this device can serve.
    /// Pool routing joins requests against these ids.
    Hello {
        models: Vec<String>,
        max_concurrency: Option<u32>,
        /// Pool sharing the user chose (flag, saved pref or the `p` key), set
        /// in the same write as the models so `--no-pool` holds before any
        /// request can arrive. None for the unchosen default: a reconnect
        /// must not undo a change made on the dashboard.
        pool: Option<bool>,
    },
}

impl ClientFrame {
    fn to_json(&self) -> String {
        match self {
            Self::Head {
                id,
                status,
                content_type,
            } => json!({
                "type": "head", "id": id, "status": status, "contentType": content_type,
            })
            .to_string(),
            Self::Chunk { id, data } => {
                json!({ "type": "chunk", "id": id, "data": data }).to_string()
            }
            Self::Done { id } => json!({ "type": "done", "id": id }).to_string(),
            Self::Error { id, message } => {
                json!({ "type": "error", "id": id, "message": message }).to_string()
            }
            Self::Hello {
                models,
                max_concurrency,
                pool,
            } => {
                let mut v = json!({ "type": "hello", "id": "", "models": models });
                if let Some(on) = pool {
                    v["pool"] = json!(on);
                }
                if let Some(n) = max_concurrency {
                    v["maxConcurrency"] = json!(n);
                }
                v.to_string()
            }
        }
    }
}

/// Cloud → device frames the relay acts on.
#[derive(Debug, PartialEq, Eq)]
enum ServerFrame {
    Request(RequestFrame),
    Cancel(String),
    /// Sent once right after connect: the device row this connection is bound
    /// to. Implicit (sk-ar) pairing never told the client its id, so pool
    /// changes had to guess between several devices.
    Device {
        id: String,
        name: String,
    },
}

/// Parse a raw WS text message into a server frame, mirroring
/// parseRelayServerFrame() in packages/contracts/src/local-relay/index.ts:
/// malformed JSON, missing `{type,id}`, or unknown types yield None (ignored
/// rather than fatal).
fn parse_server_frame(raw: &str) -> Option<ServerFrame> {
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    let obj = parsed.as_object()?;
    let ty = obj.get("type")?.as_str()?;
    let id = obj.get("id")?.as_str()?;
    let text = |key: &str| {
        obj.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    match ty {
        "request" => Some(ServerFrame::Request(RequestFrame {
            id: id.to_string(),
            path: text("path"),
            body: text("body"),
        })),
        "cancel" => Some(ServerFrame::Cancel(id.to_string())),
        "device" if !id.trim().is_empty() => Some(ServerFrame::Device {
            id: id.to_string(),
            name: text("name"),
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

struct StartArgs {
    /// None = not explicitly passed; run_relay_start auto-detects then.
    target: Option<String>,
    token: Option<String>,
    url: Option<String>,
    /// `--name` as passed. `run_relay_start` resolves it into `device`.
    name: Option<String>,
    /// The resolved device name; empty until `resolve_device`.
    device: String,
    /// Comma-joined `--model` values (the parser joins repeats for relay).
    models: Option<String>,
    all: bool,
    max_concurrency: Option<u32>,
    /// `--no-launch`: use only servers that are already running.
    no_launch: bool,
}

fn parse_start_args(parsed: &crate::parse::ParsedArgs) -> StartArgs {
    let get = |name: &str| crate::parse::get_string_flag(&parsed.flags, name);
    StartArgs {
        target: get("target"),
        token: get("token"),
        url: get("url"),
        name: get("name").filter(|s| !s.trim().is_empty()),
        device: String::new(),
        models: get("model"),
        all: parsed.flag_true("all"),
        no_launch: parsed.flag_true("no-launch"),
        max_concurrency: get("max-concurrency")
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|n| *n > 0),
    }
}

// ---------------------------------------------------------------------------
// Pairing + credential resolution
// ---------------------------------------------------------------------------

/// Mint an rk_ pairing token via POST /relay/devices with the user's sk-ar-v1
/// inference key (the route accepts inference keys) and persist it to the
/// shared config. Shared by explicit `relay pair` and auto-pair in start.
fn pair_device(
    api_key: &str,
    name: &str,
    env: &BTreeMap<String, String>,
) -> Result<String, String> {
    let url = format!("{DEFAULT_API_BASE}/relay/devices");
    let body = json!({ "name": name }).to_string();
    let (status, resp) = crate::http::http_post(&url, Some(api_key), Some(&body))?;
    let value: serde_json::Value = serde_json::from_str(&resp).unwrap_or(serde_json::Value::Null);
    let token = value.get("token").and_then(|v| v.as_str()).unwrap_or("");
    if !(200..300).contains(&status) || token.is_empty() {
        let msg = value
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("unexpected response");
        return Err(format!("Pairing failed ({status}): {msg}"));
    }
    write_profile_field(RELAY_TOKEN_FIELD, token, env)?;
    // Cached so --pool can PATCH /relay/devices/:id without another lookup.
    if let Some(id) = value
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        write_profile_field(RELAY_DEVICE_ID_FIELD, id, env)?;
    }
    Ok(token.to_string())
}

/// Resolve the credential chain documented in the module comment. Returns the
/// bearer for the connect handshake: an rk_ token or an sk-ar key. Never calls
/// the cloud unless it has to run device login.
fn resolve_relay_credential(
    explicit: Option<&str>,
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<String, String> {
    if let Some(t) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(t.to_string());
    }
    if let Some(t) = env
        .get(RELAY_TOKEN_ENV_VAR)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return Ok(t.to_string());
    }
    if let Some(t) = read_stored_relay_token(env)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return Ok(t);
    }
    if let Some(key) = resolve_sk_ar_key(parsed, env) {
        return Ok(key);
    }
    // Interactive convenience path only: a headless invocation (cron, CI,
    // piped output) must not sit polling a device code.
    if !crate::term::is_interactive() {
        let bin = crate::help::invoked_bin();
        return Err(view::error(
            "no AnyRouter key or relay token",
            &format!("not at a terminal, so `{bin} relay start` cannot open a browser to log in"),
            &format!("`{bin} login`, or set ANYROUTER_API_KEY / {RELAY_TOKEN_ENV_VAR}"),
        ));
    }
    ulog("No AnyRouter credentials found — starting device login.");
    let open_browser = crate::auth::browser_likely_available(env);
    let token = crate::auth::login_via_device(&device_login_base(), Some("cli"), open_browser)?;
    persist_api_key(&token.api_key, env)?;
    ulog("Logged in.");
    Ok(token.api_key)
}

fn read_stored_relay_token(env: &BTreeMap<String, String>) -> Option<String> {
    let path = crate::config::resolve_config_path(None, env);
    crate::key::load_config_if_present(&path)?
        .profiles
        .get(&load_active_profile_name(env))?
        .relay_token
        .clone()
}

fn load_active_profile_name(env: &BTreeMap<String, String>) -> String {
    let path = crate::config::resolve_config_path(None, env);
    crate::key::load_config_if_present(&path)
        .map(|c| c.active_profile)
        .unwrap_or_else(|| crate::config::DEFAULT_PROFILE.into())
}

/// `…/api/v1` → `…/api` — the parent base the existing device-flow helpers
/// expect (they append /v1/auth/... themselves).
fn device_login_base() -> String {
    DEFAULT_API_BASE
        .strip_suffix("/v1")
        .unwrap_or(DEFAULT_API_BASE)
        .to_string()
}

/// Write one relay-related field (or the api_key after device login) into the
/// active profile of the shared config.
fn write_profile_field(
    field: &str,
    value: &str,
    env: &BTreeMap<String, String>,
) -> Result<(), String> {
    let path = crate::config::resolve_config_path(None, env);
    let mut cfg = crate::key::load_config_if_present(&path).unwrap_or_default();
    let active = cfg.active_profile.clone();
    let entry = cfg.profiles.entry(active).or_default();
    match field {
        RELAY_TOKEN_FIELD => entry.relay_token = Some(value.to_string()),
        RELAY_DEVICE_ID_FIELD => entry.relay_device_id = Some(value.to_string()),
        RELAY_DEVICE_NAME_FIELD => entry.relay_device_name = Some(value.to_string()),
        "api_key" => entry.api_key = Some(value.to_string()),
        other => return Err(format!("unknown relay profile field {other}")),
    }
    crate::config::write_config(&cfg, &path)
}

fn persist_api_key(key: &str, env: &BTreeMap<String, String>) -> Result<(), String> {
    write_profile_field("api_key", key, env)
}

// ---------------------------------------------------------------------------
// Target detection
// ---------------------------------------------------------------------------

/// What a local endpoint speaks: OpenAI-style chat completions, the
/// Perplexity Decisions shape (`POST /v1/decisions` only), or the Apple system
/// model in-process. The in-process endpoint answers chat completions with no
/// HTTP server behind it (`foundation::foundation_model_available`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Chat,
    Decider,
    /// Apple system model, served in-process by `foundation`.
    Foundation,
}

impl TargetKind {
    fn name(self) -> &'static str {
        match self {
            TargetKind::Chat => "chat",
            TargetKind::Decider => "decider",
            TargetKind::Foundation => "foundation",
        }
    }

    /// Chat-shaped (`/chat/completions`) endpoints: the plain HTTP servers and
    /// the in-process Apple system model. Decisions never go here.
    fn is_chat_shape(self) -> bool {
        matches!(self, TargetKind::Chat | TargetKind::Foundation)
    }
}

/// One discovered (or `--target`) local endpoint and the models it serves.
#[derive(Debug, Clone)]
struct Target {
    label: String,
    base: String,
    kind: TargetKind,
    models: Vec<String>,
    /// fm serve only: its real local ids from /v1/models (e.g. `system`,
    /// `pcc`), used to map the catalog name the cloud sends.
    fm_models: Option<Vec<String>>,
}

/// Model id -> owning target. The first target listed wins a duplicate id.
#[derive(Debug, Clone, Default)]
struct Router {
    targets: Vec<Target>,
    by_model: BTreeMap<String, usize>,
}

impl Router {
    fn new(targets: Vec<Target>) -> Self {
        let mut by_model: BTreeMap<String, usize> = BTreeMap::new();
        for (i, t) in targets.iter().enumerate() {
            for m in &t.models {
                match by_model.get(m) {
                    Some(&first) => ulog(&format!(
                        "model \"{m}\" is served by both {} and {}; using {}",
                        targets[first].label, t.label, targets[first].label
                    )),
                    None => {
                        by_model.insert(m.clone(), i);
                    }
                }
            }
        }
        Router { targets, by_model }
    }

    /// Union of every target's model ids, for the hello frame.
    fn all_models(&self) -> Vec<String> {
        self.by_model.keys().cloned().collect()
    }

    /// The only target of `kind`: it takes every request of its kind, listed
    /// model or not. The cloud sends the catalog's upstream `model_name`,
    /// which need not match a local id. Model matching only applies when two
    /// or more targets of a kind exist.
    fn lone_of_kind(&self, kind: TargetKind) -> Option<&Target> {
        let mut of_kind = self.targets.iter().filter(|t| t.kind == kind);
        match (of_kind.next(), of_kind.next()) {
            (Some(t), None) => Some(t),
            _ => None,
        }
    }

    /// The only chat-shaped target (plain HTTP chat server or the in-process
    /// Apple system model): it takes every chat request, listed model or not.
    fn lone_chat_capable(&self) -> Option<&Target> {
        let mut of_chat = self.targets.iter().filter(|t| t.kind.is_chat_shape());
        match (of_chat.next(), of_chat.next()) {
            (Some(t), None) => Some(t),
            _ => None,
        }
    }

    /// Pick the target for a request by its body `model`. Decisions go only
    /// to a Decider, everything else only to a chat-shaped target.
    fn route(&self, path: &str, body: &str) -> Result<&Target, String> {
        let want_decider = path.trim_end_matches('/').ends_with("/decisions");
        if want_decider {
            if let Some(t) = self.lone_of_kind(TargetKind::Decider) {
                return Ok(t);
            }
        } else if let Some(t) = self.lone_chat_capable() {
            return Ok(t);
        }
        let model = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(str::to_string));
        let Some(model) = model else {
            return Err("Request body has no \"model\" field.".to_string());
        };
        match self.by_model.get(&model) {
            Some(&i) => {
                let t = &self.targets[i];
                if t.kind.is_chat_shape() != want_decider {
                    Ok(t)
                } else {
                    Err(format!(
                        "Model \"{model}\" is served by a {} endpoint ({}) and cannot handle {path}.",
                        t.kind.name(),
                        t.label
                    ))
                }
            }
            None => {
                let known = self.all_models();
                Err(format!(
                    "Model \"{model}\" is not served by any local endpoint on this device (available: {}).",
                    if known.is_empty() { "none".into() } else { known.join(", ") }
                ))
            }
        }
    }
}

/// Perplexity Decider for Mac: a local `POST /v1/decisions`-only server.
const DECIDER_TARGET: &str = "http://127.0.0.1:8080/v1";
const DECIDER_ADVERTISED_MODEL_ID: &str = "pplx-decider-flash-v1.2";
const LMSTUDIO_TARGET: &str = "http://localhost:1234/v1";

/// Other chat endpoints probed during discovery, after the on-device
/// servers in `servers::SERVERS`: (label, base, liveness URL).
/// Any HTTP response counts as listening, not just 2xx.
const CHAT_PROBES: [(&str, &str, &str); 3] = [
    (
        "default target",
        DEFAULT_TARGET,
        "http://localhost:8000/v1/models",
    ),
    ("Ollama", OLLAMA_TARGET, "http://localhost:11434/v1/models"),
    (
        "LM Studio",
        LMSTUDIO_TARGET,
        "http://localhost:1234/v1/models",
    ),
];

/// True when `body` is a Decisions response: a JSON object with an `answers`
/// object. Port 8080 is a common dev port, so a listener alone proves nothing.
fn is_decider_response(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .is_some_and(|v| v.get("answers").is_some_and(|a| a.is_object()))
}

/// Bases already confirmed as a Decider; a positive result holds for the
/// life of the process so reconnects do not re-run the POST.
static CONFIRMED_DECIDERS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Send one tiny real decision to `base`/decisions and check the reply shape.
fn probe_decider(base: &str) -> bool {
    if CONFIRMED_DECIDERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(base)
    {
        return true;
    }
    let url = format!("{}/decisions", base.trim_end_matches('/'));
    let body = json!({
        "model": DECIDER_ADVERTISED_MODEL_ID,
        "state": "probe",
        "questions": {"q": {"type": "choice", "instructions": "Pick a.",
            "criteria": {"a": null, "b": null}}}
    })
    .to_string();
    match ureq::post(&url)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(5))
        .send_string(&body)
    {
        Ok(resp) if (200..300).contains(&resp.status()) => {
            let ok = is_decider_response(&resp.into_string().unwrap_or_default());
            if ok {
                CONFIRMED_DECIDERS
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(base.to_string());
            }
            ok
        }
        Err(ureq::Error::Transport(t)) if t.to_string().to_lowercase().contains("timed out") => {
            ulog(&format!(
                "decider probe of {base} timed out after 5s; treating it as not a Decider (will retry on reconnect)"
            ));
            false
        }
        _ => false,
    }
}

fn chat_target(label: &str, base: &str) -> Target {
    let fetched = fetch_local_models(base);
    Target {
        label: label.to_string(),
        base: base.to_string(),
        kind: TargetKind::Chat,
        models: advertised_models(base, &fetched),
        fm_models: is_fm_serve_target(base).then_some(fetched),
    }
}

fn decider_target(base: &str) -> Target {
    Target {
        label: "Perplexity Decider".to_string(),
        base: base.to_string(),
        kind: TargetKind::Decider,
        models: vec![DECIDER_ADVERTISED_MODEL_ID.to_string()],
        fm_models: None,
    }
}

/// The Apple system model hosted in-process by `foundation`. No HTTP server
/// is behind it; `base` is a label for logs and the status screen. Requests
/// the router sends here take the `handle_foundation_request` path.
fn foundation_target() -> Target {
    Target {
        label: "Apple Foundation Models".to_string(),
        base: FOUNDATION_BASE.to_string(),
        kind: TargetKind::Foundation,
        models: vec![FM_SERVE_ADVERTISED_MODEL_ID.to_string()],
        fm_models: None,
    }
}

/// Base label of the in-process Apple system model endpoint. Not a URL: the
/// driver answers directly, so nothing is ever opened or dialed on it.
const FOUNDATION_BASE: &str = "foundation://in-process";

/// Whether an on-device server answers the way discovery requires: any
/// response for a chat server, a real decision for a Decider (8080 is a
/// common dev port), or an in-process Apple system model.
fn on_device_listening(spec: &servers::ServerSpec) -> bool {
    match spec.kind {
        TargetKind::Chat => http_probe_listening(spec.health.url()),
        TargetKind::Decider => probe_decider(spec.base),
        TargetKind::Foundation => foundation::foundation_model_available(),
    }
}

/// Probe every known local endpoint (all of them, not first-wins).
fn discover_targets() -> Vec<Target> {
    let mut out = Vec::new();
    // The in-process Apple system model comes first: when it is available it
    // owns the pool join key (`foundation-model`), and the HTTP `fm serve`
    // endpoint is never materialized as a target, so the join key is claimed
    // exactly once. Otherwise the HTTP `fm serve` probe below still serves.
    let in_process = foundation::foundation_model_available();
    if in_process {
        out.push(foundation_target());
    }
    for spec in &servers::SERVERS {
        if in_process && spec.base == FM_SERVE_TARGET {
            continue;
        }
        if on_device_listening(spec) {
            out.push(match spec.kind {
                TargetKind::Chat => chat_target(spec.endpoint, spec.base),
                TargetKind::Decider => decider_target(spec.base),
                TargetKind::Foundation => foundation_target(),
            });
        }
    }
    for (label, base, health) in CHAT_PROBES {
        if http_probe_listening(health) {
            out.push(chat_target(label, base));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// On-device models: detect, choose, and filter discovery
// ---------------------------------------------------------------------------

/// A model this Mac can serve with no extra setup beyond its own app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum OnDeviceModel {
    AppleFoundation,
    DeciderFlash,
}

impl OnDeviceModel {
    const ALL: [OnDeviceModel; 2] = [OnDeviceModel::AppleFoundation, OnDeviceModel::DeciderFlash];

    /// Its row in the server table.
    fn spec(self) -> &'static servers::ServerSpec {
        servers::spec(self)
    }

    /// Public catalog id, the one `--model` takes and the summary prints.
    fn id(self) -> &'static str {
        self.spec().id
    }

    /// The id the relay advertises in its hello frame (pool join key).
    fn local_id(self) -> &'static str {
        self.spec().local_id
    }

    fn label(self) -> &'static str {
        self.spec().label
    }

    /// The local endpoint discovery finds this model at.
    fn base(self) -> &'static str {
        self.spec().base
    }

    /// Accepts the catalog id or the local id, any case.
    fn from_id(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        Self::ALL
            .into_iter()
            .find(|m| m.id().eq_ignore_ascii_case(raw) || m.local_id().eq_ignore_ascii_case(raw))
    }
}

/// Whether one on-device model is reachable right now. Behind a trait so
/// selection is testable off-Mac.
trait OnDeviceDetector {
    /// `Ok(())` when servable now, else a short sentence on how to enable it.
    fn detect(&self, model: OnDeviceModel) -> Result<(), String>;
}

/// The real detector: the same probes the relay routes through.
struct SystemDetector;

impl OnDeviceDetector for SystemDetector {
    fn detect(&self, model: OnDeviceModel) -> Result<(), String> {
        let spec = model.spec();
        if let Some(min) = spec.min_macos {
            if let Some(major) = macos_major_version() {
                if major < min {
                    return Err(format!(
                        "needs macOS {min} or later (this Mac runs {major})"
                    ));
                }
            } else if !cfg!(target_os = "macos") {
                return Err(format!("needs a Mac on macOS {min}+"));
            }
        }
        // The Apple model can also answer in-process, with no HTTP server up.
        if model == OnDeviceModel::AppleFoundation && foundation::foundation_model_available() {
            return Ok(());
        }
        if on_device_listening(spec) {
            return Ok(());
        }
        let why = if !cfg!(target_os = "macos") {
            servers::Absent::NeedsMac
        } else if servers::Host::installed(&servers::SystemHost::default(), spec) {
            servers::Absent::Stopped
        } else {
            servers::Absent::NotInstalled
        };
        Err(spec.next_step(why))
    }
}

/// `detected` for start: a model whose server is installed but stopped
/// counts as available when start may launch it, so the picker offers it.
/// Too old a macOS still says why.
fn with_launch(
    model: OnDeviceModel,
    detected: Result<(), String>,
    launch: bool,
    installed: impl FnOnce() -> bool,
) -> Result<(), String> {
    if detected.is_ok() || !launch {
        return detected;
    }
    if let Some(min) = model.spec().min_macos {
        if macos_major_version().is_some_and(|major| major < min) {
            return detected;
        }
    }
    if installed() {
        Ok(())
    } else {
        detected
    }
}

/// The real detector, plus "installed, start will launch it".
struct StartDetector {
    launch: bool,
}

impl OnDeviceDetector for StartDetector {
    fn detect(&self, model: OnDeviceModel) -> Result<(), String> {
        with_launch(model, SystemDetector.detect(model), self.launch, || {
            servers::Host::installed(&servers::SystemHost::default(), model.spec())
        })
    }
}

/// macOS major version from `sw_vers`, None off-Mac or when it fails.
fn macos_major_version() -> Option<u32> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// One detection result, in `OnDeviceModel::ALL` order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Detected {
    model: OnDeviceModel,
    unavailable: Option<String>,
}

fn detect_all(detector: &dyn OnDeviceDetector) -> Vec<Detected> {
    OnDeviceModel::ALL
        .into_iter()
        .map(|model| Detected {
            model,
            unavailable: detector.detect(model).err(),
        })
        .collect()
}

/// What the user asked for on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelChoice {
    /// No `--model` / `--all`: picker at a TTY, else everything.
    Default,
    All,
    Only(Vec<OnDeviceModel>),
}

fn parse_model_choice(models: Option<&str>, all: bool) -> Result<ModelChoice, String> {
    let ids: Vec<&str> = models
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if all && !ids.is_empty() {
        return Err("Pass --model or --all, not both.".into());
    }
    if all {
        return Ok(ModelChoice::All);
    }
    if ids.is_empty() {
        return Ok(ModelChoice::Default);
    }
    let mut out = Vec::new();
    for id in ids {
        let model = OnDeviceModel::from_id(id).ok_or_else(|| {
            let known: Vec<&str> = OnDeviceModel::ALL.iter().map(|m| m.id()).collect();
            format!(
                "Unknown on-device model \"{id}\" (known: {}).",
                known.join(", ")
            )
        })?;
        if !out.contains(&model) {
            out.push(model);
        }
    }
    Ok(ModelChoice::Only(out))
}

/// Picker callback: gets the available models (all pre-checked), returns
/// the ones the user kept.
type Picker<'a> = &'a dyn Fn(&[OnDeviceModel]) -> Result<Vec<OnDeviceModel>, String>;

/// Resolve the set of on-device models to serve. A chosen model that is not
/// reachable yet stays in the set: discovery re-runs on every reconnect, so
/// it is served as soon as its app comes up.
fn select_models(
    choice: &ModelChoice,
    detected: &[Detected],
    picker: Option<Picker<'_>>,
) -> Result<BTreeSet<OnDeviceModel>, String> {
    match choice {
        ModelChoice::All => Ok(OnDeviceModel::ALL.into_iter().collect()),
        ModelChoice::Only(list) => Ok(list.iter().copied().collect()),
        ModelChoice::Default => {
            let available: Vec<OnDeviceModel> = detected
                .iter()
                .filter(|d| d.unavailable.is_none())
                .map(|d| d.model)
                .collect();
            match picker {
                Some(pick) if !available.is_empty() => Ok(pick(&available)?.into_iter().collect()),
                _ => Ok(OnDeviceModel::ALL.into_iter().collect()),
            }
        }
    }
}

/// Drop discovered on-device endpoints the user did not choose. Other local
/// servers (Ollama, LM Studio, …) are untouched. The in-process Apple system
/// model endpoint is kept only while `AppleFoundation` is selected.
fn keep_selected(targets: Vec<Target>, serve: &BTreeSet<OnDeviceModel>) -> Vec<Target> {
    targets
        .into_iter()
        .filter(|t| {
            if t.kind == TargetKind::Foundation {
                return serve.contains(&OnDeviceModel::AppleFoundation);
            }
            OnDeviceModel::ALL
                .into_iter()
                .find(|m| m.base() == t.base)
                .is_none_or(|m| serve.contains(&m))
        })
        .collect()
}

/// Build the router: explicit `--target` URLs (comma-separated; repeated
/// flags are joined by the parser) override discovery. With nothing found,
/// fall back to the historical default so the relay still starts.
fn build_router(explicit: Option<&str>, serve: &BTreeSet<OnDeviceModel>) -> Router {
    let urls: Vec<&str> = explicit
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut targets = if urls.is_empty() {
        keep_selected(discover_targets(), serve)
    } else {
        urls.iter()
            .map(|u| {
                // A server that lists models is a chat endpoint: never POST a
                // real decision to it. Only a silent one is probed.
                let fetched = fetch_local_models(u);
                if fetched.is_empty() && probe_decider(u) {
                    decider_target(u)
                } else {
                    Target {
                        label: "--target".to_string(),
                        base: u.to_string(),
                        kind: TargetKind::Chat,
                        models: advertised_models(u, &fetched),
                        fm_models: is_fm_serve_target(u).then(|| fetched.clone()),
                    }
                }
            })
            .collect()
    };
    if targets.is_empty() {
        targets.push(chat_target(FALLBACK_LABEL, DEFAULT_TARGET));
    }
    Router::new(targets)
}

/// Label of the endpoint used when nothing was detected.
const FALLBACK_LABEL: &str = "default target";

/// True when something answers within PROBE_TIMEOUT_MS (any status counts).
fn http_probe_listening(url: &str) -> bool {
    matches!(
        ureq::get(url).timeout(Duration::from_millis(800)).call(),
        Ok(_) | Err(ureq::Error::Status(_, _))
    )
}

// ---------------------------------------------------------------------------
// Capability advertisement (hello frame)
// ---------------------------------------------------------------------------

/// True when `target` is (or looks like) fm serve's loopback endpoint.
fn is_fm_serve_target(target: &str) -> bool {
    target.contains("127.0.0.1:1976") || target.contains("localhost:1976")
}

/// Fetch the local target's OpenAI-compatible /models list. Empty vec when
/// unreachable — connect without advertising rather than failing: a device
/// with no advertised models simply never becomes a pool donor.
fn fetch_local_models(target: &str) -> Vec<String> {
    let url = format!("{}/models", target.trim_end_matches('/'));
    let result = ureq::get(&url).timeout(Duration::from_secs(5)).call();
    match result {
        Ok(resp) if (200..300).contains(&resp.status()) => {
            let body = resp.into_string().unwrap_or_default();
            serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v.get("data").and_then(|d| d.as_array()).map(|arr| {
                        arr.iter()
                            .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                            .map(str::to_string)
                            .collect()
                    })
                })
                .unwrap_or_default()
        }
        Ok(resp) => {
            vlog(&format!(
                "could not reach {url} (status {}) — connecting without capability advertisement",
                resp.status()
            ));
            Vec::new()
        }
        Err(err) => {
            vlog(&format!(
                "could not reach {url} ({err}) — connecting without capability advertisement"
            ));
            Vec::new()
        }
    }
}

/// Model list for the hello frame: the local /models ids plus the fm serve
/// join-key correction (see FM_SERVE_ADVERTISED_MODEL_ID).
fn advertised_models(target: &str, fetched: &[String]) -> Vec<String> {
    let mut models = fetched.to_vec();
    if is_fm_serve_target(target) && !models.iter().any(|m| m == FM_SERVE_ADVERTISED_MODEL_ID) {
        models.push(FM_SERVE_ADVERTISED_MODEL_ID.to_string());
    }
    models
}

/// fm serve's own id for Apple's on-device model. The cloud sends the
/// catalog name (`foundation-model` or `apple/foundation-model`), which fm
/// serve rejects: "Unknown model 'foundation-model'. Available: system, pcc".
const FM_SERVE_SYSTEM_MODEL: &str = "system";

/// The id to send to fm serve for `requested`, given its live model list.
/// `Ok(None)` = send as is. Exactly one local model: use it. Otherwise an
/// error that lists what fm serve actually has.
fn map_fm_model(requested: &str, local: &[String]) -> Result<Option<String>, String> {
    if local.is_empty() || local.iter().any(|m| m == requested) {
        return Ok(None);
    }
    let is_catalog_name = requested == FM_SERVE_ADVERTISED_MODEL_ID
        || OnDeviceModel::from_id(requested) == Some(OnDeviceModel::AppleFoundation);
    if is_catalog_name && local.iter().any(|m| m == FM_SERVE_SYSTEM_MODEL) {
        return Ok(Some(FM_SERVE_SYSTEM_MODEL.to_string()));
    }
    if let [only] = local {
        return Ok(Some(only.clone()));
    }
    Err(format!(
        "Model \"{requested}\" is not served by fm serve on this device (available: {}).",
        local.join(", ")
    ))
}

/// Rewrite the request body's `model` for `target` when it is fm serve.
fn body_for_target(target: &Target, body: &str) -> Result<String, String> {
    let Some(local) = &target.fm_models else {
        return Ok(body.to_string());
    };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(body) else {
        return Ok(body.to_string());
    };
    let Some(requested) = v.get("model").and_then(|m| m.as_str()).map(str::to_string) else {
        return Ok(body.to_string());
    };
    match map_fm_model(&requested, local)? {
        Some(id) => {
            vlog(&format!("mapped model {requested} -> {id} for fm serve"));
            v["model"] = json!(id);
            Ok(v.to_string())
        }
        None => Ok(body.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Local request handling (worker threads)
// ---------------------------------------------------------------------------

/// Forward one request frame to the local target and stream the response back
/// as head/chunk/done frames through `tx`. Chunks are enqueued as soon as they
/// are read from the local response — never buffered whole, so SSE (or any
/// streamed body) round-trips incrementally. Runs on its own thread; `cancel`
/// flips the flag and the stream loop bails between reads.
fn handle_request(
    tx: &mpsc::SyncSender<ClientFrame>,
    frame: &RequestFrame,
    target: &str,
    cancel: &AtomicBool,
) {
    let path = if frame.path.starts_with('/') {
        frame.path.clone()
    } else {
        format!("/{}", frame.path)
    };
    let url = format!("{}{}", target.trim_end_matches('/'), path);

    // Connect fast, tolerate slow generation: no overall cap, generous idle
    // gap between body reads.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(300))
        .build();

    let resp = match agent
        .post(&url)
        .set("Content-Type", "application/json")
        .send_string(&frame.body)
    {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, resp)) => {
            // Non-2xx from the local target still relays real status/body.
            let _ = tx.send(ClientFrame::Head {
                id: frame.id.clone(),
                status: code,
                content_type: content_type_of(&resp),
            });
            let body = resp.into_string().unwrap_or_default();
            if !body.is_empty() {
                let _ = tx.send(ClientFrame::Chunk {
                    id: frame.id.clone(),
                    data: body,
                });
            }
            let _ = tx.send(ClientFrame::Done {
                id: frame.id.clone(),
            });
            return;
        }
        Err(_) if cancel.load(Ordering::SeqCst) => return, // cancelled — no frame
        Err(err) => {
            let message = err.to_string();
            vlog(&format!("request {} failed: {message}", frame.id));
            let _ = tx.send(ClientFrame::Error {
                id: frame.id.clone(),
                message: format!("Local target unreachable: {message}"),
            });
            return;
        }
    };

    let _ = tx.send(ClientFrame::Head {
        id: frame.id.clone(),
        status: resp.status(),
        content_type: content_type_of(&resp),
    });

    // Stream the body in chunks. UTF-8 safe: any partial multi-byte sequence
    // stays buffered until its tail arrives instead of being split mid-codepoint.
    let mut reader = resp.into_reader();
    let mut buf = [0u8; 8192];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        if cancel.load(Ordering::SeqCst) {
            return; // cancelled by cloud — stop relaying
        }
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                let flush_len = utf8_flush_len(&pending);
                if flush_len > 0 {
                    let data = String::from_utf8_lossy(&pending[..flush_len]).into_owned();
                    pending.drain(..flush_len);
                    if tx
                        .send(ClientFrame::Chunk {
                            id: frame.id.clone(),
                            data,
                        })
                        .is_err()
                    {
                        return; // socket died — nothing more to do
                    }
                }
            }
            Err(err) => {
                if !cancel.load(Ordering::SeqCst) {
                    let message = err.to_string();
                    vlog(&format!(
                        "request {} failed mid-stream: {message}",
                        frame.id
                    ));
                    let _ = tx.send(ClientFrame::Error {
                        id: frame.id.clone(),
                        message: format!("Local target unreachable: {message}"),
                    });
                }
                return;
            }
        }
    }
    if !pending.is_empty() && !cancel.load(Ordering::SeqCst) {
        let data = String::from_utf8_lossy(&pending).into_owned();
        let _ = tx.send(ClientFrame::Chunk {
            id: frame.id.clone(),
            data,
        });
    }
    let _ = tx.send(ClientFrame::Done {
        id: frame.id.clone(),
    });
}

/// Serve one request frame from the in-process Apple system model and stream
/// the answer back as `head`/`chunk`/`done` frames through `tx`. Runs on its
/// own thread; `cancel` aborts the generation between snapshots. Only chat
/// completions are answered — every other path is an error frame.
fn handle_foundation_request(
    tx: &mpsc::SyncSender<ClientFrame>,
    frame: &RequestFrame,
    cancel: &AtomicBool,
) {
    if cancel.load(Ordering::SeqCst) {
        return;
    }
    if !frame.path.is_empty() && !frame.path.contains("chat/completions") {
        let _ = tx.send(ClientFrame::Error {
            id: frame.id.clone(),
            message: format!("local relay does not serve {}", frame.path),
        });
        return;
    }
    let mut session = match foundation::FmSession::from_chat_body(&frame.id, &frame.body) {
        Ok(session) => session,
        Err(message) => {
            let _ = tx.send(ClientFrame::Error {
                id: frame.id.clone(),
                message,
            });
            return;
        }
    };
    let tx = tx.clone();
    let id = frame.id.clone();
    foundation::run_foundation_model(&mut session, cancel, move |event| {
        if matches!(&event, foundation::FmEvent::Error(_)) {
            vlog(&format!("request {id} failed"));
        }
        let _ = tx.send(foundation_client_frame(&id, event));
    });
}

fn foundation_client_frame(id: &str, event: foundation::FmEvent) -> ClientFrame {
    match event {
        foundation::FmEvent::Head {
            status,
            content_type,
        } => ClientFrame::Head {
            id: id.to_string(),
            status,
            content_type,
        },
        foundation::FmEvent::Chunk(data) => ClientFrame::Chunk {
            id: id.to_string(),
            data,
        },
        foundation::FmEvent::Done => ClientFrame::Done { id: id.to_string() },
        foundation::FmEvent::Error(message) => ClientFrame::Error {
            id: id.to_string(),
            message,
        },
    }
}

/// Longest prefix of `bytes` that can be flushed: everything except a
/// trailing truncated codepoint. An invalid byte (not a truncation) is
/// flushed too, so `from_utf8_lossy` turns it into U+FFFD instead of stalling
/// the stream behind it.
fn utf8_flush_len(bytes: &[u8]) -> usize {
    let mut flushed = 0;
    loop {
        match std::str::from_utf8(&bytes[flushed..]) {
            Ok(_) => return bytes.len(),
            Err(e) => match e.error_len() {
                Some(bad) => flushed += e.valid_up_to() + bad,
                None => return flushed + e.valid_up_to(),
            },
        }
    }
}

fn content_type_of(resp: &ureq::Response) -> String {
    resp.header("content-type")
        .unwrap_or("application/json")
        .to_string()
}

// ---------------------------------------------------------------------------
// Pool sharing (--pool)
// ---------------------------------------------------------------------------

/// Resolve this device's id for `--pool`: cached `relay_device_id` first; a
/// pre-cache pairing falls back to GET /relay/devices, using the result only
/// when exactly one active device exists — ambiguous otherwise, so point the
/// user at the dashboard instead of guessing.
fn resolve_relay_device_id(
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<String, String> {
    if let Some(id) = cached_relay_device_id(env) {
        return Ok(id);
    }
    let bin = crate::help::invoked_bin();
    let api_key = resolve_sk_ar_key(parsed, env).ok_or_else(|| {
        view::error(
            "no AnyRouter key to look up this device",
            "",
            &format!("`{bin} login`"),
        )
    })?;
    let url = format!("{}/relay/devices", relay_api_base(parsed, env));
    let (status, body) = crate::http::http_get(&url, Some(&api_key))?;
    let value: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let active_ids: Vec<&str> = value
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|d| d.get("status").and_then(|s| s.as_str()) == Some("active"))
                .filter_map(|d| d.get("id").and_then(|i| i.as_str()))
                .collect()
        })
        .unwrap_or_default();
    if !(200..300).contains(&status) || active_ids.is_empty() {
        return Err(view::error(
            "this device is not paired yet",
            "it pairs on its first connect",
            &format!("`{bin} relay start` (or `{bin} relay pair`)"),
        ));
    }
    if active_ids.len() > 1 {
        return Err(view::error(
            "several paired devices found; cannot tell which one this is",
            "this machine has no cached device id",
            &format!(
                "manage it at {DEVICES_URL}, or `{bin} relay unpair --yes` then `{bin} relay pair`"
            ),
        ));
    }
    let id = active_ids[0].to_string();
    write_profile_field(RELAY_DEVICE_ID_FIELD, &id, env)?;
    Ok(id)
}

/// The device id this config last connected as: the cloud's `device` frame,
/// or `relay pair`.
fn cached_relay_device_id(env: &BTreeMap<String, String>) -> Option<String> {
    let path = crate::config::resolve_config_path(None, env);
    let cfg = crate::key::load_config_if_present(&path)?;
    cfg.profiles
        .get(&cfg.active_profile)?
        .relay_device_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// PATCH /relay/devices/:id { pool_enabled } with the resolved sk-ar key.
/// Opting in lets this device serve OTHER users' requests when its own
/// capacity is idle, earning credits per served request.
fn set_relay_pool(
    device_id: &str,
    enabled: bool,
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<(), String> {
    let api_key = resolve_sk_ar_key(parsed, env).ok_or_else(|| {
        "Could not resolve an AnyRouter API key to change pool sharing.".to_string()
    })?;
    let url = format!("{}/relay/devices/{device_id}", relay_api_base(parsed, env));
    let body = json!({ "pool_enabled": enabled }).to_string();
    let (status, resp) = crate::http::http_patch(&url, Some(&api_key), Some(&body))?;
    if !(200..300).contains(&status) {
        let value: serde_json::Value =
            serde_json::from_str(&resp).unwrap_or(serde_json::Value::Null);
        let msg = value
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("unexpected response");
        return Err(format!("Could not change pool sharing ({status}): {msg}"));
    }
    Ok(())
}

/// `…/api/v1` of the resolved base (`--base-url`, else the active profile,
/// else prod), so staging and mock clouds get the relay's calls, not prod.
fn relay_api_base(parsed: &crate::parse::ParsedArgs, env: &BTreeMap<String, String>) -> String {
    let path = crate::config::resolve_config_path(None, env);
    let cfg = crate::key::load_config_if_present(&path);
    let base = crate::key::resolve_base_url(
        &parsed.flags,
        cfg.as_ref().and_then(|c| c.profiles.get(&c.active_profile)),
    );
    format!("{}/v1", base.trim_end_matches('/'))
}

/// The relay websocket for an API base: `https`→`wss`, `http`→`ws`.
fn relay_ws_url(api_base: &str) -> String {
    let ws = match api_base.split_once("://") {
        Some(("http", rest)) => format!("ws://{rest}"),
        Some(("https", rest)) => format!("wss://{rest}"),
        _ => api_base.to_string(),
    };
    format!("{ws}/relay/connect")
}

fn resolve_sk_ar_key(
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Option<String> {
    let path = crate::config::resolve_config_path(None, env);
    crate::key::resolve_api_key(
        &parsed.flags,
        env,
        crate::key::load_config_if_present(&path)
            .as_ref()
            .and_then(|c| c.profiles.get(&c.active_profile)),
    )
}

// ---------------------------------------------------------------------------
// Transport — outbound WebSocket with reconnect + exponential backoff
// ---------------------------------------------------------------------------

type Ws = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

/// Why a connect attempt failed. `Refused` (401/403) will not heal on retry —
/// the credential is wrong or out of scope — so the loop stops instead of
/// hammering the cloud with it forever. `DeviceLimit` (409) is the relay
/// device limit, which is not a credential problem, so it gets its own
/// message and hint.
#[derive(Debug)]
enum ConnectError {
    Refused(String),
    DeviceLimit(String),
    Transient(String),
}

impl From<String> for ConnectError {
    fn from(message: String) -> Self {
        Self::Transient(message)
    }
}

/// Append the query the cloud uses for implicit (sk-ar) pairing: `name=`
/// labels a new device, and `device=<id>` reconnects as the device this
/// config was bound to, or `fresh=1` asks for this machine's own device
/// instead of reusing another machine's. A URL that already carries a query
/// is left alone. The credential never goes in the URL (it lands in access
/// logs): it rides the header.
fn relay_url_with_name(url: &str, name: &str, device_id: Option<&str>) -> String {
    if url.contains('?') {
        return url.to_string();
    }
    let encode = |s: &str| -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    let bind = match device_id {
        Some(id) => format!("device={}", encode(id)),
        None => "fresh=1".to_string(),
    };
    format!("{url}?name={}&{bind}", encode(name))
}

fn connect_ws(url: &str, token: &str) -> Result<Ws, ConnectError> {
    use tungstenite::client::IntoClientRequest;
    use tungstenite::http::header::AUTHORIZATION;
    use tungstenite::http::HeaderValue;

    let mut request = url
        .into_client_request()
        .map_err(|e| format!("invalid relay url \"{url}\": {e}"))?;
    // sk-ar keys are accepted here too (one key for everything); never sent
    // via ?token= query — URLs land in access logs and must not carry keys.
    let value = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| "relay token contains invalid characters".to_string())?;
    request.headers_mut().insert(AUTHORIZATION, value);
    // `tungstenite::client::connect` dials with no timeout, so a blackholed
    // relay host would hang the reconnect loop. Dial ourselves (15s, same as
    // the relay HTTP client), then let tungstenite do TLS + handshake.
    let uri = request.uri();
    let tls = uri.scheme_str() == Some("wss");
    let host = uri
        .host()
        .ok_or_else(|| format!("relay url \"{url}\" has no host"))?
        .to_string();
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let stream = dial_with_timeout(&host, port, WS_CONNECT_TIMEOUT)?;
    // Same cap for TLS and the upgrade: a host that accepts TCP and never
    // answers would otherwise block this handshake forever.
    let _ = stream.set_read_timeout(Some(WS_CONNECT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WS_CONNECT_TIMEOUT));
    match tungstenite::client_tls(request, stream) {
        Ok((ws, _resp)) => Ok(ws),
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(resp)))
            if matches!(resp.status().as_u16(), 401 | 403) =>
        {
            Err(ConnectError::Refused(format!(
                "the relay refused this credential ({}). Check the key is valid and allowed to \
use /api/v1/relay/connect, or pass --token.",
                resp.status().as_u16()
            )))
        }
        // Every device slot is taken by a connected device: retrying cannot
        // free one, so stop and say how to remove a device.
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(resp)))
            if resp.status().as_u16() == 409 =>
        {
            Err(ConnectError::DeviceLimit(device_limit_message(
                resp.body().as_deref(),
            )))
        }
        Err(e) => Err(ConnectError::Transient(e.to_string())),
    }
}

/// The cloud's `relay_device_limit` message, or a fallback when the body is
/// missing or not JSON.
fn device_limit_message(body: Option<&[u8]>) -> String {
    body.and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok())
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            format!("this account has no free device slot. Remove one at {DEVICES_URL}")
        })
}

const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

fn dial_with_timeout(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<std::net::TcpStream, String> {
    use std::net::ToSocketAddrs;
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("could not resolve relay host {host}: {e}"))?;
    let mut last = format!("relay host {host} resolved to no addresses");
    for addr in addrs {
        match std::net::TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => return Ok(s),
            Err(e) => last = format!("could not connect to relay {addr}: {e}"),
        }
    }
    Err(last)
}

/// State one live connection owns, built fresh for each socket: a worker of
/// a dropped socket finds its queue gone and stops, so none of its frames
/// reach the next socket.
struct ConnState {
    /// Outbound frames queued by worker threads; drained by the main loop.
    /// Bounded, so a stalled socket stalls the workers, not memory.
    tx: mpsc::SyncSender<ClientFrame>,
    rx: Receiver<ClientFrame>,
    /// Cancel flags keyed by request id; `cancel` frames flip them.
    in_flight: Arc<Mutex<BTreeMap<String, Arc<AtomicBool>>>>,
    /// Counters the status screen and state file show.
    stats: Arc<Stats>,
    /// True once pool comes from the user, not the default: only then does
    /// the hello carry it.
    pool_chosen: Arc<AtomicBool>,
    timing: Timing,
    /// Requests the previous socket dropped mid-flight. The cloud may still
    /// be waiting on them (a half-open socket never told it), so each gets
    /// an `error` right after the hello.
    orphans: std::cell::RefCell<Vec<String>>,
}

impl ConnState {
    fn new(stats: Arc<Stats>) -> Self {
        let (tx, rx) = mpsc::sync_channel(OUTBOUND_QUEUE);
        ConnState {
            tx,
            rx,
            in_flight: Arc::new(Mutex::new(BTreeMap::new())),
            stats,
            pool_chosen: Arc::default(),
            timing: Timing::default(),
            orphans: Default::default(),
        }
    }
}

/// Why `serve_connection` returned.
#[derive(Debug, PartialEq, Eq)]
enum ConnEnd {
    /// Socket closed, failed or went silent: reconnect with backoff.
    Dropped(String),
    /// Ctrl-C, SIGTERM or `q`.
    Stop,
    /// `r` key: rediscover local servers and reconnect now.
    Rescan,
}

/// One full connection lifecycle: handshake → hello → serve frames until the
/// socket dies or a stop/rescan is requested.
fn serve_connection(
    ws: &mut Ws,
    router: &Router,
    state: &ConnState,
    max_concurrency: Option<u32>,
    on_device: &dyn Fn(&str),
) -> ConnEnd {
    vlog("connected");

    // Capability advertisement on every connect/reconnect (#1128), so models
    // added to the local server show up at the next reconnect at latest.
    let models = router.all_models();
    let hello = ClientFrame::Hello {
        models: models.clone(),
        max_concurrency,
        pool: state
            .pool_chosen
            .load(Ordering::Relaxed)
            .then(|| state.stats.pool.load(Ordering::Relaxed)),
    };
    if let Err(e) = send_frame(ws, &hello) {
        return ConnEnd::Dropped(e);
    }
    // Popped only once sent, so a socket that dies here hands the rest on.
    loop {
        let Some(id) = state.orphans.borrow().last().cloned() else {
            break;
        };
        let orphan = ClientFrame::Error {
            id,
            message: "relay connection dropped mid-request".into(),
        };
        if let Err(e) = send_frame(ws, &orphan) {
            return ConnEnd::Dropped(e);
        }
        state.orphans.borrow_mut().pop();
    }
    vlog(&format!(
        "advertised {} model(s): {}",
        models.len(),
        if models.is_empty() {
            "(none)".into()
        } else {
            models.join(", ")
        }
    ));

    // Largest usage total seen per request; added to the counter on `done`.
    let mut tokens: BTreeMap<String, u64> = BTreeMap::new();
    let mut last_heard = std::time::Instant::now();
    let mut last_ping = last_heard;
    let mut busy = false;
    loop {
        if last_heard.elapsed() >= state.timing.dead_after {
            return ConnEnd::Dropped(format!(
                "no reply from the cloud in {}s",
                state.timing.dead_after.as_secs_f64().round()
            ));
        }
        if live::stop_requested() != 0 {
            let _ = ws.close(None);
            let _ = ws.flush();
            return ConnEnd::Stop;
        }
        if live::take_rescan() {
            let _ = ws.close(None);
            let _ = ws.flush();
            return ConnEnd::Rescan;
        }
        // 1. Drain worker output first so streamed chunks go out promptly.
        //    A queue that never empties (a long stream to a slow cloud)
        //    still yields after one poll interval, so pongs and cancels are
        //    read; the read then barely waits, to keep the stream moving.
        let slice = std::time::Instant::now();
        let was_busy = busy;
        busy = false;
        loop {
            if slice.elapsed() >= Duration::from_millis(SOCKET_POLL_MS) {
                busy = true;
                break;
            }
            match state.rx.try_recv() {
                Ok(frame) => {
                    // Settled only once sent: a request whose last frame
                    // never left stays in flight, so the next socket reports
                    // it as an orphan.
                    if let Err(e) = send_frame(ws, &frame) {
                        return ConnEnd::Dropped(e);
                    }
                    match &frame {
                        ClientFrame::Chunk { id, data } => {
                            if let Some(n) = live::total_tokens(data) {
                                let e = tokens.entry(id.clone()).or_insert(0);
                                *e = (*e).max(n);
                            }
                        }
                        ClientFrame::Done { id } => {
                            lock_in_flight(&state.in_flight).remove(id);
                            if let Some(n) = tokens.remove(id) {
                                state.stats.add_tokens(n);
                            }
                            state.stats.finish_request(false);
                        }
                        ClientFrame::Error { id, .. } => {
                            lock_in_flight(&state.in_flight).remove(id);
                            tokens.remove(id);
                            state.stats.finish_request(true);
                        }
                        _ => {}
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return ConnEnd::Dropped("outbound queue closed".into())
                }
            }
        }

        // 2. Read every frame the cloud has sent. WouldBlock (read timeout)
        //    is not fatal: it ends the read and loops back to the drain.
        if last_ping.elapsed() >= state.timing.ping_every {
            last_ping = std::time::Instant::now();
            if let Err(e) = ws.send(tungstenite::Message::Ping(Vec::new())) {
                return ConnEnd::Dropped(format!("ping failed: {e}"));
            }
        }
        if busy != was_busy {
            let wait = if busy {
                BUSY_POLL
            } else {
                Duration::from_millis(SOCKET_POLL_MS)
            };
            set_socket_timeouts(ws, wait, state.timing.dead_after);
        }
        loop {
            let msg = ws.read();
            if msg.is_ok() {
                last_heard = std::time::Instant::now();
            }
            match msg {
                Ok(tungstenite::Message::Text(text)) => match parse_server_frame(&text) {
                    Some(ServerFrame::Request(frame)) => {
                        state.stats.requests.fetch_add(1, Ordering::Relaxed);
                        match router
                            .route(&frame.path, &frame.body)
                            .and_then(|t| Ok((t, body_for_target(t, &frame.body)?)))
                        {
                            Ok((t, body)) => {
                                state.stats.in_flight.fetch_add(1, Ordering::Relaxed);
                                let frame = RequestFrame { body, ..frame };
                                let rejected = if t.kind == TargetKind::Foundation {
                                    spawn_foundation_request(state, frame)
                                } else {
                                    let base = t.base.clone();
                                    spawn_request(state, frame, base)
                                };
                                if let Err(rejected) = rejected {
                                    if let Err(e) = send_frame(ws, &rejected) {
                                        return ConnEnd::Dropped(e);
                                    }
                                }
                            }
                            Err(message) => {
                                state.stats.errors.fetch_add(1, Ordering::Relaxed);
                                vlog(&format!("rejected request: {message}"));
                                let rejected = ClientFrame::Error {
                                    id: frame.id,
                                    message,
                                };
                                if let Err(e) = send_frame(ws, &rejected) {
                                    return ConnEnd::Dropped(e);
                                }
                            }
                        }
                    }
                    Some(ServerFrame::Device { id, name }) => {
                        vlog(&format!("bound to device {id} (\"{name}\")"));
                        on_device(&id);
                    }
                    Some(ServerFrame::Cancel(id)) => {
                        if let Some(flag) = lock_in_flight(&state.in_flight).remove(&id) {
                            flag.store(true, Ordering::SeqCst);
                            tokens.remove(&id);
                            state.stats.finish_request(false);
                        }
                    }
                    None => vlog("ignoring malformed frame from cloud"),
                },
                Ok(tungstenite::Message::Close(_)) => {
                    return ConnEnd::Dropped("the cloud closed the connection".into());
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(ref e)) if is_poll_timeout(e.kind()) => {
                    break;
                }
                Err(tungstenite::Error::ConnectionClosed)
                | Err(tungstenite::Error::AlreadyClosed) => {
                    return ConnEnd::Dropped("the connection closed".into())
                }
                Err(err) => {
                    state.stats.set_error(Some(err.to_string()));
                    return ConnEnd::Dropped(err.to_string());
                }
            }
        }
    }
}

/// A read timeout surfaces as `WouldBlock` on Unix but `TimedOut` on Windows.
/// Treating `TimedOut` as fatal dropped the relay socket every poll there.
/// `Interrupted` is a signal (Ctrl-C, SIGTERM) landing mid-read: the loop
/// then sees the stop flag instead of treating it as a dropped socket.
fn is_poll_timeout(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}

/// First up-to-`max_chars` characters of `s`, safe on any input (the id is
/// cloud-controlled and only used for diagnostics).
fn short_id(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// Spawn a worker thread for one incoming request frame and register its
/// cancel flag. `target` is the base URL of the endpoint that owns the model.
fn lock_in_flight(
    map: &Arc<Mutex<BTreeMap<String, Arc<AtomicBool>>>>,
) -> std::sync::MutexGuard<'_, BTreeMap<String, Arc<AtomicBool>>> {
    map.lock().unwrap_or_else(|e| e.into_inner())
}

/// Returns the `error` frame to send when no worker could start. The caller
/// sends it straight on the socket: the main loop is the queue's only
/// reader, so queueing it could block forever on a full queue.
fn spawn_request(
    state: &ConnState,
    frame: RequestFrame,
    target: String,
) -> Result<(), ClientFrame> {
    let cancel = Arc::new(AtomicBool::new(false));
    let id = frame.id.clone();
    lock_in_flight(&state.in_flight).insert(id.clone(), Arc::clone(&cancel));
    let tx = state.tx.clone();
    match std::thread::Builder::new()
        .name(format!("relay-req-{}", short_id(&id, 8)))
        .spawn(move || handle_request(&tx, &frame, &target, &cancel))
    {
        Ok(_) => Ok(()),
        Err(err) => {
            lock_in_flight(&state.in_flight).remove(&id);
            Err(ClientFrame::Error {
                id,
                message: format!("Could not spawn relay worker: {err}"),
            })
        }
    }
}

/// Spawn a worker thread for one incoming request frame served by the
/// in-process Apple system model and register its cancel flag.
fn spawn_foundation_request(state: &ConnState, frame: RequestFrame) -> Result<(), ClientFrame> {
    let cancel = Arc::new(AtomicBool::new(false));
    let id = frame.id.clone();
    lock_in_flight(&state.in_flight).insert(id.clone(), Arc::clone(&cancel));
    let tx = state.tx.clone();
    match std::thread::Builder::new()
        .name(format!("relay-req-{}", short_id(&id, 8)))
        .spawn(move || handle_foundation_request(&tx, &frame, &cancel))
    {
        Ok(_) => Ok(()),
        Err(err) => {
            lock_in_flight(&state.in_flight).remove(&id);
            Err(ClientFrame::Error {
                id,
                message: format!("Could not spawn relay worker: {err}"),
            })
        }
    }
}

/// Cancel every request still running on a finished socket and return their
/// ids.
fn abort_in_flight(in_flight: &Arc<Mutex<BTreeMap<String, Arc<AtomicBool>>>>) -> Vec<String> {
    let mut map = lock_in_flight(in_flight);
    for flag in map.values() {
        flag.store(true, Ordering::SeqCst);
    }
    std::mem::take(&mut *map).into_keys().collect()
}

/// A failed write ends the connection: the socket is dead or stalled past
/// the write timeout, and retrying on it only delays the reconnect.
fn send_frame(ws: &mut Ws, frame: &ClientFrame) -> Result<(), String> {
    ws.send(tungstenite::Message::text(frame.to_json()))
        .map_err(|err| format!("send {} failed: {err}", frame_type_name(frame)))
}

fn frame_type_name(frame: &ClientFrame) -> &'static str {
    match frame {
        ClientFrame::Head { .. } => "head",
        ClientFrame::Chunk { .. } => "chunk",
        ClientFrame::Done { .. } => "done",
        ClientFrame::Error { .. } => "error",
        ClientFrame::Hello { .. } => "hello",
    }
}

/// Bytes the kernel may hold unsent for the relay socket. A pong comes back
/// only after the cloud reads everything sent before the ping, so an
/// autotuned multi-megabyte send buffer on a slow uplink delays it past the
/// liveness window. Linux doubles the value, so the backlog is 512 KiB
/// there: 4s on a 1 Mbit/s uplink, and still 20 Mbit/s at 100 ms RTT.
#[cfg(unix)]
const SOCKET_SEND_BUFFER: usize = 256 * 1024;

/// Configure short socket read timeouts so `ws.read()` polls instead of
/// blocking forever (WouldBlock drives the main-loop multiplexing). A write
/// may block for `write` at most, so a stalled peer cannot outlast the
/// liveness window.
fn set_socket_timeouts(ws: &Ws, read: Duration, write: Duration) {
    use tungstenite::stream::MaybeTlsStream;
    let sock: Option<&std::net::TcpStream> = match ws.get_ref() {
        MaybeTlsStream::Plain(s) => Some(s),
        MaybeTlsStream::Rustls(tls) => Some(&tls.sock),
        _ => None,
    };
    if let Some(sock) = sock {
        let _ = sock.set_read_timeout(Some(read));
        let _ = sock.set_write_timeout(Some(write));
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd as _;
            let size = SOCKET_SEND_BUFFER as libc::c_int;
            // SAFETY: a valid fd and a c_int option value of the given size.
            unsafe {
                libc::setsockopt(
                    sock.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&size as *const libc::c_int).cast(),
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Discover local servers, publish them to the status screen, and log the
/// endpoint list only when it changed, so a flapping network does not flood
/// the terminal with the same lines on every reconnect.
fn refresh_router(
    explicit: Option<&str>,
    serve: &BTreeSet<OnDeviceModel>,
    stats: &Stats,
    last_sig: &mut String,
    tags: &BTreeMap<&str, &str>,
) -> Router {
    let router = build_router(explicit, serve);
    let endpoints = endpoint_rows(&router, tags);
    let sig = format!("{endpoints:?}{:?}", router.all_models());
    if sig != *last_sig {
        for t in &router.targets {
            if t.label == FALLBACK_LABEL && explicit.is_none() {
                ulog(&format!(
                    "no local server detected; trying {DEFAULT_TARGET}. Start `fm serve`, \
                     Ollama or LM Studio, or pass --target <url>."
                ));
            } else {
                ulog(&format!(
                    "serving {} ({}) at {}",
                    tagged_label(t, tags),
                    t.kind.name(),
                    t.base
                ));
            }
        }
        *last_sig = sig;
    }
    stats.set_router(router.all_models(), endpoints);
    router
}

/// `label (managed)` / `label (external)` for servers start launched or found.
fn tagged_label(t: &Target, tags: &BTreeMap<&str, &str>) -> String {
    match tags.get(t.base.as_str()) {
        Some(tag) => format!("{} ({tag})", t.label),
        None => t.label.clone(),
    }
}

/// The status screen's Endpoints rows.
fn endpoint_rows(router: &Router, tags: &BTreeMap<&str, &str>) -> Vec<(String, String)> {
    router
        .targets
        .iter()
        .map(|t| (tagged_label(t, tags), t.base.clone()))
        .collect()
}

/// Which on-device models to serve: `--model` / `--all`, then the saved
/// choice, then the checklist at a terminal (its answer is saved), else all.
fn resolve_serve(
    choice: &ModelChoice,
    paths: &state::Paths,
    launch: bool,
) -> Result<BTreeSet<OnDeviceModel>, String> {
    let detected = detect_all(&StartDetector { launch });
    for d in &detected {
        if let Some(why) = &d.unavailable {
            ulog(&format!("{} not available: {why}", d.model.id()));
        }
    }
    let mut prefs = state::load_prefs(paths);
    if *choice == ModelChoice::Default {
        if let Some(saved) = &prefs.models {
            return Ok(saved
                .iter()
                .filter_map(|id| OnDeviceModel::from_id(id))
                .collect());
        }
    }
    let asked = std::cell::Cell::new(false);
    let picker = |available: &[OnDeviceModel]| -> Result<Vec<OnDeviceModel>, String> {
        asked.set(true);
        let items: Vec<String> = available
            .iter()
            .map(|m| format!("{}  {}", m.id(), crate::term::dim(m.label())))
            .collect();
        let checked = crate::term::pick_checkboxes(
            "Serve which on-device models? (space toggles, enter starts)",
            &items,
            &vec![true; items.len()],
        )?;
        Ok(available
            .iter()
            .zip(checked)
            .filter_map(|(m, on)| on.then_some(*m))
            .collect())
    };
    let serve = select_models(
        choice,
        &detected,
        crate::term::is_interactive().then_some(&picker as Picker<'_>),
    )?;
    if asked.get() {
        prefs.models = Some(serve.iter().map(|m| m.id().to_string()).collect());
        state::save_prefs(paths, &prefs)?;
        ulog(&format!(
            "Saved. Change it later with `{} relay models enable|disable <id>`.",
            crate::help::invoked_bin()
        ));
    }
    Ok(serve)
}

/// `anyr relay start [--detach]`.
fn run_relay_start(
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
    detach: bool,
) -> Result<i32, String> {
    let bin = crate::help::invoked_bin();
    let mut args = parse_start_args(parsed);
    let pool_flag = cli::pool_flag(parsed)?;
    let quiet = parsed.flag_true("quiet");
    VERBOSE.store(parsed.flag_true("verbose"), Ordering::Relaxed);
    live::set_quiet(quiet);
    let detached_child = env.get(daemon::ENV_DETACHED).is_some_and(|v| v == "1");
    let paths = state::paths(env);

    match state::running(&paths) {
        state::Running::Yes(s) => {
            return Err(view::error(
                &format!("a relay is already running (pid {})", s.pid),
                "one relay runs per AnyRouter config",
                &format!("`{bin} relay status`, or `{bin} relay stop` first"),
            ))
        }
        state::Running::Unresponsive(pid) => {
            return Err(view::error(
                &format!("a relay process (pid {pid}) is not responding"),
                "",
                &format!("`{bin} relay stop`, then start again"),
            ))
        }
        state::Running::No => {}
    }

    let choice = parse_model_choice(args.models.as_deref(), args.all).map_err(|e| {
        crate::cmd::dispatch::hint(&format!("{e}\nSee the list with `{{bin}} relay models`."))
    })?;
    let token = resolve_relay_credential(args.token.as_deref(), parsed, env)?;
    args.device = resolve_device(args.name.as_deref(), parsed, env, quiet || detached_child);
    let serve = if detached_child {
        env.get(daemon::ENV_SERVE)
            .map(|s| s.split(',').filter_map(OnDeviceModel::from_id).collect())
            .unwrap_or_default()
    } else {
        resolve_serve(&choice, &paths, launches_servers(&args))?
    };
    let (pool, pool_source) = state::effective_pool(pool_flag, &state::load_prefs(&paths));

    if detach && !detached_child {
        let child = detached_child_args(&args, pool_flag, parsed.flag_true("verbose"));
        return daemon::spawn(
            &paths,
            daemon::Handoff {
                args: child,
                credential: token,
                api_key: resolve_sk_ar_key(parsed, env),
                serve: serve.iter().map(|m| m.id().to_string()).collect(),
            },
            &bin,
        );
    }

    run_foreground(Foreground {
        parsed,
        env,
        args: &args,
        token: &token,
        serve: &serve,
        pool,
        pool_chosen: pool_source != state::PoolSource::Default,
        follow_pool_pref: pool_flag.is_none(),
        paths,
        detached: detached_child,
        quiet,
    })
}

/// Start launches missing on-device servers unless `--no-launch` or an
/// explicit `--target` (the user named the servers).
fn launches_servers(args: &StartArgs) -> bool {
    !args.no_launch && args.target.is_none()
}

/// How often a running relay checks the servers it started.
const SUPERVISE_EVERY: Duration = Duration::from_secs(2);

/// argv for the detached child. The pool flag is forwarded only when the
/// user gave one, so the child keeps following `relay pool on|off`; the
/// credential never goes here (argv is visible in `ps`).
fn detached_child_args(args: &StartArgs, pool_flag: Option<bool>, verbose: bool) -> Vec<String> {
    let mut child: Vec<String> = vec!["relay".into(), "start".into()];
    if let Some(on) = pool_flag {
        child.push(if on { "--pool" } else { "--no-pool" }.into());
    }
    for (flag, value) in [
        ("--name", Some(args.device.clone())),
        ("--url", args.url.clone()),
        ("--target", args.target.clone()),
        (
            "--max-concurrency",
            args.max_concurrency.map(|n| n.to_string()),
        ),
    ] {
        if let Some(v) = value {
            child.push(flag.into());
            child.push(v);
        }
    }
    if args.no_launch {
        child.push("--no-launch".into());
    }
    if verbose {
        child.push("--verbose".into());
    }
    child
}

struct Foreground<'a> {
    parsed: &'a crate::parse::ParsedArgs,
    env: &'a BTreeMap<String, String>,
    args: &'a StartArgs,
    token: &'a str,
    serve: &'a BTreeSet<OnDeviceModel>,
    pool: bool,
    /// Pool came from a flag or saved pref rather than the default.
    pool_chosen: bool,
    follow_pool_pref: bool,
    paths: state::Paths,
    detached: bool,
    quiet: bool,
}

/// Connect and serve until stopped. Reconnects with backoff; a refused
/// credential ends the run.
fn run_foreground(f: Foreground<'_>) -> Result<i32, String> {
    let bin = crate::help::invoked_bin();
    let stats = Arc::new(Stats::default());
    stats.pool.store(f.pool, Ordering::Relaxed);
    let meta = live::Meta {
        pid: std::process::id(),
        started_at: state::now_secs(),
        detached: f.detached,
        device: f.args.device.clone(),
    };
    state::write_pid(&f.paths, meta.pid)?;
    live::install_signal_handlers(f.detached);
    let live_screen =
        live::wants_live_screen(f.quiet, f.detached, f.env.get("TERM").map(String::as_str));

    let default_ws = relay_ws_url(&relay_api_base(f.parsed, f.env));
    let ws_base = f
        .args
        .url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&default_ws);

    // Find or start the on-device servers before the first discovery, so it
    // sees them. Explicit --target URLs are the user's own servers.
    let supervisor: Option<Arc<Mutex<servers::Supervisor>>> = f.args.target.is_none().then(|| {
        // The in-process Apple system model needs no server process: while it
        // covers the Apple model, never launch `fm serve` as a child.
        let in_process = foundation::foundation_model_available();
        let specs = f
            .serve
            .iter()
            .map(|m| m.spec())
            .filter(|spec| !(in_process && spec.base == FM_SERVE_TARGET))
            .collect();
        Arc::new(Mutex::new(servers::Supervisor::start(
            specs,
            f.args.no_launch,
            // The detached child must connect within `--detach`'s wait;
            // supervision picks a slow server up once it answers.
            !f.detached,
            Box::new(servers::SystemHost::default()),
            Box::new(|m: &str| ulog(m)),
        )))
    });
    let tags = || {
        supervisor
            .as_ref()
            .map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).tags())
            .unwrap_or_default()
    };
    let supervise_done = Arc::new(AtomicBool::new(false));
    let supervise_thread = supervisor.as_ref().map(|sup| {
        let (sup, done) = (Arc::clone(sup), Arc::clone(&supervise_done));
        std::thread::spawn(move || loop {
            let mut waited = Duration::ZERO;
            while waited < SUPERVISE_EVERY {
                if done.load(Ordering::SeqCst) || live::stop_requested() != 0 {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
                waited += Duration::from_millis(100);
            }
            if sup.lock().unwrap_or_else(|e| e.into_inner()).tick() {
                live::request_rescan();
            }
        })
    });

    let mut last_sig = String::new();
    let mut router = refresh_router(
        f.args.target.as_deref(),
        f.serve,
        &stats,
        &mut last_sig,
        &tags(),
    );
    if !live_screen && !f.quiet {
        print_start_summary(&router, f.pool, f.detached, f.env);
    }

    // The id lives in memory first: the config file is only the cache between
    // runs, so a failed write must not turn every reconnect into `fresh=1`.
    // Pool syncs go to this id once the cloud has named it, never to the
    // (possibly stale) cache.
    let device_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let pool_chosen = Arc::new(AtomicBool::new(f.pool_chosen));
    let on_pool: live::PoolToggle = {
        let (parsed, env) = (f.parsed.clone(), f.env.clone());
        let pool_chosen = Arc::clone(&pool_chosen);
        let device_id = Arc::clone(&device_id);
        let prefs_path = state::paths(f.env);
        Box::new(move |on| {
            pool_chosen.store(true, Ordering::Relaxed);
            let mut prefs = state::load_prefs(&prefs_path);
            prefs.pool = Some(on);
            let _ = state::save_prefs(&prefs_path, &prefs);
            ulog(&format!(
                "pool turned {} (saved)",
                if on { "on" } else { "off" }
            ));
            let live = device_id.lock().unwrap_or_else(|e| e.into_inner()).clone();
            sync_pool_in_background(on, parsed.clone(), env.clone(), live);
        })
    };
    let ticker = live::Ticker::start(
        Arc::clone(&stats),
        meta.clone(),
        state::paths(f.env),
        live_screen,
        f.follow_pool_pref,
        on_pool,
    );
    let timing = Timing::from_env(f.env);

    // Pool is synced once per run: at connect when the device id is already
    // known, else when the cloud's `device` frame names it.
    let pool_synced = std::cell::Cell::new(false);
    let sync_pool_once = || {
        if !pool_synced.replace(true) {
            let live = device_id.lock().unwrap_or_else(|e| e.into_inner()).clone();
            sync_pool_in_background(
                stats.pool.load(Ordering::Relaxed),
                f.parsed.clone(),
                f.env.clone(),
                live,
            );
        }
    };
    let on_device = |id: &str| {
        *device_id.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.to_string());
        if cached_relay_device_id(f.env).as_deref() != Some(id) {
            if let Err(e) = write_profile_field(RELAY_DEVICE_ID_FIELD, id, f.env) {
                ulog(&format!("could not save this device's id: {e}"));
            }
        }
        sync_pool_once();
    };
    let mut backoff = Backoff::default();
    let mut orphans: Vec<String> = Vec::new();
    // Set while disconnected, for the "back online after" line.
    let mut offline_since: Option<std::time::Instant> = None;
    let mut last_err = String::new();
    let mut refused: Option<ConnectError> = None;
    loop {
        if live::stop_requested() != 0 {
            break;
        }
        // Rebuilt per attempt: the first connect's `device` frame turns
        // `fresh=1` into this device's own id for every reconnect.
        let known_id = device_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .or_else(|| cached_relay_device_id(f.env));
        let url = relay_url_with_name(ws_base, &f.args.device, known_id.as_deref());
        vlog(&format!("relay url={url}"));
        match connect_ws(&url, f.token) {
            Ok(mut ws) => {
                last_err.clear();
                stats.set_conn(Conn::Connected);
                stats.set_error(None);
                match offline_since.take() {
                    Some(t) => ulog(&format!(
                        "back online after {}",
                        view::duration(t.elapsed().as_secs())
                    )),
                    None => ulog(&format!("connected as \"{}\"", f.args.device)),
                }
                if known_id.is_some() {
                    sync_pool_once();
                }
                set_socket_timeouts(
                    &ws,
                    Duration::from_millis(SOCKET_POLL_MS),
                    timing.dead_after,
                );
                let conn = ConnState {
                    pool_chosen: Arc::clone(&pool_chosen),
                    timing,
                    orphans: std::mem::take(&mut orphans).into(),
                    ..ConnState::new(Arc::clone(&stats))
                };
                let up_since = std::time::Instant::now();
                let end =
                    serve_connection(&mut ws, &router, &conn, f.args.max_concurrency, &on_device);
                orphans = conn.orphans.take();
                orphans.extend(abort_in_flight(&conn.in_flight));
                drop(conn);
                stats.in_flight.store(0, Ordering::Relaxed);
                if up_since.elapsed() >= timing.stable_after {
                    backoff = Backoff::default();
                }
                match end {
                    ConnEnd::Stop => break,
                    ConnEnd::Rescan => {
                        stats.set_conn(Conn::Connecting);
                        router = refresh_router(
                            f.args.target.as_deref(),
                            f.serve,
                            &stats,
                            &mut last_sig,
                            &tags(),
                        );
                        continue;
                    }
                    ConnEnd::Dropped(why) => {
                        offline_since = Some(std::time::Instant::now());
                        ulog(&format!("disconnected: {why}"));
                    }
                }
            }
            Err(err @ (ConnectError::Refused(_) | ConnectError::DeviceLimit(_))) => {
                refused = Some(err);
                break;
            }
            Err(ConnectError::Transient(err)) => {
                offline_since.get_or_insert_with(std::time::Instant::now);
                if err != last_err {
                    ulog(&format!("relay connect failed: {err}"));
                    last_err = err.clone();
                }
                stats.set_error(Some(err));
            }
        }
        let delay = backoff.next(&timing);
        stats.set_conn(Conn::Reconnecting {
            attempt: backoff.attempt,
            retry_at: state::now_secs() + (delay.as_millis() as u64).div_ceil(1000),
        });
        ulog(&format!(
            "reconnecting in {:.1}s (attempt {})",
            delay.as_secs_f64(),
            backoff.attempt
        ));
        live::interruptible_sleep(delay);
        if live::stop_requested() != 0 {
            break;
        }
        let _ = live::take_rescan();
        stats.set_conn(Conn::Connecting);
        // Re-discover before every reconnect so endpoints started or stopped
        // since the last connect are picked up, and so no socket sits open
        // and unread while probes run.
        router = refresh_router(
            f.args.target.as_deref(),
            f.serve,
            &stats,
            &mut last_sig,
            &tags(),
        );
    }

    stats.set_conn(Conn::Stopping);
    ticker.finish();
    // Stop only what this relay started; external servers keep running.
    supervise_done.store(true, Ordering::SeqCst);
    if let Some(t) = supervise_thread {
        let _ = t.join();
    }
    if let Some(sup) = &supervisor {
        sup.lock().unwrap_or_else(|e| e.into_inner()).stop_all();
    }
    state::clear_if_owned(&f.paths, meta.pid);
    if let Some(err) = refused {
        return Err(match err {
            ConnectError::Refused(msg) => view::error(
                "the relay refused this credential",
                &msg,
                &format!("`{bin} login` for a fresh key, or `{bin} relay doctor`"),
            ),
            ConnectError::DeviceLimit(msg) => view::error(
                "this account already has the maximum number of devices",
                &msg,
                &format!(
                    "remove a device in the dashboard ({DEVICES_URL}) or \
                     `{bin} relay devices rm <id> --yes`, or start the relay with an existing `--name`"
                ),
            ),
            ConnectError::Transient(msg) => view::error(
                "the relay could not connect",
                &msg,
                &format!("`{bin} relay doctor`"),
            ),
        });
    }
    let snap = stats.snapshot(&meta, state::now_secs());
    if !f.quiet {
        let tokens = snap
            .tokens
            .map(|t| format!(", {} tokens", view::thousands(t)))
            .unwrap_or_default();
        eprintln!(
            "Relay stopped after {}. Served {} request(s){tokens}.",
            view::duration(snap.updated_at.saturating_sub(snap.started_at)),
            view::thousands(snap.requests)
        );
    }
    Ok(if live::stop_requested() == live::STOP_INT {
        130
    } else {
        0
    })
}

/// The one line that says whether this device donates to the shared pool.
fn pool_line(pool: bool) -> String {
    let bin = crate::help::invoked_bin();
    if pool {
        format!(
            "Pool: on. Donating idle capacity to the shared pool and earning credits. \
             Turn off: `{bin} relay pool off`. Learn more: {POOL_URL}"
        )
    } else {
        format!(
            "Pool: off. Only your own account uses this device. Turn on: `{bin} relay pool on`."
        )
    }
}

fn print_start_summary(
    router: &Router,
    pool: bool,
    detached: bool,
    env: &BTreeMap<String, String>,
) {
    let models = router.all_models();
    println!("AnyRouter relay");
    println!("  Account:   {}", load_active_profile_name(env));
    println!(
        "  Models:    {}",
        if models.is_empty() {
            "none detected".to_string()
        } else {
            models.join(", ")
        }
    );
    println!("  {}", pool_line(pool));
    println!("  Dashboard: {DEVICES_URL}");
    if !detached {
        println!(
            "  Stop with Ctrl-C. Run it in the background: `{} relay start --detach`",
            crate::help::invoked_bin()
        );
    }
}

/// Apply the pool setting to this device without blocking the socket loop.
/// Failure never stops the relay: own-device serving still works.
fn sync_pool_in_background(
    pool: bool,
    parsed: crate::parse::ParsedArgs,
    env: BTreeMap<String, String>,
    live_device_id: Option<String>,
) {
    std::thread::spawn(move || {
        let result = sync_pool(pool, &parsed, &env, live_device_id.as_deref())
            .map_err(|e| e.trim_start_matches("error: ").to_string());
        match (result, pool) {
            (Ok(()), _) => vlog(&format!("pool_enabled={pool} saved for this device")),
            (Err(err), true) => {
                ulog(&format!("could not turn on pool sharing: {err}"));
                ulog(&format!(
                    "still serving your own requests. Manage it at {DEVICES_URL}"
                ));
            }
            (Err(err), false) => {
                ulog(&format!("could not confirm pool sharing is off: {err}"));
                ulog(&format!("check this device at {DEVICES_URL}"));
            }
        }
    });
}

/// The pool sync itself: the id the cloud bound this connection to wins; the
/// cached id is only a fallback before the first `device` frame. The config
/// may not have been writable, so the cache can be stale — PATCHing it would
/// flip pool sharing on the wrong device.
fn sync_pool(
    pool: bool,
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
    live_device_id: Option<&str>,
) -> Result<(), String> {
    let id = match live_device_id {
        Some(id) => id.to_string(),
        None => resolve_relay_device_id(parsed, env)?,
    };
    set_relay_pool(&id, pool, parsed, env)
}

/// `anyr relay pair [--name <device>]` — explicit pairing (start auto-pairs).
fn run_relay_pair(
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let bin = crate::help::invoked_bin();
    let flag =
        crate::parse::get_string_flag(&parsed.flags, "name").filter(|s| !s.trim().is_empty());
    let name = resolve_device(flag.as_deref(), parsed, env, false);
    let Some(api_key) = resolve_sk_ar_key(parsed, env) else {
        return Err(view::error(
            "not logged in",
            "pairing needs an AnyRouter key",
            &format!("`{bin} login` (or just `{bin} relay start`, which logs in and pairs)"),
        ));
    };
    let _ = pair_device(&api_key, &name, env)?;
    println!("Paired as \"{name}\". Token saved to your AnyRouter config.");
    println!("Next: `{bin} relay start`");
    Ok(0)
}

/// The name saved in the active profile, if any.
fn saved_device_name(env: &BTreeMap<String, String>) -> Option<String> {
    let path = crate::config::resolve_config_path(None, env);
    let cfg = crate::key::load_config_if_present(&path)?;
    cfg.profiles
        .get(&cfg.active_profile)?
        .relay_device_name
        .clone()
        .filter(|s| !s.trim().is_empty())
}

/// The device name for this run: `--name`, else the saved name, else the
/// machine's own, else a generated one. The result is saved on first use, so
/// resolving again gives the same name even if the Mac is renamed. A new
/// `--name` replaces the saved one here and, best effort, in the cloud.
fn resolve_device(
    flag: Option<&str>,
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
    quiet: bool,
) -> String {
    let saved = saved_device_name(env);
    let host = env
        .get("HOSTNAME")
        .or_else(|| env.get("COMPUTERNAME"))
        .map(String::as_str);
    let (name, source) = devname::resolve(
        &devname::SystemRunner,
        &devname::Inputs {
            flag,
            saved: saved.as_deref(),
            macos: cfg!(target_os = "macos"),
            env_hostname: host,
            seed: devname::seed_now(),
        },
    );
    if saved.as_deref() != Some(name.as_str()) {
        if let Err(e) = write_profile_field(RELAY_DEVICE_NAME_FIELD, &name, env) {
            eprintln!("Could not save the device name: {e}");
        } else if let (Some(old), devname::Source::Flag) = (&saved, source) {
            if *old != name {
                if let Err(e) = rename_on_cloud(parsed, env, &name) {
                    eprintln!("Saved the name here; the cloud still shows \"{old}\" ({e})");
                }
            }
        }
    }
    if !quiet {
        if let Some(line) = devname::first_start_line(&name, source, &crate::help::invoked_bin()) {
            println!("{line}");
        }
    }
    name
}

/// PATCH /relay/devices/:id { name } for the cached (or only) paired device.
fn rename_on_cloud(
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
    name: &str,
) -> Result<(), String> {
    let id = resolve_relay_device_id(parsed, env)?;
    let key = resolve_sk_ar_key(parsed, env)
        .ok_or_else(|| "no AnyRouter key to rename the device".to_string())?;
    patch_device_name(DEFAULT_API_BASE, &key, &id, name)
}

fn patch_device_name(api_base: &str, key: &str, id: &str, name: &str) -> Result<(), String> {
    let url = format!("{api_base}/relay/devices/{id}");
    let body = json!({ "name": name }).to_string();
    let (status, resp) = crate::http::http_patch(&url, Some(key), Some(&body))?;
    if (200..300).contains(&status) {
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_str(&resp).unwrap_or(serde_json::Value::Null);
    let msg = value
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap_or("unexpected response");
    Err(format!("Could not rename the device ({status}): {msg}"))
}

/// Cloud first, then the config: a refused rename leaves the config alone.
fn rename_device(
    api_base: &str,
    key: &str,
    id: &str,
    name: &str,
    env: &BTreeMap<String, String>,
) -> Result<(), String> {
    patch_device_name(api_base, key, id, name)?;
    write_profile_field(RELAY_DEVICE_NAME_FIELD, name, env)
}

/// `anyr relay rename <name>` — cloud first, so the config never claims a
/// name the dashboard does not show. A running relay keeps the old name on
/// its status screen until it restarts.
fn run_relay_rename(
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
    name: &str,
) -> Result<i32, String> {
    let bin = crate::help::invoked_bin();
    let name = devname::clean(name);
    if name.is_empty() {
        return Err(view::error(
            "the device name is empty",
            "",
            &format!("`{bin} relay rename \"Studio\"`"),
        ));
    }
    if resolve_sk_ar_key(parsed, env).is_none() {
        return Err(view::error(
            "not logged in",
            "renaming needs an AnyRouter key",
            &format!("`{bin} login`"),
        ));
    }
    let id = resolve_relay_device_id(parsed, env)?;
    let key = resolve_sk_ar_key(parsed, env).unwrap_or_default();
    rename_device(DEFAULT_API_BASE, &key, &id, &name, env)?;
    println!("Device renamed to \"{name}\".");
    if matches!(state::running(&state::paths(env)), state::Running::Yes(_)) {
        println!("Restart the relay to show it on the status screen: `{bin} relay stop && {bin} relay start`");
    }
    Ok(0)
}

/// Where the relay credential comes from, without any network call.
fn credential_kind(
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Option<String> {
    let profile = load_active_profile_name(env);
    let set = |k: &str| env.get(k).is_some_and(|v| !v.trim().is_empty());
    if crate::parse::get_string_flag(&parsed.flags, "token").is_some() {
        return Some("relay token (--token)".into());
    }
    if set(RELAY_TOKEN_ENV_VAR) {
        return Some(format!("relay token ({RELAY_TOKEN_ENV_VAR})"));
    }
    if read_stored_relay_token(env).is_some_and(|t| !t.trim().is_empty()) {
        return Some(format!("relay token (profile {profile})"));
    }
    if set("ANYROUTER_API_KEY") {
        return Some("AnyRouter key (ANYROUTER_API_KEY)".into());
    }
    resolve_sk_ar_key(parsed, env).map(|_| format!("AnyRouter key (profile {profile})"))
}

/// `anyr relay` and `anyr relay status [--json]`. Reads the state file only:
/// no probes, so it answers instantly.
fn run_relay_status(
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
    json_out: bool,
) -> Result<i32, String> {
    let bin = crate::help::invoked_bin();
    let paths = state::paths(env);
    let running = state::running(&paths);
    let prefs = state::load_prefs(&paths);
    let pool = state::effective_pool(None, &prefs);
    let credential = credential_kind(parsed, env);
    let (snap, unresponsive) = match &running {
        state::Running::Yes(s) => (Some(s), None),
        state::Running::Unresponsive(pid) => (None, Some(*pid)),
        state::Running::No => (None, None),
    };
    let next = view::next_step(&bin, credential.is_some(), snap, unresponsive.is_some());
    if json_out {
        let v = json!({
            "running": snap.is_some() || unresponsive.is_some(),
            "responding": snap.is_some(),
            "pid": snap.map(|s| s.pid).or(unresponsive),
            "relay": snap.map(state::Snapshot::to_value),
            "credential": credential,
            "pool": match snap {
                Some(s) => json!({ "enabled": s.pool, "source": "running" }),
                None => json!({ "enabled": pool.0, "source": pool.1.name() }),
            },
            "models": prefs.models,
            "next": next.replace('`', ""),
        });
        println!("{v:#}");
        return Ok(0);
    }
    let p = Paint(crate::term::color_enabled());
    match snap {
        Some(s) => print!("{}", view::render_running(s, state::now_secs(), p)),
        None => print!(
            "{}",
            view::render_idle(
                &view::Idle {
                    credential: credential.as_deref(),
                    pool,
                    models: prefs.models.as_deref(),
                    unresponsive_pid: unresponsive,
                },
                p,
            )
        ),
    }
    print!("{}", view::render_next(&next, p));
    Ok(0)
}

/// Every model `relay` can serve on this machine, with its saved toggle.
fn model_rows(prefs: &state::Prefs) -> Vec<view::ModelRow> {
    let mut rows: Vec<view::ModelRow> = detect_all(&SystemDetector)
        .into_iter()
        .map(|d| view::ModelRow {
            id: d.model.id().to_string(),
            source: match d.model {
                OnDeviceModel::AppleFoundation if d.unavailable.is_none() => {
                    let ids = fetch_local_models(FM_SERVE_TARGET);
                    if ids.is_empty() {
                        d.model.label().to_string()
                    } else {
                        format!("{} (fm serve: {})", d.model.label(), ids.join(", "))
                    }
                }
                _ => d.model.label().to_string(),
            },
            unavailable: d.unavailable,
            enabled: prefs.models.as_ref().is_none_or(|m| {
                m.iter()
                    .any(|id| OnDeviceModel::from_id(id) == Some(d.model))
            }),
            toggleable: true,
        })
        .collect();
    for (label, base, health) in CHAT_PROBES.iter() {
        if !http_probe_listening(health) {
            continue;
        }
        for id in fetch_local_models(base) {
            rows.push(view::ModelRow {
                id,
                source: label.to_string(),
                unavailable: None,
                enabled: true,
                toggleable: false,
            });
        }
    }
    rows
}

fn run_relay_models(env: &BTreeMap<String, String>, json_out: bool) -> Result<i32, String> {
    let bin = crate::help::invoked_bin();
    let prefs = state::load_prefs(&state::paths(env));
    let rows = model_rows(&prefs);
    if json_out {
        let v = json!({
            "models": rows.iter().map(|r| json!({
                "id": r.id,
                "source": r.source,
                "available": r.unavailable.is_none(),
                "reason": r.unavailable,
                "enabled": r.enabled,
                "toggleable": r.toggleable,
            })).collect::<Vec<_>>(),
        });
        println!("{v:#}");
        return Ok(0);
    }
    let p = Paint(crate::term::color_enabled());
    print!("{}", view::render_models(&rows, p));
    print!(
        "{}",
        view::render_next(
            &format!("`{bin} relay models disable <id>` or `{bin} relay models enable <id>`"),
            p
        )
    );
    Ok(0)
}

fn run_relay_model_set(
    env: &BTreeMap<String, String>,
    id: &str,
    enable: bool,
) -> Result<i32, String> {
    let bin = crate::help::invoked_bin();
    let Some(model) = OnDeviceModel::from_id(id) else {
        let known: Vec<&str> = OnDeviceModel::ALL.iter().map(|m| m.id()).collect();
        return Err(crate::cmd::dispatch::hint(&format!(
            "Unknown on-device model \"{id}\" (known: {}).\nOther local servers are always served; \
             stop them to stop serving their models.",
            known.join(", ")
        )));
    };
    let paths = state::paths(env);
    let mut prefs = state::load_prefs(&paths);
    let mut list: Vec<String> = prefs
        .models
        .clone()
        .unwrap_or_else(|| {
            OnDeviceModel::ALL
                .iter()
                .map(|m| m.id().to_string())
                .collect()
        })
        .into_iter()
        .filter(|m| OnDeviceModel::from_id(m) != Some(model))
        .collect();
    if enable {
        list.push(model.id().to_string());
    }
    list.sort();
    prefs.models = Some(list);
    state::save_prefs(&paths, &prefs)?;
    println!(
        "{} {}.",
        if enable { "Enabled" } else { "Disabled" },
        model.id()
    );
    if matches!(state::running(&paths), state::Running::Yes(_)) {
        println!("Applies on restart: `{bin} relay stop && {bin} relay start --detach`");
    } else {
        println!("Applies on the next `{bin} relay start`.");
    }
    Ok(0)
}

fn run_relay_pool(
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
    set: Option<bool>,
    json_out: bool,
) -> Result<i32, String> {
    let bin = crate::help::invoked_bin();
    let paths = state::paths(env);
    let mut prefs = state::load_prefs(&paths);
    let Some(on) = set else {
        let (on, src) = state::effective_pool(None, &prefs);
        if json_out {
            println!("{:#}", json!({ "enabled": on, "source": src.name() }));
        } else {
            println!("Pool {}  ({})", view::pool_text(on), src.name());
            println!("Learn more: {POOL_URL}");
            let p = Paint(crate::term::color_enabled());
            print!(
                "{}",
                view::render_next(
                    &format!("`{bin} relay pool {}`", if on { "off" } else { "on" }),
                    p
                )
            );
        }
        return Ok(0);
    };
    prefs.pool = Some(on);
    state::save_prefs(&paths, &prefs)?;
    let remote =
        resolve_relay_device_id(parsed, env).and_then(|id| set_relay_pool(&id, on, parsed, env));
    if json_out {
        println!(
            "{:#}",
            json!({
                "enabled": on,
                "source": "saved",
                "device_updated": remote.is_ok(),
                "error": remote.as_ref().err(),
            })
        );
        return Ok(0);
    }
    println!("Pool {}.", view::pool_text(on));
    match remote {
        Ok(()) => println!("Saved, and this device was updated."),
        Err(e) => {
            println!("Saved; applies on the next `{bin} relay start`.");
            eprintln!(
                "note: the paired device was not updated now: {}",
                e.trim_start_matches("error: ").replace('\n', "\n  ")
            );
        }
    }
    Ok(0)
}

/// Remove the stored pairing token and device id from the active profile.
fn clear_relay_fields(env: &BTreeMap<String, String>) -> Result<(), String> {
    let path = crate::config::resolve_config_path(None, env);
    let Some(mut cfg) = crate::key::load_config_if_present(&path) else {
        return Ok(());
    };
    let active = cfg.active_profile.clone();
    if let Some(entry) = cfg.profiles.get_mut(&active) {
        entry.relay_token = None;
        entry.relay_device_id = None;
    }
    crate::config::write_config(&cfg, &path)
}

fn run_relay_unpair(
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
    yes: bool,
) -> Result<i32, String> {
    let bin = crate::help::invoked_bin();
    let paths = state::paths(env);
    if !matches!(state::running(&paths), state::Running::No) {
        return Err(view::error(
            "the relay is running",
            "unpairing a live device would drop its connection mid-request",
            &format!("`{bin} relay stop` first"),
        ));
    }
    if !yes {
        if !crate::term::is_interactive() {
            return Err(crate::cmd::dispatch::hint(
                "`relay unpair` asks for confirmation; pass --yes when not at a terminal.",
            ));
        }
        if !crate::term::confirm("Remove this device from your AnyRouter account?") {
            eprintln!("Cancelled.");
            return Ok(1);
        }
    }
    let mut warning = None;
    match resolve_relay_device_id(parsed, env) {
        Ok(id) => match resolve_sk_ar_key(parsed, env) {
            Some(key) => {
                let url = format!("{DEFAULT_API_BASE}/relay/devices/{id}");
                match crate::http::http_delete(&url, Some(&key)) {
                    Ok((s, _)) if (200..300).contains(&s) || s == 404 => {}
                    Ok((s, _)) => warning = Some(format!("the cloud answered {s}")),
                    Err(e) => warning = Some(e),
                }
            }
            None => warning = Some("no AnyRouter key to remove it from your account".into()),
        },
        Err(e) => warning = Some(e),
    }
    clear_relay_fields(env)?;
    match warning {
        None => println!("Unpaired: removed this device from your account and forgot its token."),
        Some(w) => {
            println!("Forgot the pairing on this machine.");
            let why = w.lines().next().unwrap_or_default();
            eprintln!(
                "note: nothing removed from your account ({}). Check {DEVICES_URL}",
                why.trim_start_matches("error: ")
            );
        }
    }
    println!("Next: `{bin} relay start` pairs again.");
    Ok(0)
}

/// The real probes behind `relay doctor`.
struct SystemProbe<'a> {
    parsed: &'a crate::parse::ParsedArgs,
    env: &'a BTreeMap<String, String>,
}

impl doctor::Probe for SystemProbe<'_> {
    fn is_mac(&self) -> bool {
        cfg!(target_os = "macos")
    }
    fn macos_major(&self) -> Option<u32> {
        macos_major_version()
    }
    fn fm_serve(&self) -> Option<Vec<String>> {
        http_probe_listening(FM_SERVE_HEALTH_URL).then(|| fetch_local_models(FM_SERVE_TARGET))
    }
    fn in_process_foundation(&self) -> bool {
        foundation::foundation_model_available()
    }
    fn decider(&self) -> bool {
        probe_decider(DECIDER_TARGET)
    }
    fn installed(&self, model: OnDeviceModel) -> bool {
        servers::Host::installed(&servers::SystemHost::default(), model.spec())
    }
    fn other_servers(&self) -> Vec<String> {
        CHAT_PROBES
            .iter()
            .filter(|(_, _, health)| http_probe_listening(health))
            .map(|(label, base, _)| {
                let models = fetch_local_models(base);
                if models.is_empty() {
                    format!("{label} ({base}): no models listed")
                } else {
                    let mut names = models.join(", ");
                    if names.chars().count() > 120 {
                        names = format!(
                            "{}… (+{} more)",
                            names.chars().take(120).collect::<String>(),
                            models.len().saturating_sub(3)
                        );
                    }
                    format!("{label} ({base}): {} models — {names}", models.len())
                }
            })
            .collect()
    }
    fn network(&self) -> Result<(), String> {
        match ureq::get(&format!("{DEFAULT_API_BASE}/health"))
            .timeout(Duration::from_secs(5))
            .call()
        {
            Ok(_) | Err(ureq::Error::Status(_, _)) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
    fn auth(&self) -> doctor::Auth {
        let Some(key) = resolve_sk_ar_key(self.parsed, self.env) else {
            return match credential_kind(self.parsed, self.env) {
                Some(_) => doctor::Auth::Token,
                None => doctor::Auth::None,
            };
        };
        match crate::http::http_get(&format!("{DEFAULT_API_BASE}/relay/devices"), Some(&key)) {
            Ok((s, _)) if (200..300).contains(&s) => doctor::Auth::KeyOk,
            Ok((s @ (401 | 403), _)) => doctor::Auth::KeyRejected(s),
            Ok((s, _)) => doctor::Auth::Unverified(format!("HTTP {s}")),
            Err(e) => doctor::Auth::Unverified(e),
        }
    }
    fn relay_pid(&self) -> Option<u32> {
        match state::running(&state::paths(self.env)) {
            state::Running::Yes(s) => Some(s.pid),
            state::Running::Unresponsive(pid) => Some(pid),
            state::Running::No => None,
        }
    }
}

fn run_relay_doctor(
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
    json_out: bool,
) -> Result<i32, String> {
    let bin = crate::help::invoked_bin();
    let checks = doctor::run_checks(&SystemProbe { parsed, env }, &bin);
    let code = i32::from(doctor::failed(&checks));
    if json_out {
        println!("{:#}", doctor::to_json(&checks));
        return Ok(code);
    }
    let p = Paint(crate::term::color_enabled());
    println!("{}", p.bold("AnyRouter relay doctor"));
    print!("{}", doctor::render(&checks, p));
    if code == 0 {
        println!("\nAll required checks passed.");
    } else {
        println!("\nSome checks failed. Run each fix above, then `{bin} relay doctor` again.");
    }
    Ok(code)
}

/// `anyr relay [subcommand]` dispatcher.
pub fn run(
    parsed: &crate::parse::ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let cmd = cli::parse_cmd(parsed)?;
    let paths = state::paths(env);
    let bin = crate::help::invoked_bin();
    match cmd {
        cli::Cmd::Overview { json } | cli::Cmd::Status { json } => {
            run_relay_status(parsed, env, json)
        }
        cli::Cmd::Start { detach } => run_relay_start(parsed, env, detach),
        cli::Cmd::Stop => daemon::stop(&paths, &bin),
        cli::Cmd::Logs { follow, lines } => daemon::logs(&paths, follow, lines, &bin),
        cli::Cmd::Models { json } => run_relay_models(env, json),
        cli::Cmd::DeviceModels => devices::run_models(parsed, env),
        cli::Cmd::Devices(d) => devices::run(parsed, env, d),
        cli::Cmd::ModelSet { id, enable } => run_relay_model_set(env, &id, enable),
        cli::Cmd::Pool { set, json } => run_relay_pool(parsed, env, set, json),
        cli::Cmd::Pair => run_relay_pair(parsed, env),
        cli::Cmd::Rename { name } => run_relay_rename(parsed, env, &name),
        cli::Cmd::Unpair { yes } => run_relay_unpair(parsed, env, yes),
        cli::Cmd::Doctor { json } => run_relay_doctor(parsed, env, json),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[test]
    fn poll_timeout_covers_unix_and_windows_read_timeouts() {
        assert!(is_poll_timeout(std::io::ErrorKind::WouldBlock));
        assert!(is_poll_timeout(std::io::ErrorKind::TimedOut));
        assert!(is_poll_timeout(std::io::ErrorKind::Interrupted));
        assert!(!is_poll_timeout(std::io::ErrorKind::ConnectionReset));
    }

    #[test]
    fn dial_gives_up_within_the_timeout_on_a_blackholed_host() {
        // 192.0.2.0/24 (TEST-NET-1) is never routed: without a connect
        // timeout the reconnect loop would hang for minutes here.
        let t = std::time::Instant::now();
        let res = super::dial_with_timeout("192.0.2.1", 9, Duration::from_millis(300));
        assert!(res.is_err());
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "dial ignored its timeout"
        );
    }

    #[test]
    fn dial_connects_to_a_live_listener() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(super::dial_with_timeout("127.0.0.1", port, Duration::from_secs(2)).is_ok());
    }

    use super::*;

    fn request_frame(raw: &str) -> RequestFrame {
        match parse_server_frame(raw).expect("should parse") {
            ServerFrame::Request(f) => f,
            other => panic!("should be a request, got {other:?}"),
        }
    }

    #[test]
    fn parses_request_frame_with_all_fields() {
        let f = request_frame(
            r#"{"type":"request","id":"abc","path":"/chat/completions","body":"{\"model\":\"m\"}","stream":true}"#,
        );
        assert_eq!(f.id, "abc");
        assert_eq!(f.path, "/chat/completions");
        assert_eq!(f.body, r#"{\"model\":\"m\"}"#.replace('\\', ""));
    }

    #[test]
    fn parses_request_without_optional_fields_as_empty() {
        let f = request_frame(r#"{"type":"request","id":"x"}"#);
        assert_eq!(f.id, "x");
        assert_eq!(f.path, "");
        assert_eq!(f.body, "");
    }

    #[test]
    fn cancel_frame_maps_to_err_with_id() {
        let parsed = parse_server_frame(r#"{"type":"cancel","id":"req-7"}"#).unwrap();
        assert_eq!(parsed, ServerFrame::Cancel("req-7".into()));
    }

    #[test]
    fn device_frame_names_the_device_this_connection_is_bound_to() {
        assert_eq!(
            parse_server_frame(r#"{"type":"device","id":"dev-1","name":"Desk"}"#),
            Some(ServerFrame::Device {
                id: "dev-1".into(),
                name: "Desk".into()
            })
        );
        // No id, no binding: never cache an empty device id.
        assert!(parse_server_frame(r#"{"type":"device","id":" "}"#).is_none());
    }

    #[test]
    fn malformed_and_unknown_frames_are_ignored() {
        assert!(parse_server_frame("not json").is_none());
        assert!(parse_server_frame("{}").is_none());
        assert!(parse_server_frame(r#"{"type":"request"}"#).is_none()); // no id
        assert!(parse_server_frame(r#"{"id":"a"}"#).is_none()); // no type
        assert!(parse_server_frame(r#"{"type":"future-frame","id":"a"}"#).is_none());
        assert!(parse_server_frame("[1,2]").is_none());
    }

    #[test]
    fn head_frame_uses_camel_case_content_type() {
        let json = ClientFrame::Head {
            id: "r1".into(),
            status: 200,
            content_type: "text/event-stream".into(),
        }
        .to_json();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "head");
        assert_eq!(v["id"], "r1");
        assert_eq!(v["status"], 200);
        assert_eq!(v["contentType"], "text/event-stream");
    }

    #[test]
    fn hello_frame_includes_max_concurrency_only_when_set() {
        let without = ClientFrame::Hello {
            models: vec!["foundation-model".into()],
            max_concurrency: None,
            pool: None,
        }
        .to_json();
        let v: serde_json::Value = serde_json::from_str(&without).unwrap();
        assert_eq!(v["type"], "hello");
        assert_eq!(v["models"][0], "foundation-model");
        assert!(v.get("maxConcurrency").is_none());

        let with = ClientFrame::Hello {
            models: vec![],
            max_concurrency: Some(3),
            pool: None,
        }
        .to_json();
        let v: serde_json::Value = serde_json::from_str(&with).unwrap();
        assert_eq!(v["maxConcurrency"], 3);
    }

    #[test]
    fn hello_frame_states_a_chosen_pool_and_omits_the_default() {
        // --no-pool must reach the cloud with the model list, not in a later
        // PATCH that can fail and leave an old pool_enabled=true in place.
        for pool in [false, true] {
            let json = ClientFrame::Hello {
                models: vec!["foundation-model".into()],
                max_concurrency: None,
                pool: Some(pool),
            }
            .to_json();
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(v["pool"], pool);
        }
        // The unchosen default leaves the server value alone on reconnect.
        let json = ClientFrame::Hello {
            models: vec![],
            max_concurrency: None,
            pool: None,
        }
        .to_json();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("pool").is_none());
    }

    #[test]
    fn done_and_error_frames_are_minimal_envelopes() {
        let done: serde_json::Value =
            serde_json::from_str(&ClientFrame::Done { id: "d1".into() }.to_json()).unwrap();
        assert_eq!(done["type"], "done");
        assert_eq!(done["id"], "d1");

        let err: serde_json::Value = serde_json::from_str(
            &ClientFrame::Error {
                id: "e1".into(),
                message: "boom".into(),
            }
            .to_json(),
        )
        .unwrap();
        assert_eq!(err["type"], "error");
        assert_eq!(err["message"], "boom");
    }

    #[test]
    fn fm_serve_targets_advertise_the_pool_join_key() {
        // Even when /models returns nothing usable, the executor's upstream
        // model_name must be present or pool routing can't match (#1128).
        let models = advertised_models(FM_SERVE_TARGET, &[]);
        assert_eq!(models, vec!["foundation-model"]);

        let fetched = vec!["_base".into()];
        let models = advertised_models(FM_SERVE_TARGET, &fetched);
        assert_eq!(models, vec!["_base", "foundation-model"]);
    }

    #[test]
    fn non_fm_targets_keep_local_model_list_verbatim() {
        let fetched = vec!["llama3".into(), "qwen2".into()];
        let models = advertised_modules_placeholder(&fetched);
        assert_eq!(models, fetched);

        // No duplicate when fm serve itself already lists the id.
        let fetched = vec!["foundation-model".into()];
        let models = advertised_models("http://localhost:1976/v1", &fetched);
        assert_eq!(models, vec!["foundation-model"]);
    }

    fn advertised_modules_placeholder(fetched: &[String]) -> Vec<String> {
        advertised_models("http://10.0.0.5:8000/v1", fetched)
    }

    #[test]
    fn short_id_is_char_boundary_safe_and_bounded() {
        assert_eq!(short_id("abcdefghijklmnop", 8), "abcdefgh");
        // Multibyte inside the first 8 BYTES must not panic.
        assert_eq!(short_id("héllo-world", 4), "héll");
        assert_eq!(short_id("🚀🚀🚀", 2), "🚀🚀");
        assert_eq!(short_id("", 8), "");
    }

    #[test]
    fn utf8_flush_len_never_splits_a_codepoint() {
        // "héllo" where é is 2 bytes: split inside é keeps the partial byte.
        let bytes = "héllo".as_bytes();
        assert_eq!(utf8_flush_len(&bytes[..1]), 1); // 'h' complete, é split
        assert_eq!(utf8_flush_len(bytes), 6); // whole string

        // 4-byte emoji split mid-sequence.
        let rocket = "🚀x";
        let rb = rocket.as_bytes();
        assert_eq!(utf8_flush_len(&rb[..2]), 0);
        assert_eq!(utf8_flush_len(rb), 5);

        // ASCII flushes everything.
        assert_eq!(utf8_flush_len(b"plain"), 5);
    }

    #[test]
    fn utf8_flush_len_flushes_past_an_invalid_byte() {
        // 0xFF is never valid UTF-8; the text after it must not stall.
        assert_eq!(utf8_flush_len(b"a\xffbc"), 4);
        assert_eq!(utf8_flush_len(b"\xffhello"), 6);
        // Invalid byte followed by a truncated codepoint keeps only the tail.
        let mut v = b"a\xff".to_vec();
        v.extend_from_slice(&"é".as_bytes()[..1]);
        assert_eq!(utf8_flush_len(&v), 2);
        assert_eq!(
            String::from_utf8_lossy(&b"a\xffbc"[..utf8_flush_len(b"a\xffbc")]),
            "a\u{FFFD}bc"
        );
    }

    #[test]
    fn time_scale_is_ignored_when_scaling_is_disallowed() {
        let env = BTreeMap::from([("ANYR_RELAY_TIME_SCALE".to_string(), "0.01".to_string())]);
        let release = Timing::from_env_with(&env, false);
        assert_eq!(release.ping_every, Timing::default().ping_every);
        assert_eq!(release.dead_after, Timing::default().dead_after);
        let debug = Timing::from_env_with(&env, true);
        assert!(debug.ping_every < Timing::default().ping_every);
    }

    #[test]
    fn start_args_read_flags_with_defaults() {
        let mk = |flags: Vec<(&str, crate::parse::FlagValue)>| crate::parse::ParsedArgs {
            command: "relay".into(),
            flags: flags.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            passthrough: vec!["start".into()],
            raw_from: None,
        };
        use crate::parse::FlagValue;

        let bare = mk(vec![("verbose", FlagValue::Bool(true))]);
        let args = parse_start_args(&bare);
        assert!(args.target.is_none());
        assert!(args.name.is_none());
        assert!(args.models.is_none());
        assert!(!args.all);
        assert!(args.max_concurrency.is_none());

        let full = mk(vec![
            (
                "target",
                FlagValue::Value("http://127.0.0.1:1976/v1".into()),
            ),
            ("token", FlagValue::Value("rk_x".into())),
            ("url", FlagValue::Value("wss://example.test/relay".into())),
            ("name", FlagValue::Value("Desk".into())),
            ("pool", FlagValue::Bool(true)),
            ("max-concurrency", FlagValue::Value("4".into())),
        ]);
        let args = parse_start_args(&full);
        assert_eq!(args.target.as_deref(), Some("http://127.0.0.1:1976/v1"));
        assert_eq!(args.token.as_deref(), Some("rk_x"));
        assert_eq!(args.name.as_deref(), Some("Desk"));
        assert_eq!(args.max_concurrency, Some(4));

        // --pool / --no-pool live in cli::pool_flag (tested there).
        let both = mk(vec![
            (
                "model",
                FlagValue::Value("apple/foundation-model,foundation-model".into()),
            ),
            ("all", FlagValue::Bool(true)),
        ]);
        let args = parse_start_args(&both);
        assert_eq!(
            args.models.as_deref(),
            Some("apple/foundation-model,foundation-model")
        );
        assert!(args.all);

        // Non-positive / non-numeric max-concurrency is ignored, not an error.
        let bad = mk(vec![("max-concurrency", FlagValue::Value("0".into()))]);
        assert!(parse_start_args(&bad).max_concurrency.is_none());
    }

    /// Fake detector: the listed models are reachable, the rest are not.
    struct FakeDetector(Vec<OnDeviceModel>);
    impl OnDeviceDetector for FakeDetector {
        fn detect(&self, model: OnDeviceModel) -> Result<(), String> {
            if self.0.contains(&model) {
                Ok(())
            } else {
                Err("not running".into())
            }
        }
    }

    fn set(models: &[OnDeviceModel]) -> BTreeSet<OnDeviceModel> {
        models.iter().copied().collect()
    }

    use OnDeviceModel::{AppleFoundation as Fm, DeciderFlash as Dec};

    #[test]
    fn model_choice_parses_ids_aliases_and_rejects_bad_input() {
        assert_eq!(
            parse_model_choice(None, false).unwrap(),
            ModelChoice::Default
        );
        assert_eq!(parse_model_choice(None, true).unwrap(), ModelChoice::All);
        // Catalog id, local id and case variants all resolve; duplicates fold.
        assert_eq!(
            parse_model_choice(Some("apple/foundation-model,FOUNDATION-MODEL"), false).unwrap(),
            ModelChoice::Only(vec![Fm])
        );
        assert_eq!(
            parse_model_choice(
                Some("pplx-decider-flash-v1.2, apple/foundation-model"),
                false
            )
            .unwrap(),
            ModelChoice::Only(vec![Dec, Fm])
        );
        let err = parse_model_choice(Some("gpt-5"), false).unwrap_err();
        assert!(
            err.contains("gpt-5") && err.contains("apple/foundation-model"),
            "{err}"
        );
        assert!(parse_model_choice(Some("foundation-model"), true).is_err());
    }

    #[test]
    fn detection_reports_why_a_model_is_unavailable() {
        let d = detect_all(&FakeDetector(vec![Dec]));
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].model, Fm);
        assert_eq!(d[0].unavailable.as_deref(), Some("not running"));
        assert_eq!(d[1].model, Dec);
        assert!(d[1].unavailable.is_none());
    }

    #[test]
    fn picker_sees_only_available_models_and_its_answer_wins() {
        let detected = detect_all(&FakeDetector(vec![Fm]));
        let seen = Mutex::new(Vec::new());
        let pick = |avail: &[OnDeviceModel]| {
            seen.lock().unwrap().extend_from_slice(avail);
            Ok(Vec::new())
        };
        let got = select_models(&ModelChoice::Default, &detected, Some(&pick)).unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![Fm]);
        // User unchecked everything: serve no on-device model.
        assert!(got.is_empty());
    }

    #[test]
    fn without_a_tty_every_on_device_model_is_served() {
        // No picker (stdin not a TTY): serve all, so an app started later
        // is picked up on reconnect with no restart.
        let detected = detect_all(&FakeDetector(vec![Dec]));
        assert_eq!(
            select_models(&ModelChoice::Default, &detected, None).unwrap(),
            set(&[Fm, Dec])
        );
    }

    #[test]
    fn picker_is_skipped_when_nothing_is_available_or_flags_decide() {
        let none = detect_all(&FakeDetector(vec![]));
        let boom = |_: &[OnDeviceModel]| -> Result<Vec<OnDeviceModel>, String> {
            panic!("picker must not run")
        };
        assert_eq!(
            select_models(&ModelChoice::Default, &none, Some(&boom)).unwrap(),
            set(&[Fm, Dec])
        );
        let both = detect_all(&FakeDetector(vec![Fm, Dec]));
        assert_eq!(
            select_models(&ModelChoice::Only(vec![Dec]), &both, Some(&boom)).unwrap(),
            set(&[Dec])
        );
        assert_eq!(
            select_models(&ModelChoice::All, &none, Some(&boom)).unwrap(),
            set(&[Fm, Dec])
        );
    }

    #[test]
    fn picker_cancel_stops_start() {
        let both = detect_all(&FakeDetector(vec![Fm, Dec]));
        let cancel = |_: &[OnDeviceModel]| Err("Cancelled.".to_string());
        assert!(select_models(&ModelChoice::Default, &both, Some(&cancel)).is_err());
    }

    #[test]
    fn unselected_on_device_endpoints_are_dropped_others_kept() {
        let targets = vec![
            Target {
                label: "fm".into(),
                base: FM_SERVE_TARGET.into(),
                kind: TargetKind::Chat,
                models: vec!["foundation-model".into()],
                fm_models: None,
            },
            decider_target(DECIDER_TARGET),
            Target {
                label: "Ollama".into(),
                base: OLLAMA_TARGET.into(),
                kind: TargetKind::Chat,
                models: vec!["llama3".into()],
                fm_models: None,
            },
        ];
        let kept: Vec<String> = keep_selected(targets.clone(), &set(&[Dec]))
            .into_iter()
            .map(|t| t.label)
            .collect();
        assert_eq!(kept, vec!["Perplexity Decider", "Ollama"]);
        assert_eq!(keep_selected(targets, &set(&[Fm, Dec])).len(), 3);
    }

    #[test]
    fn pool_line_names_the_opt_out_and_the_pool_page() {
        let on = pool_line(true);
        assert!(
            on.contains("Donating") && on.contains("relay pool off") && on.contains(POOL_URL),
            "{on}"
        );
        let off = pool_line(false);
        assert!(off.starts_with("Pool: off"), "{off}");
    }

    #[test]
    fn device_login_base_strips_v1_suffix() {
        assert_eq!(device_login_base(), "https://anyrouter.dev/api");
    }

    #[test]
    fn config_round_trips_relay_fields() {
        let src = "\
active_profile: default
profiles:
  default:
    api_key: sk-ar-v1-real
    relay_token: rk_pairing-token
    relay_device_id: dev_123
";
        let cfg = crate::config::parse_config(src);
        let p = cfg.profiles.get("default").unwrap();
        assert_eq!(p.relay_token.as_deref(), Some("rk_pairing-token"));
        assert_eq!(p.relay_device_id.as_deref(), Some("dev_123"));

        let again = crate::config::parse_config(&crate::config::serialize_config(&cfg));
        let p = again.profiles.get("default").unwrap();
        assert_eq!(p.relay_token.as_deref(), Some("rk_pairing-token"));
        assert_eq!(p.relay_device_id.as_deref(), Some("dev_123"));
    }

    /// One-shot HTTP stub that records the request line and body it receives.
    fn spawn_capture_stub(
        status: u16,
        resp: &str,
    ) -> (String, std::thread::JoinHandle<(String, String)>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let resp = resp.to_string();
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let n = s.read(&mut tmp).unwrap();
                buf.extend_from_slice(&tmp[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let want = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if body.len() >= want {
                        let line = head.lines().next().unwrap_or("").to_string();
                        let out = format!(
                            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp}",
                            resp.len()
                        );
                        let _ = s.write_all(out.as_bytes());
                        return (line, body.to_string());
                    }
                }
            }
        });
        (base, h)
    }

    #[test]
    fn rename_patches_the_device_name_in_the_cloud() {
        let (base, h) = spawn_capture_stub(200, r#"{"id":"dev_1","name":"Studio"}"#);
        patch_device_name(&base, "sk-ar-v1-k", "dev_1", "Studio").unwrap();
        let (line, body) = h.join().unwrap();
        assert!(line.starts_with("PATCH /relay/devices/dev_1 "), "{line}");
        assert_eq!(body, r#"{"name":"Studio"}"#);
    }

    #[test]
    fn rename_surfaces_a_cloud_refusal() {
        let (base, h) = spawn_capture_stub(403, r#"{"error":{"message":"nope"}}"#);
        let err = patch_device_name(&base, "sk-ar-v1-k", "dev_1", "Studio").unwrap_err();
        let _ = h.join();
        assert!(err.contains("403") && err.contains("nope"), "{err}");
    }

    #[test]
    fn rename_updates_config_and_server_together() {
        let home = temp_home("rename-ok");
        let env = env_with(&home, &[]);
        write_profile_field(RELAY_DEVICE_NAME_FIELD, "Old", &env).unwrap();
        let (base, h) = spawn_capture_stub(200, r#"{"id":"dev_1","name":"New"}"#);
        rename_device(&base, "sk-ar-v1-k", "dev_1", "New", &env).unwrap();
        let (_, body) = h.join().unwrap();
        assert_eq!(body, r#"{"name":"New"}"#);
        assert_eq!(saved_device_name(&env).as_deref(), Some("New"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn refused_rename_leaves_the_config_alone() {
        let home = temp_home("rename-refused");
        let env = env_with(&home, &[]);
        write_profile_field(RELAY_DEVICE_NAME_FIELD, "Old", &env).unwrap();
        let (base, h) = spawn_capture_stub(403, r#"{"error":{"message":"nope"}}"#);
        assert!(rename_device(&base, "sk-ar-v1-k", "dev_1", "New", &env).is_err());
        let _ = h.join();
        assert_eq!(saved_device_name(&env).as_deref(), Some("Old"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn device_name_is_saved_once_and_kept_when_resolved_again() {
        let home = temp_home("name-persist");
        let env = env_with(&home, &[("HOSTNAME", "")]);
        let p = parsed_start(vec![]);
        let first = resolve_device(Some("Desk"), &p, &env, true);
        assert_eq!(first, "Desk");
        assert_eq!(saved_device_name(&env).as_deref(), Some("Desk"));
        // No flag: the saved name wins, and saving again changes nothing.
        let again = resolve_device(None, &p, &env, true);
        let third = resolve_device(None, &p, &env, true);
        assert_eq!((again.as_str(), third.as_str()), ("Desk", "Desk"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn config_round_trips_the_device_name() {
        let src = "active_profile: default\nprofiles:\n  default:\n    relay_device_name: swift-otter-42\n";
        let cfg = crate::config::parse_config(src);
        let again = crate::config::parse_config(&crate::config::serialize_config(&cfg));
        assert_eq!(
            again.profiles["default"].relay_device_name.as_deref(),
            Some("swift-otter-42")
        );
    }

    fn spawn_stub_target(responses: Vec<(u16, String)>) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("http://{addr}");
        let handle = std::thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(_) => return,
                };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                loop {
                    match stream.read(&mut tmp) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                let header_end = pos + 4;
                                let headers = String::from_utf8_lossy(&buf[..header_end]);
                                let want = headers
                                    .lines()
                                    .find_map(|line| {
                                        let (k, v) = line.split_once(':')?;
                                        (k.eq_ignore_ascii_case("content-length"))
                                            .then(|| v.trim().parse::<usize>().ok())
                                            .flatten()
                                    })
                                    .unwrap_or(0);
                                while buf.len() - header_end < want {
                                    match stream.read(&mut tmp) {
                                        Ok(0) => break,
                                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                        Err(_) => break,
                                    }
                                }
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let reason = match status {
                    200 => "OK",
                    502 => "Bad Gateway",
                    _ => "OK",
                };
                let bytes = body.as_bytes();
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(bytes);
                let _ = stream.flush();
            }
        });
        (url, handle)
    }

    fn req(id: &str, path: &str, body: &str) -> RequestFrame {
        RequestFrame {
            id: id.into(),
            path: path.into(),
            body: body.into(),
        }
    }

    fn drain(rx: &mpsc::Receiver<ClientFrame>) -> Vec<ClientFrame> {
        let mut out = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            out.push(frame);
        }
        out
    }

    fn chunk_text(frames: &[ClientFrame]) -> String {
        frames
            .iter()
            .filter_map(|f| match f {
                ClientFrame::Chunk { data, .. } => Some(data.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn streams_local_response_back_as_head_chunk_done() {
        let (target, join) = spawn_stub_target(vec![(200, "hello world".into())]);
        let (tx, rx) = mpsc::sync_channel(OUTBOUND_QUEUE);
        handle_request(
            &tx,
            &req("r1", "/v1/chat/completions", "{}"),
            &target,
            &AtomicBool::new(false),
        );
        let frames = drain(&rx);
        let _ = join.join();
        assert!(
            matches!(
                frames.as_slice(),
                [
                    ClientFrame::Head {
                        id,
                        status: 200,
                        ..
                    },
                    ClientFrame::Chunk { data, .. },
                    ClientFrame::Done { .. }
                ] if id == "r1" && data == "hello world"
            ),
            "expected Head(200)+Chunk+Done, got {} frames",
            frames.len()
        );
    }

    #[test]
    fn non_2xx_from_target_relays_status_and_body() {
        let (target, join) = spawn_stub_target(vec![(502, "bad gateway".into())]);
        let (tx, rx) = mpsc::sync_channel(OUTBOUND_QUEUE);
        handle_request(
            &tx,
            &req("r2", "/v1/chat/completions", "{}"),
            &target,
            &AtomicBool::new(false),
        );
        let frames = drain(&rx);
        let _ = join.join();
        assert!(
            matches!(
                frames.as_slice(),
                [
                    ClientFrame::Head { status: 502, .. },
                    ClientFrame::Chunk { data, .. },
                    ClientFrame::Done { .. }
                ] if data == "bad gateway"
            ),
            "expected Head(502)+Chunk+Done, got {} frames",
            frames.len()
        );
    }

    #[test]
    fn unreachable_target_produces_error_frame() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let target = format!("http://{addr}");
        let (tx, rx) = mpsc::sync_channel(OUTBOUND_QUEUE);
        handle_request(
            &tx,
            &req("r3", "/v1/chat/completions", "{}"),
            &target,
            &AtomicBool::new(false),
        );
        let frames = drain(&rx);
        assert_eq!(frames.len(), 1, "expected a single Error frame");
        match &frames[0] {
            ClientFrame::Error { id, message } => {
                assert_eq!(id, "r3");
                assert!(message.contains("Local target unreachable"), "{message}");
            }
            _ => panic!("expected Error frame"),
        }
    }

    #[test]
    fn cancel_flag_stops_streaming() {
        let body = "x".repeat(1024 * 1024);
        let (target, join) = spawn_stub_target(vec![(200, body)]);
        let (tx, rx) = mpsc::sync_channel(OUTBOUND_QUEUE);
        handle_request(
            &tx,
            &req("r4", "/v1/chat/completions", "{}"),
            &target,
            &AtomicBool::new(true),
        );
        let frames = drain(&rx);
        let _ = join.join();
        let extra = frames
            .iter()
            .any(|f| matches!(f, ClientFrame::Chunk { .. } | ClientFrame::Done { .. }));
        assert!(
            !extra,
            "cancel before handle_request must not send Chunk or Done (got {} frames)",
            frames.len()
        );
    }

    #[test]
    fn utf8_split_body_round_trips() {
        let expected = "héllo 🚀".repeat(1000);
        let (target, join) = spawn_stub_target(vec![(200, expected.clone())]);
        let (tx, rx) = mpsc::sync_channel(OUTBOUND_QUEUE);
        handle_request(
            &tx,
            &req("r5", "/v1/chat/completions", "{}"),
            &target,
            &AtomicBool::new(false),
        );
        let frames = drain(&rx);
        let _ = join.join();
        assert!(
            matches!(frames.first(), Some(ClientFrame::Head { status: 200, .. })),
            "missing Head(200)"
        );
        assert!(
            matches!(frames.last(), Some(ClientFrame::Done { .. })),
            "missing Done"
        );
        assert_eq!(chunk_text(&frames), expected);
    }

    // ---- credential + connect behavior (one-key path) ----

    fn parsed_start(flags: Vec<(&str, crate::parse::FlagValue)>) -> crate::parse::ParsedArgs {
        crate::parse::ParsedArgs {
            command: "relay".into(),
            flags: flags.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            passthrough: vec!["start".into()],
            raw_from: None,
        }
    }

    fn env_with(home: &std::path::Path, extra: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert("ANYROUTER_HOME".into(), home.display().to_string());
        for (k, v) in extra {
            env.insert((*k).into(), (*v).into());
        }
        env
    }

    fn temp_home(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("anyr-relay-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sk_ar_key_alone_is_the_connect_credential_without_pairing() {
        // The scoped-key bug: start used to POST /relay/devices first and died
        // with "Pairing failed (403)". With only an sk-ar key the bearer is the
        // key itself; nothing is minted or written to the config.
        let home = temp_home("one-key");
        let env = env_with(&home, &[("ANYROUTER_API_KEY", "sk-ar-v1-onlykey")]);
        let cred = resolve_relay_credential(None, &parsed_start(vec![]), &env).unwrap();
        assert_eq!(cred, "sk-ar-v1-onlykey");
        assert!(
            !home.join("config.yaml").exists(),
            "resolving the credential must not write relay_token/device id"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn legacy_rk_tokens_still_win_over_the_sk_ar_key() {
        let home = temp_home("legacy");
        std::fs::write(
            home.join("config.yaml"),
            "active_profile: default\nprofiles:\n  default:\n    api_key: sk-ar-v1-cfg\n    relay_token: rk_stored\n",
        )
        .unwrap();
        let parsed = parsed_start(vec![]);

        let env = env_with(&home, &[("ANYROUTER_API_KEY", "sk-ar-v1-env")]);
        assert_eq!(
            resolve_relay_credential(None, &parsed, &env).unwrap(),
            "rk_stored"
        );

        let env = env_with(
            &home,
            &[
                ("ANYROUTER_RELAY_TOKEN", "rk_env"),
                ("ANYROUTER_API_KEY", "sk-ar-v1-env"),
            ],
        );
        assert_eq!(
            resolve_relay_credential(None, &parsed, &env).unwrap(),
            "rk_env"
        );
        assert_eq!(
            resolve_relay_credential(Some(" rk_flag "), &parsed, &env).unwrap(),
            "rk_flag"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn relay_url_gets_an_encoded_device_name_and_never_a_key() {
        assert_eq!(
            relay_url_with_name("wss://x.test/api/v1/relay/connect", "My Mac", None),
            "wss://x.test/api/v1/relay/connect?name=My%20Mac&fresh=1"
        );
        assert_eq!(
            relay_url_with_name("wss://x.test/c", "Büro&x", None),
            "wss://x.test/c?name=B%C3%BCro%26x&fresh=1"
        );
        // An explicit --url query is the caller's; leave it alone.
        assert_eq!(
            relay_url_with_name("ws://127.0.0.1:1/c?name=Fixed", "My Mac", Some("d1")),
            "ws://127.0.0.1:1/c?name=Fixed"
        );
    }

    #[test]
    fn relay_url_reconnects_as_its_own_device_once_the_id_is_known() {
        // Without this, two machines on one key were bound to the same device
        // and kicked each other off every few seconds.
        assert_eq!(
            relay_url_with_name("wss://x.test/c", "Desk", Some("6f1c-ab")),
            "wss://x.test/c?name=Desk&device=6f1c-ab"
        );
    }

    type MockCloud = (
        String,
        Arc<Mutex<Vec<(String, String)>>>,
        std::thread::JoinHandle<()>,
    );

    /// One-shot mock cloud: accepts a single WebSocket, records the handshake
    /// (path+query, Authorization), then runs `script` on the live socket.
    fn spawn_mock_cloud(
        refuse_with: Option<u16>,
        script: impl FnOnce(&mut tungstenite::WebSocket<std::net::TcpStream>) + Send + 'static,
    ) -> MockCloud {
        use tungstenite::handshake::server::{ErrorResponse, Request, Response};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "ws://{}/api/v1/relay/connect",
            listener.local_addr().unwrap()
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_in = Arc::clone(&seen);
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            #[allow(clippy::result_large_err)]
            let callback = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
                let auth = req
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                seen_in.lock().unwrap().push((req.uri().to_string(), auth));
                match refuse_with {
                    Some(code) => {
                        let mut r = ErrorResponse::new(Some("nope".into()));
                        *r.status_mut() = tungstenite::http::StatusCode::from_u16(code).unwrap();
                        Err(r)
                    }
                    None => Ok(resp),
                }
            };
            if let Ok(mut ws) = tungstenite::accept_hdr(stream, callback) {
                script(&mut ws);
            }
        });
        (url, seen, handle)
    }

    #[test]
    fn connect_sends_the_key_in_the_header_and_the_name_in_the_query_only() {
        let (url, seen, join) = spawn_mock_cloud(None, |ws| {
            let _ = ws.close(None);
            let _ = ws.flush();
        });
        let url = relay_url_with_name(&url, "Desk", None);
        assert!(connect_ws(&url, "sk-ar-v1-secret").is_ok());
        let _ = join.join();
        let seen = seen.lock().unwrap();
        let (uri, auth) = &seen[0];
        assert_eq!(auth, "Bearer sk-ar-v1-secret");
        assert!(
            uri.ends_with("/api/v1/relay/connect?name=Desk&fresh=1"),
            "{uri}"
        );
        assert!(!uri.contains("sk-ar"), "key leaked into the URL: {uri}");
    }

    #[test]
    fn a_401_or_403_handshake_stops_the_relay_instead_of_retrying() {
        for code in [401u16, 403] {
            let (url, _seen, join) = spawn_mock_cloud(Some(code), |_| {});
            match connect_ws(&url, "sk-ar-v1-scoped") {
                Err(ConnectError::Refused(msg)) => {
                    assert!(msg.contains(&code.to_string()), "{msg}");
                    assert!(!msg.contains("sk-ar"), "message must not echo the key");
                }
                other => panic!("expected Refused for {code}, got {other:?}"),
            }
            let _ = join.join();
        }
        // Every device slot taken: stop, and pass the cloud's message on. The
        // device limit is not a credential problem, so it must not come back as
        // Refused (which prints the login hint).
        let (url, _seen, join) = spawn_mock_cloud(Some(409), |_| {});
        match connect_ws(&url, "sk-ar-v1-k") {
            Err(ConnectError::DeviceLimit(msg)) => {
                assert!(msg.contains("device slot"), "{msg}")
            }
            other => panic!("expected DeviceLimit for 409, got {other:?}"),
        }
        let _ = join.join();
        assert_eq!(
            device_limit_message(Some(
                br#"{"error":{"code":"relay_device_limit","message":"Device limit reached (5)."}}"#
            )),
            "Device limit reached (5)."
        );
        // 5xx is the cloud's problem, not the credential's: keep retrying.
        let (url, _seen, join) = spawn_mock_cloud(Some(500), |_| {});
        assert!(matches!(
            connect_ws(&url, "sk-ar-v1-k"),
            Err(ConnectError::Transient(_))
        ));
        let _ = join.join();
    }

    /// A one-shot HTTP mock that records the request line and body and replies
    /// `200 {}`. Returns the base URL (no trailing slash) and a handle for the
    /// recorded (request line, body), in receive order.
    fn spawn_http_mock() -> (String, std::thread::JoinHandle<Vec<(String, String)>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/api", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Read, Write};
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request_line = line.trim().to_string();
            let mut content_length = 0_usize;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header == "\r\n" || header.is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':') {
                    if name.trim().eq_ignore_ascii_case("content-length") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0_u8; content_length];
            reader.read_exact(&mut body).unwrap();
            let body = String::from_utf8_lossy(&body).into_owned();
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
            vec![(request_line, body)]
        });
        (base, handle)
    }

    #[test]
    fn pool_sync_patches_the_live_device_id_not_the_stale_cache() {
        // #198(a): after the first `device` frame the in-memory id wins; the
        // cached id can be stale when the config was unwritable.
        let home = temp_home("pool-live-id");
        std::fs::write(
            home.join("config.yaml"),
            "active_profile: default\nprofiles:\n  default:\n    relay_device_id: dev-stale\n",
        )
        .unwrap();
        let (base, handle) = spawn_http_mock();
        let env = env_with(&home, &[("ANYROUTER_API_KEY", "sk-ar-v1-pooltest8888")]);
        let parsed = parsed_start(vec![("base-url", crate::parse::FlagValue::Value(base))]);

        // Live id known: PATCH must go there, not to the cached dev-stale.
        sync_pool(true, &parsed, &env, Some("dev-live")).expect("sync pool on live id");
        let seen = handle.join().unwrap();
        assert_eq!(seen.len(), 1);
        let (line, body) = &seen[0];
        assert!(line.contains("/api/v1/relay/devices/dev-live"), "{line}");
        assert!(body.contains("\"pool_enabled\":true"), "{body}");

        // Before the first device frame: fall back to the cached id.
        let (base, handle) = spawn_http_mock();
        let parsed = parsed_start(vec![("base-url", crate::parse::FlagValue::Value(base))]);
        sync_pool(false, &parsed, &env, None).expect("sync pool from cache");
        let seen = handle.join().unwrap();
        let (line, body) = &seen[0];
        assert!(line.contains("/api/v1/relay/devices/dev-stale"), "{line}");
        assert!(body.contains("\"pool_enabled\":false"), "{body}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn mock_cloud_round_trip_returns_the_model_text() {
        // Whole device path with no network beyond loopback: cloud sends a
        // `request` frame down, the relay forwards it to a stub "fm serve",
        // and head/chunk/done come back up carrying the completion text.
        let completion =
            r#"{"choices":[{"message":{"role":"assistant","content":"Hello from the Mac"}}]}"#;
        let (local, local_join) = spawn_stub_target(vec![
            (200, r#"{"data":[{"id":"_base"}]}"#.into()), // GET /models
            (200, completion.into()),                     // POST /chat/completions
        ]);
        let router = Router::new(vec![chat_target("stub", &local)]);

        let trace: Arc<Mutex<Vec<String>>> = Arc::default();
        let cloud_trace = Arc::clone(&trace);
        let (url, _seen, cloud_join) = spawn_mock_cloud(None, move |ws| {
            // A read that never completes must fail the test, not hang CI (#109).
            let _ = ws.get_ref().set_read_timeout(Some(Duration::from_secs(20)));
            let note = |s: String| cloud_trace.lock().unwrap().push(s);
            note("cloud: waiting for hello".into());
            let first = ws.read().unwrap();
            let hello: serde_json::Value = serde_json::from_str(first.to_text().unwrap()).unwrap();
            note(format!("cloud: got {}", first.to_text().unwrap()));
            assert_eq!(hello["type"], "hello");
            assert_eq!(hello["models"][0], "_base");
            let body =
                r#"{"model":"foundation-model","messages":[{"role":"user","content":"hi"}]}"#;
            ws.send(tungstenite::Message::text(
                serde_json::json!({
                    "type": "request", "id": "req-1", "path": "/chat/completions",
                    "body": body, "stream": false,
                })
                .to_string(),
            ))
            .unwrap();
            let mut types = Vec::new();
            let mut text = String::new();
            loop {
                let msg = ws.read().unwrap();
                note(format!(
                    "cloud: got {}",
                    msg.to_text().unwrap_or("<binary>")
                ));
                let v: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
                assert_eq!(v["id"], "req-1");
                let ty = v["type"].as_str().unwrap().to_string();
                if ty == "head" {
                    assert_eq!(v["status"], 200);
                }
                if ty == "chunk" {
                    text.push_str(v["data"].as_str().unwrap());
                }
                types.push(ty.clone());
                if ty == "done" || ty == "error" {
                    break;
                }
            }
            assert_eq!(types.first().map(String::as_str), Some("head"));
            assert_eq!(types.last().map(String::as_str), Some("done"));
            assert!(text.contains("Hello from the Mac"), "{text}");
            let _ = ws.close(None);
            let _ = ws.flush();
        });

        let mut ws = connect_ws(&url, "sk-ar-v1-test").expect("connect to mock cloud");
        set_socket_timeouts(&ws, Duration::from_millis(20), Duration::from_secs(30));
        let state = ConnState::new(Arc::default());
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let end = serve_connection(&mut ws, &router, &state, None, &|_| {});
            let _ = done_tx.send(end);
        });
        let end = done_rx
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| {
                panic!("relay did not finish in 60s: {:#?}", trace.lock().unwrap())
            });
        assert!(
            matches!(end, ConnEnd::Dropped(_)),
            "{:#?}",
            trace.lock().unwrap()
        );
        cloud_join
            .join()
            .unwrap_or_else(|_| panic!("mock cloud assertions: {:#?}", trace.lock().unwrap()));
        let _ = local_join.join();
    }

    fn t(label: &str, kind: TargetKind, models: &[&str]) -> Target {
        Target {
            label: label.into(),
            base: format!("http://{label}/v1"),
            kind,
            models: models.iter().map(|m| m.to_string()).collect(),
            fm_models: None,
        }
    }

    fn sample_router() -> Router {
        Router::new(vec![
            t("fm", TargetKind::Chat, &["foundation-model"]),
            t("ollama", TargetKind::Chat, &["llama3", "foundation-model"]),
            t(
                "decider",
                TargetKind::Decider,
                &[DECIDER_ADVERTISED_MODEL_ID],
            ),
        ])
    }

    #[test]
    fn model_map_is_the_union_and_first_target_wins_duplicates() {
        let r = sample_router();
        assert_eq!(
            r.all_models(),
            vec!["foundation-model", "llama3", DECIDER_ADVERTISED_MODEL_ID]
        );
        assert_eq!(r.by_model["foundation-model"], 0);
        assert_eq!(r.by_model["llama3"], 1);
    }

    #[test]
    fn routes_each_request_to_the_owning_target() {
        let r = sample_router();
        let chat = r
            .route("/chat/completions", r#"{"model":"llama3"}"#)
            .unwrap();
        assert_eq!(chat.label, "ollama");
        let dec = r
            .route(
                "/decisions",
                &format!(r#"{{"model":"{DECIDER_ADVERTISED_MODEL_ID}"}}"#),
            )
            .unwrap();
        assert_eq!(dec.label, "decider");
    }

    #[test]
    fn unknown_model_is_rejected_with_the_available_list() {
        let err = sample_router()
            .route("/chat/completions", r#"{"model":"nope"}"#)
            .unwrap_err();
        assert!(err.contains("\"nope\""), "{err}");
        assert!(err.contains("llama3"), "{err}");
        assert!(sample_router().route("/chat/completions", "{}").is_err());
    }

    #[test]
    fn path_and_target_kind_must_match() {
        let r = sample_router();
        let err = r
            .route(
                "/chat/completions",
                &format!(r#"{{"model":"{DECIDER_ADVERTISED_MODEL_ID}"}}"#),
            )
            .unwrap_err();
        assert!(err.contains("decider"), "{err}");
        // Two deciders: model matching applies, and a chat model is refused.
        let r2 = Router::new(vec![
            t("d1", TargetKind::Decider, &["dec-a"]),
            t("d2", TargetKind::Decider, &["dec-b"]),
            t("c", TargetKind::Chat, &["llama3"]),
        ]);
        let err = r2.route("/decisions", r#"{"model":"llama3"}"#).unwrap_err();
        assert!(err.contains("chat"), "{err}");
    }

    #[test]
    fn a_single_chat_endpoint_accepts_an_unlisted_model() {
        let r = Router::new(vec![t("only", TargetKind::Chat, &["llama3"])]);
        assert_eq!(
            r.route("/chat/completions", r#"{"model":"foundation-model"}"#)
                .unwrap()
                .label,
            "only"
        );
        assert_eq!(r.route("/chat/completions", "{}").unwrap().label, "only");
    }

    #[test]
    fn routing_failure_sends_an_error_frame() {
        let r = sample_router();
        let (url, _seen, cloud_join) = spawn_mock_cloud(None, |ws| {
            let _hello = ws.read().unwrap();
            ws.send(tungstenite::Message::text(
                serde_json::json!({
                    "type": "request", "id": "bad-1", "path": "/chat/completions",
                    "body": r#"{"model":"nope"}"#, "stream": false,
                })
                .to_string(),
            ))
            .unwrap();
            let msg = ws.read().unwrap();
            let v: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            assert_eq!(v["type"], "error");
            assert_eq!(v["id"], "bad-1");
            assert!(v["message"].as_str().unwrap().contains("nope"));
            let _ = ws.close(None);
            let _ = ws.flush();
        });
        let mut ws = connect_ws(&url, "sk-ar-v1-test").expect("connect");
        set_socket_timeouts(&ws, Duration::from_millis(20), Duration::from_secs(30));
        let state = ConnState::new(Arc::default());
        serve_connection(&mut ws, &r, &state, None, &|_| {});
        cloud_join.join().expect("mock cloud assertions");
    }

    #[test]
    fn serve_connection_sends_pool_in_hello_and_reports_the_bound_device() {
        let r = Router::new(vec![t("only", TargetKind::Chat, &["m"])]);
        let (url, _seen, cloud_join) = spawn_mock_cloud(None, |ws| {
            let hello: serde_json::Value =
                serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(hello["type"], "hello");
            assert_eq!(hello["pool"], false);
            ws.send(tungstenite::Message::Text(
                r#"{"type":"device","id":"dev-42","name":"Desk"}"#.into(),
            ))
            .unwrap();
            std::thread::sleep(Duration::from_millis(200));
            let _ = ws.close(None);
            let _ = ws.flush();
        });
        let mut ws = connect_ws(&url, "sk-ar-v1-test").expect("connect");
        set_socket_timeouts(&ws, Duration::from_millis(20), Duration::from_secs(30));
        // --no-pool: pool chosen and off.
        let state = ConnState {
            pool_chosen: Arc::new(AtomicBool::new(true)),
            ..ConnState::new(Arc::default())
        };
        let bound = Mutex::new(Vec::new());
        serve_connection(&mut ws, &r, &state, None, &|id| {
            bound.lock().unwrap().push(id.to_string())
        });
        cloud_join.join().expect("mock cloud assertions");
        assert_eq!(*bound.lock().unwrap(), vec!["dev-42".to_string()]);
    }

    #[test]
    fn lone_target_without_a_model_list_takes_any_chat_request() {
        let r = Router::new(vec![t("only", TargetKind::Chat, &[])]);
        assert_eq!(
            r.route("/chat/completions", r#"{"model":"x"}"#)
                .unwrap()
                .label,
            "only"
        );
        assert!(r.route("/decisions", r#"{"model":"x"}"#).is_err());
    }

    #[test]
    fn detached_child_args_follow_saved_pool_and_never_carry_the_credential() {
        let parsed = parsed_start(vec![
            ("token", crate::parse::FlagValue::Value("rk_secret".into())),
            ("name", crate::parse::FlagValue::Value("Desk".into())),
        ]);
        let mut args = parse_start_args(&parsed);
        args.device = args.name.clone().unwrap();
        let argv = detached_child_args(&args, None, false);
        assert_eq!(argv, vec!["relay", "start", "--name", "Desk"]);
        assert!(!argv
            .iter()
            .any(|a| a.contains("rk_") || a.contains("sk-ar")));
        assert!(detached_child_args(&args, Some(false), true).ends_with(&["--verbose".to_string()]));
        assert!(detached_child_args(&args, Some(false), false).contains(&"--no-pool".to_string()));
    }

    #[test]
    fn no_launch_is_parsed_and_forwarded_to_the_detached_child() {
        // WHY: --detach re-execs the relay; without forwarding, the
        // background copy would launch servers the user said not to.
        let on = parse_start_args(&parsed_start(vec![(
            "no-launch",
            crate::parse::FlagValue::Bool(true),
        )]));
        assert!(on.no_launch);
        assert!(!launches_servers(&on));
        assert!(detached_child_args(&on, None, false).contains(&"--no-launch".to_string()));
        let off = parse_start_args(&parsed_start(vec![]));
        assert!(launches_servers(&off));
        assert!(!detached_child_args(&off, None, false).contains(&"--no-launch".to_string()));
        let parsed = crate::parse::parse_cli_args(["relay", "start", "--no-launch"]).unwrap();
        assert!(parse_start_args(&parsed).no_launch);
    }

    #[test]
    fn explicit_target_never_launches_servers() {
        let args = parse_start_args(&parsed_start(vec![(
            "target",
            crate::parse::FlagValue::Value("http://localhost:9000/v1".into()),
        )]));
        assert!(!launches_servers(&args));
    }

    #[test]
    fn installed_but_stopped_server_is_offered_when_start_may_launch_it() {
        let down = || Err::<(), String>("open the Decider for Mac app (serves on :8080)".into());
        let m = OnDeviceModel::DeciderFlash;
        assert_eq!(with_launch(m, down(), true, || true), Ok(()));
        assert_eq!(
            with_launch(m, down(), false, || true),
            down(),
            "--no-launch"
        );
        assert_eq!(
            with_launch(m, down(), true, || false),
            down(),
            "not installed"
        );
        assert_eq!(with_launch(m, Ok(()), true, || unreachable!()), Ok(()));
    }

    #[test]
    fn endpoints_line_marks_managed_and_external_servers() {
        let router = Router::new(vec![
            Target {
                label: "fm serve (Apple Foundation Models)".into(),
                base: FM_SERVE_TARGET.into(),
                kind: TargetKind::Chat,
                models: vec!["foundation-model".into()],
                fm_models: None,
            },
            decider_target(DECIDER_TARGET),
            Target {
                label: "Ollama".into(),
                base: OLLAMA_TARGET.into(),
                kind: TargetKind::Chat,
                models: vec!["llama3".into()],
                fm_models: None,
            },
        ]);
        let tags: BTreeMap<&str, &str> =
            [(FM_SERVE_TARGET, "managed"), (DECIDER_TARGET, "external")].into();
        let rows = endpoint_rows(&router, &tags);
        assert_eq!(rows[0].0, "fm serve (Apple Foundation Models) (managed)");
        assert_eq!(rows[1].0, "Perplexity Decider (external)");
        assert_eq!(rows[2].0, "Ollama");
    }

    #[test]
    fn fm_serve_model_mapping() {
        let fm = vec!["system".to_string(), "pcc".to_string()];
        // The owner's failing call: catalog names must reach fm as `system`.
        assert_eq!(
            map_fm_model("foundation-model", &fm).unwrap().as_deref(),
            Some("system")
        );
        assert_eq!(
            map_fm_model("apple/foundation-model", &fm)
                .unwrap()
                .as_deref(),
            Some("system")
        );
        // Already a real id, or an unknown list: pass through untouched.
        assert_eq!(map_fm_model("pcc", &fm).unwrap(), None);
        assert_eq!(map_fm_model("foundation-model", &[]).unwrap(), None);
        // One local model takes any name.
        assert_eq!(
            map_fm_model("whatever", &["base".to_string()])
                .unwrap()
                .as_deref(),
            Some("base")
        );
        let err = map_fm_model("gpt-5", &fm).unwrap_err();
        assert!(
            err.contains("gpt-5") && err.contains("system, pcc"),
            "{err}"
        );
    }

    /// Fake fm serve: GET /models lists [system, pcc]; POST answers with the
    /// model id it received, so the test sees what the relay forwarded.
    fn spawn_fake_fm_serve() -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let (head, body) = loop {
                    let n = stream.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        break (String::new(), String::new());
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(pos) = text.find("\r\n\r\n") {
                        let head = text[..pos].to_string();
                        let want = head
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if text.len() - pos - 4 >= want {
                            break (head, text[pos + 4..].to_string());
                        }
                    }
                };
                let reply = if head.starts_with("GET") {
                    r#"{"data":[{"id":"system"},{"id":"pcc"}]}"#.to_string()
                } else {
                    let model = serde_json::from_str::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|v| v["model"].as_str().map(str::to_string))
                        .unwrap_or_default();
                    json!({ "model": model }).to_string()
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        url
    }

    #[test]
    fn requests_reach_a_fake_fm_serve_under_its_real_model_id() {
        let fm = spawn_fake_fm_serve();
        let fetched = fetch_local_models(&fm);
        assert_eq!(fetched, vec!["system", "pcc"]);
        let target = Target {
            label: "fm serve".into(),
            base: fm.clone(),
            kind: TargetKind::Chat,
            models: advertised_models(FM_SERVE_TARGET, &fetched),
            fm_models: Some(fetched),
        };
        // Two chat targets so routing must match by model, as in the field.
        let router = Router::new(vec![target, t("ollama", TargetKind::Chat, &["llama3"])]);
        let (url, _seen, cloud_join) = spawn_mock_cloud(None, |ws| {
            let _hello = ws.read().unwrap();
            let send =
                |ws: &mut tungstenite::WebSocket<std::net::TcpStream>, id: &str, model: &str| {
                    ws.send(tungstenite::Message::text(
                        json!({
                            "type": "request", "id": id, "path": "/chat/completions",
                            "body": json!({"model": model, "messages": []}).to_string(),
                        })
                        .to_string(),
                    ))
                    .unwrap();
                };
            send(ws, "ok-1", "foundation-model");
            let mut text = String::new();
            loop {
                let v: serde_json::Value =
                    serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
                match v["type"].as_str().unwrap() {
                    "chunk" => text.push_str(v["data"].as_str().unwrap()),
                    "done" => break,
                    "error" => panic!("unexpected error: {v}"),
                    _ => {}
                }
            }
            assert!(text.contains(r#""model":"system""#), "{text}");
            // A name fm serve does not have and that routes to it: the
            // pplx-style unknown id is refused by routing with the list.
            send(ws, "bad-1", "pcc-unknown");
            let v: serde_json::Value =
                serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(v["type"], "error");
            let _ = ws.close(None);
            let _ = ws.flush();
        });
        let mut ws = connect_ws(&url, "sk-ar-v1-test").expect("connect");
        set_socket_timeouts(&ws, Duration::from_millis(20), Duration::from_secs(30));
        let state = ConnState::new(Arc::default());
        serve_connection(&mut ws, &router, &state, None, &|_| {});
        cloud_join.join().expect("mock cloud assertions");
    }

    #[test]
    fn a_lone_fm_serve_target_returns_the_available_ids_for_an_unknown_model() {
        let target = Target {
            label: "fm serve".into(),
            base: "http://127.0.0.1:1/v1".into(),
            kind: TargetKind::Chat,
            models: vec![],
            fm_models: Some(vec!["system".into(), "pcc".into()]),
        };
        let err = body_for_target(&target, r#"{"model":"gpt-5"}"#).unwrap_err();
        assert!(err.contains("available: system, pcc"), "{err}");
        let body = body_for_target(&target, r#"{"model":"foundation-model","x":1}"#).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["model"], "system");
        assert_eq!(v["x"], 1, "other fields survive the rewrite");
    }

    #[test]
    fn decider_probe_recognizes_only_a_decisions_reply() {
        assert!(is_decider_response(
            r#"{"answers":{"q":{"choice":"a","probabilities":{"a":0.9},"confidence":0.9}}}"#
        ));
        assert!(!is_decider_response(r#"{"error":"not found"}"#));
        assert!(!is_decider_response(r#"{"answers":"x"}"#));
        assert!(!is_decider_response("<html>dev server</html>"));
    }

    #[test]
    fn foundation_target_advertises_only_the_pool_join_key() {
        let r = Router::new(vec![foundation_target()]);
        // Pool routing joins on the upstream name, never the catalog id.
        assert_eq!(r.all_models(), vec!["foundation-model"]);
        assert!(!r.all_models().contains(&"apple/foundation-model".into()));
    }

    #[test]
    fn lone_in_process_foundation_takes_chat_requests_and_leaves_decisions() {
        let r = Router::new(vec![
            foundation_target(),
            t(
                "decider",
                TargetKind::Decider,
                &[DECIDER_ADVERTISED_MODEL_ID],
            ),
        ]);
        // The only chat-shaped target: takes chat even unlisted.
        let got = r
            .route("/chat/completions", r#"{"model":"anything"}"#)
            .unwrap();
        assert_eq!(got.kind, TargetKind::Foundation);
        let got = r.route("/v1/decisions", r#"{"model":"pplx-decider-flash-v1.2"}"#);
        assert_eq!(got.unwrap().kind, TargetKind::Decider);
    }

    #[test]
    fn foundation_and_chat_match_by_model_when_both_exist() {
        let r = Router::new(vec![
            foundation_target(),
            t("ollama", TargetKind::Chat, &["llama3"]),
        ]);
        let got = r.route("/chat/completions", r#"{"model":"foundation-model"}"#);
        assert_eq!(got.unwrap().kind, TargetKind::Foundation);
        let got = r.route("/chat/completions", r#"{"model":"llama3"}"#);
        assert_eq!(got.unwrap().label, "ollama");
        let err = r
            .route("/chat/completions", r#"{"model":"nope"}"#)
            .unwrap_err();
        assert!(err.contains("available: foundation-model, llama3"), "{err}");
    }

    #[test]
    fn keep_selected_drops_the_foundation_target_unselected() {
        let targets = vec![
            foundation_target(),
            t("ollama", TargetKind::Chat, &["llama3"]),
        ];
        let none: BTreeSet<OnDeviceModel> = BTreeSet::new();
        let kept = keep_selected(targets.clone(), &none);
        assert!(!kept.iter().any(|t| t.kind == TargetKind::Foundation));
        assert_eq!(kept.len(), 1);
        let apple_only: BTreeSet<OnDeviceModel> =
            [OnDeviceModel::AppleFoundation].into_iter().collect();
        let kept = keep_selected(targets, &apple_only);
        assert!(kept.iter().any(|t| t.kind == TargetKind::Foundation));
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn foundation_request_with_a_bad_body_does_not_call_the_model() {
        let (tx, rx) = mpsc::sync_channel(OUTBOUND_QUEUE);
        handle_foundation_request(
            &tx,
            &req("r9", "/chat/completions", "not-json"),
            &AtomicBool::new(false),
        );
        let frames = drain(&rx);
        match frames.as_slice() {
            [ClientFrame::Error { id, message }] => {
                assert_eq!(id, "r9");
                assert!(message.contains("not JSON"), "{message}");
            }
            _ => panic!("expected one error frame, got {} frames", frames.len()),
        }
    }

    #[test]
    fn foundation_request_rejects_a_path_other_than_chat_completions() {
        let (tx, rx) = mpsc::sync_channel(OUTBOUND_QUEUE);
        handle_foundation_request(
            &tx,
            &req("r10", "/v1/models", "{}"),
            &AtomicBool::new(false),
        );
        let frames = drain(&rx);
        match frames.as_slice() {
            [ClientFrame::Error { id, message }] => {
                assert_eq!(id, "r10");
                assert!(message.contains("/v1/models"), "{message}");
            }
            _ => panic!("expected one error frame, got {} frames", frames.len()),
        }
    }

    #[test]
    fn foundation_cancel_before_start_sends_no_frames() {
        // Off the Apple driver the failure path still answers a head/chunk/done
        // triple unless the request is already cancelled.
        let (tx, rx) = mpsc::sync_channel(OUTBOUND_QUEUE);
        handle_foundation_request(
            &tx,
            &req(
                "r11",
                "/chat/completions",
                r#"{"model":"foundation-model"}"#,
            ),
            &AtomicBool::new(true),
        );
        let frames = drain(&rx);
        assert!(
            frames.is_empty(),
            "cancelled before start must send no frames: {frames:?}"
        );
    }
}
