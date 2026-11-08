//! In-process Apple system language model for `anyr relay`.
//!
//! Rust cannot import the system framework. On Apple Silicon the build embeds
//! a small Swift helper (`foundation_relay.swift`) and loads it only when the
//! OS is new enough to run it. The helper calls `SystemLanguageModel.default`
//! and `LanguageModelSession`. This module turns the OpenAI chat body into
//! that prompt and the helper's text into the same head/chunk/done frames an
//! HTTP target would return.
//!
//! Other platforms leave the helper out. `foundation_model_available` is then
//! false, and relay keeps the HTTP (`fm serve`) probe.

use super::FM_SERVE_ADVERTISED_MODEL_ID;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

#[cfg(anyr_foundation_model)]
use std::ffi::{CStr, CString};
#[cfg(anyr_foundation_model)]
use std::os::raw::{c_char, c_void};
#[cfg(anyr_foundation_model)]
use std::sync::Mutex;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FmEvent {
    Head { status: u16, content_type: String },
    Chunk(String),
    Done,
    Error(String),
}

#[derive(Debug)]
// The Apple driver is the production caller. Other builds keep the encoder so
// the wire format stays tested, and the fields are unread outside that driver.
#[cfg_attr(not(anyr_foundation_model), allow(dead_code))]
pub(crate) struct FmSession {
    id: String,
    model: String,
    stream: bool,
    instructions: String,
    prompt: String,
    last: String,
    sent_head: bool,
    sent_role: bool,
    failed: bool,
    finished: bool,
}

#[cfg_attr(not(anyr_foundation_model), allow(dead_code))]
impl FmSession {
    pub(crate) fn from_chat_body(id: &str, body: &str) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(body).map_err(|_| "the chat request is not JSON".to_string())?;
        let messages = value
            .get("messages")
            .and_then(|m| m.as_array())
            .ok_or_else(|| "the chat request has no messages".to_string())?;
        let (instructions, prompt) = prompt_from_messages(messages)?;
        let model = value
            .get("model")
            .and_then(|m| m.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(FM_SERVE_ADVERTISED_MODEL_ID)
            .to_string();
        Ok(Self {
            id: id.to_string(),
            model,
            stream: value.get("stream").and_then(|s| s.as_bool()) == Some(true),
            instructions,
            prompt,
            last: String::new(),
            sent_head: false,
            sent_role: false,
            failed: false,
            finished: false,
        })
    }

    pub(crate) fn instructions(&self) -> &str {
        &self.instructions
    }

    pub(crate) fn prompt(&self) -> &str {
        &self.prompt
    }

    pub(crate) fn wants_stream(&self) -> bool {
        self.stream
    }

    #[cfg(anyr_foundation_model)]
    fn empty() -> Self {
        Self {
            id: String::new(),
            model: FM_SERVE_ADVERTISED_MODEL_ID.to_string(),
            stream: false,
            instructions: String::new(),
            prompt: String::new(),
            last: String::new(),
            sent_head: false,
            sent_role: false,
            failed: false,
            finished: false,
        }
    }

    /// Record one cumulative snapshot from the helper. Streaming sessions emit
    /// the new suffix immediately. A non-streaming session stores the text and
    /// emits it from [`finish`](Self::finish).
    pub(crate) fn push_snapshot(&mut self, cumulative: &str) -> Vec<FmEvent> {
        if self.failed || self.finished {
            return Vec::new();
        }
        let delta = snapshot_delta(&self.last, cumulative);
        self.last = cumulative.to_string();
        if !self.stream || delta.is_empty() {
            return Vec::new();
        }
        let mut events = self.ensure_head();
        events.push(FmEvent::Chunk(self.content_chunk(&delta)));
        events
    }

    pub(crate) fn finish(&mut self) -> Vec<FmEvent> {
        if self.failed || self.finished {
            return Vec::new();
        }
        self.finished = true;
        if self.stream {
            let mut events = self.ensure_head();
            events.push(FmEvent::Chunk(self.stop_chunk()));
            events.push(FmEvent::Chunk("data: [DONE]\n\n".to_string()));
            events.push(FmEvent::Done);
            events
        } else {
            let mut events = self.ensure_head();
            events.push(FmEvent::Chunk(self.completion_json()));
            events.push(FmEvent::Done);
            events
        }
    }

