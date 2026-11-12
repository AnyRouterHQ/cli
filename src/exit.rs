//! Exit codes and the one place errors are printed.
//!
//! Every command returns `Err(String)` on failure. A message may carry a
//! leading tag (`tag(code, msg)`) naming its exit code; `commands::run` reads
//! it, prints the message once (text, or one JSON line in JSON mode) and exits
//! with that code. Untagged errors exit 1.

use std::cell::Cell;

pub const OK: i32 = 0;
pub const ERROR: i32 = 1;
pub const USAGE: i32 = 2;
pub const NOT_FOUND: i32 = 3;
pub const AUTH: i32 = 4;
pub const RATE_LIMITED: i32 = 5;
pub const CONFIRM_REQUIRED: i32 = 6;
pub const NETWORK: i32 = 7;
pub const PAYMENT_REQUIRED: i32 = 8;

/// (code, name, meaning) — the table `anyr commands --json` and
/// `anyr help agents` print.
pub const CODES: &[(i32, &str, &str)] = &[
    (OK, "ok", "success"),
    (ERROR, "error", "the request or command failed"),
    (
        USAGE,
        "usage",
        "bad flag, argument or subcommand; nothing was sent",
    ),
    (
        NOT_FOUND,
        "not_found",
        "the model, key, id or resource does not exist",
    ),
    (
        AUTH,
        "auth",
        "no key, a rejected key, or a key without the scope",
    ),
    (RATE_LIMITED, "rate_limited", "HTTP 429; retry later"),
    (
        CONFIRM_REQUIRED,
        "confirm_required",
        "the command asks first; rerun with --yes (the hint is the exact command)",
    ),
    (
        NETWORK,
        "network",
        "no answer: DNS, TLS, connect or timeout; retry reads freely, writes only if idempotent",
    ),
    (
        PAYMENT_REQUIRED,
        "payment_required",
        "HTTP 402; add credits or change plan",
    ),
];

/// Codes worth retrying unchanged after a pause.
pub fn retryable(code: i32) -> bool {
    matches!(code, RATE_LIMITED | NETWORK)
}

const TAG: char = '\u{1}';

thread_local! {
    static JSON_ERRORS: Cell<bool> = const { Cell::new(false) };
    static REQUEST_ID: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The request id of the last failed HTTP response (`X-Request-Id`, else
/// Cloudflare's `cf-ray`); `None` after a success or when no answer came.
pub fn set_request_id(id: Option<String>) {
    REQUEST_ID.with(|r| *r.borrow_mut() = id);
}

fn request_id() -> Option<String> {
    REQUEST_ID.with(|r| r.borrow().clone())
}

/// Switch error output to one JSON object per error on stderr.
pub fn set_json_errors(on: bool) {
    JSON_ERRORS.with(|c| c.set(on));
}

pub fn json_errors() -> bool {
    JSON_ERRORS.with(Cell::get)
}

/// Mark `message` with the exit code it should produce.
pub fn tag(code: i32, message: impl AsRef<str>) -> String {
    format!("{TAG}{code}{TAG}{}", message.as_ref())
}

/// Exit code for an HTTP status that is not 2xx.
pub fn for_status(status: u16) -> i32 {
    match status {
        401 | 403 => AUTH,
        402 => PAYMENT_REQUIRED,
        404 => NOT_FOUND,
        429 => RATE_LIMITED,
        _ => ERROR,
    }
}

/// The exit code for a failure printed as `message`: a plain error (1) that
/// says the server never answered is a network failure (7); one naming a
/// restricted key is an auth failure (4).
pub fn refine(code: i32, message: &str) -> i32 {
    if code != ERROR {
        return code;
    }
    if message.contains(crate::http::SCOPE_ERROR) {
        return AUTH;
    }
    // A server status error always names `HTTP <status>`; only a request
    // that got no answer reads `Could not reach`.
    if message.contains(crate::http::UNREACHABLE) && !message.contains("HTTP ") {
        return NETWORK;
    }
    code
}

/// The exit-6 error for a command that would ask `question` but cannot (no
/// TTY) and was not given `--yes`. The hint is the same command plus `--yes`.
pub fn confirm_required(question: &str) -> String {
    tag(
        CONFIRM_REQUIRED,
        format!(
            "{question} Pass --yes to confirm in a script.\n{} {}",
            crate::term::dim("hint:"),
            crate::help::rerun_with_yes()
        ),
    )
}

/// Split a possibly tagged error into its exit code and message. The tag may
/// sit anywhere (a caller can wrap a tagged error in more text); it is removed.
pub fn classify(err: &str) -> (i32, String) {
    if let Some(rest) = err.strip_prefix(crate::cmd::dispatch::USAGE) {
        return (USAGE, rest.to_string());
    }
    if let Some(start) = err.find(TAG) {
        let after = &err[start + 1..];
        if let Some(end) = after.find(TAG) {
            if let Ok(code) = after[..end].parse() {
                let msg = format!("{}{}", &err[..start], &after[end + 1..]);
                return (code, msg);
            }
        }
    }
    (ERROR, err.to_string())
}

fn name(code: i32) -> &'static str {
    CODES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, n, _)| *n)
        .unwrap_or("error")
}