    /// Model failure before any head becomes an OpenAI error response. After
    /// the head has been sent the status is fixed, so the relay error frame
    /// ends the stream.
    pub(crate) fn fail(&mut self, message: &str) -> Vec<FmEvent> {
        if self.failed || self.finished {
            return Vec::new();
        }
        self.failed = true;
        if self.sent_head {
            return vec![FmEvent::Error(format!("system model failed: {message}"))];
        }
        self.sent_head = true;
        let body = serde_json::json!({
            "error": { "message": message, "type": "invalid_request_error" }
        })
        .to_string();
        vec![
            FmEvent::Head {
                status: 400,
                content_type: "application/json".to_string(),
            },
            FmEvent::Chunk(body),
            FmEvent::Done,
        ]
    }

    fn ensure_head(&mut self) -> Vec<FmEvent> {
        if self.sent_head {
            return Vec::new();
        }
        self.sent_head = true;
        let content_type = if self.stream {
            "text/event-stream"
        } else {
            "application/json"
        };
        vec![FmEvent::Head {
            status: 200,
            content_type: content_type.to_string(),
        }]
    }

    fn completion_id(&self) -> String {
        format!("chatcmpl-{}", self.id)
    }

    fn content_chunk(&mut self, delta: &str) -> String {
        let mut piece = serde_json::json!({ "content": delta });
        if !self.sent_role {
            piece["role"] = serde_json::json!("assistant");
            self.sent_role = true;
        }
        sse(&serde_json::json!({
            "id": self.completion_id(),
            "object": "chat.completion.chunk",
            "model": self.model,
            "choices": [{ "index": 0, "delta": piece, "finish_reason": null }]
        }))
    }

    fn stop_chunk(&mut self) -> String {
        let delta = if self.sent_role {
            serde_json::json!({})
        } else {
            self.sent_role = true;
            serde_json::json!({ "role": "assistant" })
        };
        sse(&serde_json::json!({
            "id": self.completion_id(),
            "object": "chat.completion.chunk",
            "model": self.model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": "stop" }]
        }))
    }

    fn completion_json(&self) -> String {
        serde_json::json!({
            "id": self.completion_id(),
            "object": "chat.completion",
            "model": self.model,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": self.last },
                "finish_reason": "stop"
            }]
        })
        .to_string()
    }
}

#[cfg_attr(not(anyr_foundation_model), allow(dead_code))]
fn sse(value: &serde_json::Value) -> String {
    format!("data: {value}\n\n")
}

#[cfg_attr(not(anyr_foundation_model), allow(dead_code))]
fn snapshot_delta(previous: &str, cumulative: &str) -> String {
    cumulative
        .strip_prefix(previous)
        .unwrap_or(cumulative)
        .to_string()
}

fn prompt_from_messages(messages: &[serde_json::Value]) -> Result<(String, String), String> {
    let mut instructions: Vec<String> = Vec::new();
    let mut turns: Vec<String> = Vec::new();
    for message in messages {
        let role = message.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let text = message
            .get("content")
            .map(content_text)
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        match role {
            "system" | "developer" => instructions.push(text),
            "assistant" => turns.push(format!("Assistant: {text}")),
            "tool" | "function" => turns.push(format!("Tool: {text}")),
            _ => turns.push(format!("User: {text}")),
        }
    }
    if turns.is_empty() {
        return Err("the chat request has no user message".into());
    }
    Ok((instructions.join("\n\n"), turns.join("\n\n")))
}

fn content_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts.iter().filter_map(part_text).collect(),
        _ => String::new(),
    }
}