/// Print an error once, on stderr: `error: <message>` in text mode, or
/// `{"error":{"code","exit","message","hint"}}` in JSON mode.
pub fn print(code: i32, message: &str) {
    let (_, message) = classify(message);
    let message = with_next_step(&message);
    let message = message.as_str();
    if message.trim().is_empty() {
        return;
    }
    if json_errors() {
        eprintln!("{}", json_line(code, message));
    } else {
        let message = crate::key::mask_secrets(message);
        if has_prefix(&message) || (code == USAGE && !is_single_sentence(&message)) {
            eprintln!("{message}");
        } else {
            eprintln!("{} {message}", crate::term::err("error:"));
        }
    }
}

/// A one-line message that ends in `Run: x`, `Try: x` or `Use: x` becomes the
/// message plus a `hint:` line, the shape `anyr bogus` already prints.
fn with_next_step(message: &str) -> String {
    let t = message.trim_end();
    if t.contains('\n') || has_prefix(t) {
        return message.to_string();
    }
    for (marker, hint) in [
        (". Run: ", "run `{}`"),
        (". Try: ", "one of {}"),
        (". Use: ", "usage: {}"),
    ] {
        if let Some((head, rest)) = t.split_once(marker) {
            let hint = hint.replace("{}", rest.trim());
            return format!("{head}\n{} {hint}", crate::term::dim("hint:"));
        }
    }
    message.to_string()
}

/// Plain sentences get the `error:` prefix. Usage lines and multi-line help
/// text print as they are.
fn is_single_sentence(message: &str) -> bool {
    let first = message.trim_start();
    !message.trim_end().contains('\n') && !first.starts_with("Usage") && !first.starts_with("usage")
}

fn has_prefix(message: &str) -> bool {
    message.find("error:").is_some_and(|i| i < 12)
}