fn part_text(part: &serde_json::Value) -> Option<String> {
    if let Some(text) = part.as_str() {
        return Some(text.to_string());
    }
    let obj = part.as_object()?;
    obj.get("text")
        .or_else(|| obj.get("content"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// True when this process can answer relay requests with the system model.
/// False on Windows, Linux, Intel Macs, and whenever the model is not available.
/// The false path does not mention the model.
pub(crate) fn foundation_model_available() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| probe_foundation_model().unwrap_or_default())
}

/// Load the helper and ask whether `SystemLanguageModel.default` is available.
/// `Ok(false)` means the helper ran and the model is not available.
pub(crate) fn probe_foundation_model() -> Result<bool, String> {
    #[cfg(not(anyr_foundation_model))]
    {
        Err("system model helper is not part of this build".into())
    }
    #[cfg(anyr_foundation_model)]
    {
        if macos_product_major().is_some_and(|major| major < 26) {
            return Ok(false);
        }
        let lib = load_foundation_lib()?;
        let ready = unsafe { (lib.status)() } == 1;
        Ok(ready)
    }
}

pub(crate) fn run_foundation_model(
    session: &mut FmSession,
    cancel: &AtomicBool,
    send: impl FnMut(FmEvent) + Send + 'static,
) {
    if cancel.load(Ordering::SeqCst) {
        return;
    }
    #[cfg(not(anyr_foundation_model))]
    {
        let mut send = send;
        for event in session.fail("system model is unavailable") {
            send(event);
        }
    }
    #[cfg(anyr_foundation_model)]
    {
        drive_foundation_model(session, cancel, send);
    }
}

#[cfg(anyr_foundation_model)]
fn macos_product_major() -> Option<u32> {
    let output = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    text.trim().split('.').next()?.parse().ok()
}

#[cfg(anyr_foundation_model)]
struct FmLib {
    _handle: *mut c_void,
    status: unsafe extern "C" fn() -> i32,
    complete: unsafe extern "C" fn(
        *const c_char,
        *const c_char,
        i32,
        unsafe extern "C" fn(*mut c_void, *const c_char),
        unsafe extern "C" fn(*mut c_void, *const c_char),
        unsafe extern "C" fn(*mut c_void) -> i32,
        *mut c_void,
    ) -> i32,
}

#[cfg(anyr_foundation_model)]
unsafe impl Send for FmLib {}
#[cfg(anyr_foundation_model)]
unsafe impl Sync for FmLib {}

#[cfg(anyr_foundation_model)]
fn load_foundation_lib() -> Result<&'static FmLib, String> {
    static LIB: OnceLock<Result<FmLib, String>> = OnceLock::new();
    let slot = LIB.get_or_init(load_foundation_lib_once);
    match slot {
        Ok(lib) => Ok(lib),
        Err(err) => Err(err.clone()),
    }
}

#[cfg(anyr_foundation_model)]
fn load_foundation_lib_once() -> Result<FmLib, String> {
    // A per-version, per-process directory under the temp dir, owner-only on
    // unix (same recipe as the config dir). The helper bytes are written with
    // an owner-only file so no other user can swap or read the dylib while it
    // loads. The file is removed right after `dlopen` (the mapping stays).
    let dir = std::env::temp_dir().join(format!(
        "anyr-fm-{}-{}",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    ));
    crate::config::ensure_private_dir(&dir)
        .map_err(|err| format!("could not create the system model helper dir: {err}"))?;
    let path = dir.join("libanyrfm.dylib");
    crate::config::write_private(
        &path,
        include_bytes!(concat!(env!("OUT_DIR"), "/libanyrfm.dylib")),
    )
    .map_err(|err| format!("could not write the system model helper: {err}"))?;
    let c_path = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| "system model helper path is not valid".to_string())?;
    // SAFETY: the path is a NUL-terminated filesystem path. A null handle is checked.
    let handle = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    let _ = std::fs::remove_file(&path);
    if handle.is_null() {
        return Err(format!(
            "could not load the system model helper: {}",
            dl_error()
        ));
    }
    let status = load_symbol(handle, "anyr_fm_status")?;
    let complete = load_symbol(handle, "anyr_fm_complete")?;
    Ok(FmLib {
        _handle: handle,
        status,
        complete,
    })
}

#[cfg(anyr_foundation_model)]
fn load_symbol<T>(handle: *mut c_void, name: &str) -> Result<T, String> {
    let c_name = CString::new(name).map_err(|_| format!("bad helper symbol {name}"))?;
    // SAFETY: `handle` came from a successful dlopen and is still open.
    // `dlsym` returns null when the symbol is missing.
    let sym = unsafe { libc::dlsym(handle, c_name.as_ptr()) };
    if sym.is_null() {
        return Err(format!(
            "system model helper is missing {name}: {}",
            dl_error()
        ));
    }
    // SAFETY: the Swift helper exports this exact C signature under `name`.
    Ok(unsafe { std::mem::transmute_copy(&sym) })
}