/// One-line JSON error object (ANSI and the `error:` prefix removed; `hint:`
/// lines move to `hint`).
pub fn json_line(code: i32, message: &str) -> String {
    let plain = strip_ansi(&crate::key::mask_secrets(message));
    let mut text = Vec::new();
    let mut hints = Vec::new();
    for line in plain.lines() {
        let t = line.trim();
        if let Some(h) = t.strip_prefix("hint:") {
            hints.push(h.trim().to_string());
        } else {
            text.push(line.trim_end());
        }
    }
    let mut msg = text.join("\n").trim().to_string();
    if let Some(rest) = msg.strip_prefix("error:") {
        msg = rest.trim_start().to_string();
    }
    let mut err = serde_json::json!({
        "code": name(code),
        "exit": code,
        "message": msg,
        "retryable": retryable(code),
    });
    if !hints.is_empty() {
        err["hint"] = serde_json::Value::String(hints.join("\n"));
    }
    // Only an error from an HTTP reply gets the id; a later local failure
    // must not carry an earlier reply's id.
    if let Some(id) = request_id()
        .filter(|_| !matches!(code, USAGE | CONFIRM_REQUIRED | NETWORK) && plain.contains("HTTP "))
    {
        err["request_id"] = serde_json::Value::String(id);
    }
    serde_json::json!({ "error": err }).to_string()
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: ESC [ … final byte in @..~
            Some('[') => {
                for n in chars.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            // OSC (hyperlinks): ESC ] … BEL or ESC \
            Some(']') => {
                while let Some(n) = chars.next() {
                    if n == '\x07' {
                        break;
                    }
                    if n == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_errors_keep_their_code_and_untagged_ones_exit_1() {
        assert_eq!(classify(&tag(AUTH, "no key")), (AUTH, "no key".into()));
        assert_eq!(classify("boom"), (ERROR, "boom".into()));
        let usage = format!("{}Usage: anyr x", crate::cmd::dispatch::USAGE);
        assert_eq!(classify(&usage), (USAGE, "Usage: anyr x".into()));
        let wrapped = format!("Could not launch: {}", tag(NOT_FOUND, "no such model"));
        assert_eq!(
            classify(&wrapped),
            (NOT_FOUND, "Could not launch: no such model".into())
        );
    }

    #[test]
    fn http_statuses_map_to_the_documented_codes() {
        assert_eq!(for_status(401), AUTH);
        assert_eq!(for_status(403), AUTH);
        assert_eq!(for_status(404), NOT_FOUND);
        assert_eq!(for_status(429), RATE_LIMITED);
        assert_eq!(for_status(402), PAYMENT_REQUIRED);
        assert_eq!(for_status(500), ERROR);
    }

    #[test]
    fn json_line_is_one_parseable_object_without_ansi_or_prefix() {
        let line = json_line(
            AUTH,
            "\x1b[31merror:\x1b[0m key rejected\n\x1b[2mhint:\x1b[0m sign in again: anyr login",
        );
        assert!(!line.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["error"]["exit"], 4);
        assert_eq!(v["error"]["code"], "auth");
        assert_eq!(v["error"]["message"], "key rejected");
        assert_eq!(v["error"]["hint"], "sign in again: anyr login");
        assert_eq!(v["error"]["retryable"], false);
        assert!(v["error"].get("request_id").is_none());
    }

    #[test]
    fn only_rate_limits_and_network_failures_are_retryable() {
        // WHY: an agent retries on `retryable`; retrying an auth or payment
        // failure burns calls and never succeeds.
        let retry: Vec<i32> = CODES
            .iter()
            .map(|c| c.0)
            .filter(|c| retryable(*c))
            .collect();
        assert_eq!(retry, vec![RATE_LIMITED, NETWORK]);
        let v: serde_json::Value = serde_json::from_str(&json_line(NETWORK, "x")).unwrap();
        assert_eq!(v["error"]["retryable"], true);
        assert_eq!(v["error"]["code"], "network");
    }

    #[test]
    fn a_failed_response_id_lands_in_the_envelope() {
        set_request_id(Some("req_1".into()));
        let v: serde_json::Value =
            serde_json::from_str(&json_line(ERROR, "HTTP 500: boom")).unwrap();
        let local: serde_json::Value =
            serde_json::from_str(&json_line(ERROR, "agent not installed")).unwrap();
        set_request_id(None);
        assert_eq!(v["error"]["request_id"], "req_1");
        assert!(local["error"].get("request_id").is_none(), "stale id");
    }

    #[test]
    fn unanswered_requests_exit_7_but_server_errors_keep_their_code() {
        let net = "Could not reach AnyRouter: https://x/api/v1/me: timed out";
        assert_eq!(refine(ERROR, net), NETWORK);
        assert_eq!(refine(ERROR, &format!("Could not launch: {net}")), NETWORK);
        assert_eq!(refine(ERROR, "HTTP 502: Could not reach upstream"), ERROR);
        assert_eq!(refine(AUTH, net), AUTH);
        assert_eq!(refine(ERROR, "boom"), ERROR);
    }

    #[test]
    fn next_step_sentences_become_a_hint_line() {
        let out = strip_ansi(&with_next_step("Unknown model \"x\". Run: anyr models"));
        assert_eq!(out, "Unknown model \"x\"\nhint: run `anyr models`");
        let out = strip_ansi(&with_next_step(
            "Unknown keys subcommand \"x\". Try: list | show",
        ));
        assert_eq!(
            out,
            "Unknown keys subcommand \"x\"\nhint: one of list | show"
        );
        assert_eq!(with_next_step("plain failure"), "plain failure");
        assert_eq!(
            with_next_step("error: a\nhint: b"),
            "error: a\nhint: b",
            "an already formed error is left alone"
        );
    }

    #[test]
    fn only_plain_sentences_get_the_error_prefix() {
        assert!(is_single_sentence("Unknown model \"x\""));
        assert!(!is_single_sentence("Usage: anyr x [flags]"));
        assert!(!is_single_sentence("line one\nline two"));
    }

    #[test]
    fn strip_ansi_removes_osc_hyperlinks() {
        assert_eq!(
            strip_ansi("\x1b]8;;https://x\x1b\\link\x1b]8;;\x1b\\"),
            "link"
        );
    }
}