#[cfg(anyr_foundation_model)]
fn dl_error() -> String {
    // SAFETY: dlerror returns a thread-local C string, or null.
    let err = unsafe { libc::dlerror() };
    if err.is_null() {
        "unknown dynamic loader error".to_string()
    } else {
        unsafe { CStr::from_ptr(err) }
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(anyr_foundation_model)]
struct DriveSlot {
    session: Mutex<FmSession>,
    cancel: *const AtomicBool,
    send: Mutex<Box<dyn FnMut(FmEvent) + Send>>,
}

#[cfg(anyr_foundation_model)]
unsafe impl Send for DriveSlot {}
#[cfg(anyr_foundation_model)]
unsafe impl Sync for DriveSlot {}

#[cfg(anyr_foundation_model)]
fn deliver(session: &mut FmSession, send: &mut impl FnMut(FmEvent), message: &str) {
    for event in session.fail(message) {
        send(event);
    }
}

#[cfg(anyr_foundation_model)]
fn drive_foundation_model(
    session: &mut FmSession,
    cancel: &AtomicBool,
    mut send: impl FnMut(FmEvent) + Send + 'static,
) {
    let mut owned = std::mem::replace(session, FmSession::empty());
    let instructions = match CString::new(owned.instructions()) {
        Ok(value) => value,
        Err(_) => {
            deliver(
                &mut owned,
                &mut send,
                "the chat request contains a NUL byte",
            );
            *session = owned;
            return;
        }
    };
    let prompt = match CString::new(owned.prompt()) {
        Ok(value) => value,
        Err(_) => {
            deliver(
                &mut owned,
                &mut send,
                "the chat request contains a NUL byte",
            );
            *session = owned;
            return;
        }
    };
    let stream = i32::from(owned.wants_stream());
    let slot = DriveSlot {
        session: Mutex::new(owned),
        cancel,
        send: Mutex::new(Box::new(send)),
    };
    let rc = match load_foundation_lib() {
        Ok(lib) => {
            // SAFETY: `slot` stays alive until `complete` returns. Swift calls
            // the callbacks serially and only with this pointer. The C strings
            // stay alive for the same span. The helper copies them before it
            // awaits.
            unsafe {
                (lib.complete)(
                    instructions.as_ptr(),
                    prompt.as_ptr(),
                    stream,
                    on_text,
                    on_error,
                    should_stop,
                    (&slot as *const DriveSlot).cast::<c_void>().cast_mut(),
                )
            }
        }
        Err(err) => {
            emit_fail(&slot, &err);
            1
        }
    };
    let cancelled = unsafe { (*slot.cancel).load(Ordering::SeqCst) };
    if rc == 0 && !cancelled {
        emit_events(&slot, |session| session.finish());
    } else if rc == 1 {
        emit_events(&slot, |session| session.fail("system model failed"));
    }
    let restored = slot
        .session
        .into_inner()
        .unwrap_or_else(|err| err.into_inner());
    *session = restored;
}

#[cfg(anyr_foundation_model)]
fn emit_fail(slot: &DriveSlot, message: &str) {
    emit_events(slot, |session| session.fail(message));
}

#[cfg(anyr_foundation_model)]
fn emit_events(slot: &DriveSlot, make: impl FnOnce(&mut FmSession) -> Vec<FmEvent>) {
    let events = {
        let mut session = slot.session.lock().unwrap_or_else(|err| err.into_inner());
        make(&mut session)
    };
    let mut send = slot.send.lock().unwrap_or_else(|err| err.into_inner());
    for event in events {
        send(event);
    }
}

#[cfg(anyr_foundation_model)]
unsafe extern "C" fn on_text(ctx: *mut c_void, text: *const c_char) {
    callback_text(ctx, text, true);
}

#[cfg(anyr_foundation_model)]
unsafe extern "C" fn on_error(ctx: *mut c_void, text: *const c_char) {
    callback_text(ctx, text, false);
}

#[cfg(anyr_foundation_model)]
fn callback_text(ctx: *mut c_void, text: *const c_char, snapshot: bool) {
    if ctx.is_null() || text.is_null() {
        return;
    }
    // SAFETY: `ctx` is the DriveSlot passed to anyr_fm_complete. The caller
    // blocks until that call returns, and Swift invokes callbacks one at a time.
    let slot = unsafe { &*ctx.cast::<DriveSlot>() };
    let stopped = unsafe { (*slot.cancel).load(Ordering::SeqCst) };
    if stopped {
        return;
    }
    let text = unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned();
    if snapshot {
        emit_events(slot, |session| session.push_snapshot(&text));
    } else {
        emit_events(slot, |session| session.fail(&text));
    }
}

#[cfg(anyr_foundation_model)]
unsafe extern "C" fn should_stop(ctx: *mut c_void) -> i32 {
    if ctx.is_null() {
        return 1;
    }
    let slot = unsafe { &*ctx.cast::<DriveSlot>() };
    i32::from(unsafe { (*slot.cancel).load(Ordering::SeqCst) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(body: &str) -> FmSession {
        FmSession::from_chat_body("req", body).expect("body should parse")
    }

    fn chunks(events: &[FmEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                FmEvent::Chunk(data) => Some(data.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn prompt_keeps_system_text_out_of_the_user_turns() {
        let session = session(
            r#"{"model":"foundation-model","messages":[
                {"role":"system","content":"be brief"},
                {"role":"user","content":"hi"},
                {"role":"assistant","content":"hello"},
                {"role":"user","content":[{"type":"text","text":"again"}]}
            ]}"#,
        );
        assert_eq!(session.instructions(), "be brief");
        assert_eq!(
            session.prompt(),
            "User: hi\n\nAssistant: hello\n\nUser: again"
        );
        assert!(!session.wants_stream());
        assert_eq!(session.model, FM_SERVE_ADVERTISED_MODEL_ID);
    }

    #[test]
    fn missing_messages_name_the_chat_request() {
        let err = FmSession::from_chat_body("req", "{}").unwrap_err();
        assert!(err.contains("chat request"), "{err}");
        let err = FmSession::from_chat_body("req", "nope").unwrap_err();
        assert!(err.contains("not JSON"), "{err}");
        // Instructions alone are not a turn the model can answer.
        let err = FmSession::from_chat_body(
            "req",
            r#"{"messages":[{"role":"system","content":"be brief"}]}"#,
        )
        .unwrap_err();
        assert!(err.contains("no user message"), "{err}");
    }

    #[test]
    fn cancel_before_start_sends_no_frames() {
        let mut session = session(r#"{"messages":[{"role":"user","content":"hi"}]}"#);
        let (tx, rx) = std::sync::mpsc::channel();
        run_foundation_model(&mut session, &AtomicBool::new(true), move |event| {
            let _ = tx.send(event);
        });
        assert!(rx.try_iter().next().is_none());
    }

    #[test]
    fn stream_deltas_use_the_upstream_model_name() {
        let mut session = session(
            r#"{"model":"foundation-model","stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
        );
        let first = session.push_snapshot("Hel");
        let second = session.push_snapshot("Hello");
        let body = format!("{}{}", chunks(&first), chunks(&second));
        assert!(body.contains("\"model\":\"foundation-model\""), "{body}");
        assert!(!body.contains("apple/foundation-model"), "{body}");
        assert!(body.contains("\"content\":\"Hel\""), "{body}");
        assert!(body.contains("\"content\":\"lo\""), "{body}");
        assert!(matches!(
            first.first(),
            Some(FmEvent::Head {
                status: 200,
                content_type
            }) if content_type == "text/event-stream"
        ));
        let tail = chunks(&session.finish());
        assert!(tail.contains("\"finish_reason\":\"stop\""), "{tail}");
        assert!(tail.contains("data: [DONE]\n\n"), "{tail}");
    }

    #[test]
    fn non_stream_response_is_one_json_completion() {
        let mut session = session(r#"{"messages":[{"role":"user","content":"hi"}]}"#);
        assert!(session.push_snapshot("done").is_empty());
        let events = session.finish();
        assert!(matches!(
            events.first(),
            Some(FmEvent::Head {
                status: 200,
                content_type
            }) if content_type == "application/json"
        ));
        let body = chunks(&events);
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["model"], FM_SERVE_ADVERTISED_MODEL_ID);
        assert_eq!(value["choices"][0]["message"]["content"], "done");
        assert_ne!(value["model"], "apple/foundation-model");
    }

    #[test]
    fn failure_before_a_head_is_an_openai_error() {
        let mut session = session(r#"{"stream":true,"messages":[{"role":"user","content":"hi"}]}"#);
        let events = session.fail("turned off");
        assert!(matches!(
            events.first(),
            Some(FmEvent::Head { status: 400, .. })
        ));
        let body = chunks(&events);
        assert!(body.contains("turned off"), "{body}");
        assert!(body.contains("invalid_request_error"), "{body}");
        assert!(session.finish().is_empty());
    }

    #[cfg(anyr_foundation_model)]
    #[test]
    fn foundation_helper_loads() {
        // The dylib has to open even when the model itself is unavailable.
        // A load error here means the Apple Silicon build cannot answer
        // relay frames in-process.
        probe_foundation_model().expect("system model helper should load");
    }

    #[test]
    fn unavailable_helper_reports_the_system_model_without_the_catalog_id() {
        if foundation_model_available() {
            return;
        }
        let mut session = session(r#"{"messages":[{"role":"user","content":"hi"}]}"#);
        let (tx, rx) = std::sync::mpsc::channel();
        run_foundation_model(&mut session, &AtomicBool::new(false), move |event| {
            let _ = tx.send(event);
        });
        let events: Vec<_> = rx.try_iter().collect();
        let rendered = format!("{events:?}");
        assert!(
            rendered.contains("system model"),
            "expected the system-model failure, got {rendered}"
        );
        assert!(!rendered.contains("apple/foundation-model"), "{rendered}");
    }
}
