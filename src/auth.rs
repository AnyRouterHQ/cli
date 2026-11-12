//! Login acquisition: --key / env, paste, device-code (RFC 8628-style
//! `/v1/auth/cli/device/*`). Browser GUI opens the verification URL.

use std::collections::BTreeMap;
#[cfg(feature = "native")]
use std::io::{self, Write};
#[cfg(feature = "native")]
use std::process::Command;
#[cfg(feature = "native")]
use std::thread;
#[cfg(feature = "native")]
use std::time::Duration;

#[cfg(feature = "native")]
use crate::http::{http_post, join_api};
use crate::key::no_key_error;
use crate::parse::{get_string_flag, FlagValue};
use crate::term;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceToken {
    pub api_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePoll {
    Pending,
    SlowDown,
    Denied,
    Expired,
    Failed(String),
    Ready(DeviceToken),
}

pub fn parse_device_start(body: &str) -> Result<DeviceStart, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Invalid device-start response: {e}"))?;
    let device_code = value
        .get("device_code")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Device login response missing device_code.".to_string())?
        .to_string();
    let user_code = value
        .get("user_code")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let raw_uri = value
        .get("verification_uri_complete")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            value
                .get("verification_uri")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("https://anyrouter.dev/cli/device");
    let verification_uri = with_user_code(raw_uri, &user_code);
    let interval = value.get("interval").and_then(|v| v.as_u64()).unwrap_or(5);
    let expires_in = value
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(600);
    Ok(DeviceStart {
        device_code,
        user_code,
        verification_uri,
        interval,
        expires_in,
    })
}

/// Prefer the server's complete URL; otherwise append `?code=`.
pub fn with_user_code(uri: &str, user_code: &str) -> String {
    if user_code.is_empty() || uri.contains("code=") {
        return uri.to_string();
    }
    let sep = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{sep}code={user_code}")
}

fn error_code(value: &serde_json::Value) -> String {
    if let Some(s) = value.get("error").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(s) = value
        .get("error")
        .and_then(|v| v.get("code"))
        .and_then(|v| v.as_str())
    {
        return s.to_string();
    }
    String::new()
}

pub fn parse_device_token(status: u16, body: &str) -> DevicePoll {
    if (200..300).contains(&status) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
            return DevicePoll::Failed("Device login response was not JSON.".into());
        };
        let Some(api_key) = value
            .get("key")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
        else {
            return DevicePoll::Failed("Device login response did not include an API key.".into());
        };
        // Legacy servers may still send `management_key`; ignore it — API keys
        // with Key Management permission are enough.
        return DevicePoll::Ready(DeviceToken { api_key });
    }
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    match error_code(&value).as_str() {
        "authorization_pending" => DevicePoll::Pending,
        "slow_down" => DevicePoll::SlowDown,
        "access_denied" => DevicePoll::Denied,
        "expired_token" => DevicePoll::Expired,
        other => DevicePoll::Failed(format!(
            "Device login failed (HTTP {status}{}).",
            if other.is_empty() {
                String::new()
            } else {
                format!(" {other}")
            }
        )),
    }
}

pub fn browser_likely_available(env: &BTreeMap<String, String>) -> bool {
    if env.contains_key("SSH_CONNECTION") || env.contains_key("SSH_TTY") {
        return false;
    }
    if env.get("CI").map(|s| !s.is_empty()).unwrap_or(false) {
        return false;
    }
    true
}

#[cfg(not(feature = "native"))]
pub fn open_url(_url: &str) -> bool {
    false
}

#[cfg(feature = "native")]
fn spawn_opener(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(feature = "native")]
pub fn open_url(url: &str) -> bool {
    if cfg!(target_os = "macos") {
        return spawn_opener("open", &[url]);
    }
    if cfg!(target_os = "windows") {
        return spawn_opener("cmd", &["/C", "start", "", url]);
    }
    spawn_opener("xdg-open", &[url])
        || spawn_opener("gio", &["open", url])
        || spawn_opener("sensible-browser", &[url])
}

#[cfg(feature = "native")]
fn print_device_block(start: &DeviceStart, opened: bool) {
    let mins = (start.expires_in / 60).max(1);
    eprintln!();
    if opened {
        eprintln!("  {}", term::bold("Opened your browser to approve login."));
    } else {
        eprintln!("  {}", term::bold("Open this URL to sign in:"));
    }
    eprintln!();
    eprintln!("    {}", term::link(&start.verification_uri));
    eprintln!();
    eprintln!(
        "  {}",
        term::dim(&format!(
            "Approve in the browser — this terminal continues when you're done. Expires in {mins} min."
        ))
    );
    eprintln!();
    let _ = io::stderr().flush();
}

#[cfg(not(feature = "native"))]
pub fn start_device_flow(_base_url: &str, _tool: Option<&str>) -> Result<DeviceStart, String> {
    Err("device login is not available in the browser demo".into())
}

#[cfg(feature = "native")]
pub fn start_device_flow(base_url: &str, tool: Option<&str>) -> Result<DeviceStart, String> {
    let url = join_api(base_url, "/v1/auth/cli/device/start");
    let env: BTreeMap<String, String> = crate::sort::map(std::env::vars());
    let label = crate::cmd::key_audit::device_key_label(
        &crate::cmd::key_audit::machine_host(&env),
        crate::civil::now_secs(),
    );
    let body = device_start_body(tool, &label);
    let (status, resp) = http_post(&url, None, Some(&body))?;
    if !(200..300).contains(&status) {
        return Err(format!("Failed to start device login (HTTP {status})."));
    }
    parse_device_start(&resp)
}

/// The device-start request: the launching tool and the name for the new key.
pub fn device_start_body(tool: Option<&str>, key_label: &str) -> String {
    let mut body = serde_json::json!({ "key_label": key_label });
    if let Some(t) = tool.filter(|t| !t.is_empty()) {
        body["tool"] = serde_json::Value::String(t.to_string());
    }
    body.to_string()
}

/// What the poll loop should do next, derived from one server response or
/// transport outcome plus the running clock budget. Pure — unit-testable.
#[derive(Debug)]
enum PollAction {
    Continue,
    SlowDown,
    GiveUp(String),
}

/// Decide the action for one poll iteration. `elapsed_secs` is wall time since
/// the flow started; `net_errors` counts consecutive transport failures.
fn poll_action(
    outcome: &Result<DevicePoll, ()>, // Err(()) = transport/network error
    elapsed_secs: u64,
    net_errors: u32,
    expires_in: u64,
) -> PollAction {
    if matches!(outcome, Err(())) && net_errors >= 5 {
        return PollAction::GiveUp(
            "Device login lost contact with the server after several retries. Check your network and re-run."
                .into(),
        );
    }
    if elapsed_secs >= expires_in {
        return PollAction::GiveUp(
            "Device login code expired. Re-run to start a new login.".into(),
        );
    }
    match outcome {
        Err(()) => PollAction::Continue,
        Ok(DevicePoll::Pending) => PollAction::Continue,
        Ok(DevicePoll::SlowDown) => PollAction::SlowDown,
        Ok(DevicePoll::Denied) => PollAction::GiveUp("Device login was denied by the user.".into()),
        Ok(DevicePoll::Expired) => {
            PollAction::GiveUp("Device login code expired. Re-run to start a new login.".into())
        }
        Ok(DevicePoll::Failed(msg)) => PollAction::GiveUp(msg.clone()),
        Ok(DevicePoll::Ready(_)) => PollAction::Continue,
    }
}

#[cfg(not(feature = "native"))]
pub fn poll_device_token(_base_url: &str, _start: &DeviceStart) -> Result<DeviceToken, String> {
    Err("device login is not available in the browser demo".into())
}

#[cfg(feature = "native")]
pub fn poll_device_token(base_url: &str, start: &DeviceStart) -> Result<DeviceToken, String> {
    let url = join_api(base_url, "/v1/auth/cli/device/token");
    let payload = serde_json::json!({ "device_code": start.device_code }).to_string();
    let mut interval_ms = start.interval.saturating_mul(1000).max(1000);
    let started = std::time::Instant::now();
    let mut net_errors: u32 = 0;
    let expires_secs = start.expires_in.max(60);
    loop {
        thread::sleep(Duration::from_millis(interval_ms));
        let outcome = http_post(&url, None, Some(&payload))
            .map(|(status, body)| parse_device_token(status, &body))
            .map_err(|_| ());
        match outcome {
            Ok(DevicePoll::Ready(token)) => return Ok(token),
            other => {
                let elapsed = started.elapsed().as_secs();
                let consecutive_net_errors = if other.is_err() {
                    net_errors.saturating_add(1)
                } else {
                    net_errors
                };
                match poll_action(&other, elapsed, consecutive_net_errors, expires_secs) {
                    PollAction::Continue => {
                        if other.is_err() {
                            eprintln!("{}", term::warn("Network error while polling. Retrying…"));
                            net_errors = consecutive_net_errors;
                        } else {
                            net_errors = 0;
                        }
                    }
                    PollAction::SlowDown => {
                        net_errors = 0;
                        interval_ms = interval_ms.saturating_add(5000);
                    }
                    PollAction::GiveUp(msg) => return Err(msg),
                }
            }
        }
    }
}

#[cfg(not(feature = "native"))]
pub fn login_via_device(
    _base_url: &str,
    _tool: Option<&str>,
    _open_browser: bool,
) -> Result<DeviceToken, String> {
    Err("device login is not available in the browser demo".into())
}

#[cfg(feature = "native")]
pub fn login_via_device(
    base_url: &str,
    tool: Option<&str>,
    open_browser: bool,
) -> Result<DeviceToken, String> {
    let start = start_device_flow(base_url, tool)?;
    let opened = open_browser && open_url(&start.verification_uri);
    print_device_block(&start, opened);
    poll_device_token(base_url, &start)
}

#[derive(Debug, Clone)]
pub struct AcquiredKey {
    pub api_key: String,
    pub source: String,
}

/// Wraps the shared `--key` handling (see `crate::key::key_from_flag`).
fn key_from_flag(
    raw: &str,
    stdin: &mut dyn std::io::BufRead,
    warn_to: &mut dyn std::io::Write,
) -> Result<Option<AcquiredKey>, String> {
    Ok(
        crate::key::key_from_flag(raw, stdin, warn_to)?.map(|(api_key, from_stdin)| AcquiredKey {
            api_key,
            source: if from_stdin { "stdin" } else { "--key" }.into(),
        }),
    )
}

/// Priority: --key / ANYROUTER_API_KEY → --device → TTY paste / auto device.
pub fn acquire_api_key(
    flags: &std::collections::HashMap<String, FlagValue>,
    env: &BTreeMap<String, String>,
    base_url: &str,
    tool: Option<&str>,
) -> Result<AcquiredKey, String> {
    if let Some(key) = get_string_flag(flags, "key") {
        if let Some(acquired) =
            key_from_flag(&key, &mut std::io::stdin().lock(), &mut std::io::stderr())?
        {
            return Ok(acquired);
        }
    }
    if let Some(key) = env
        .get("ANYROUTER_API_KEY")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return Ok(AcquiredKey {
            api_key: key.to_string(),
            source: "ANYROUTER_API_KEY".into(),
        });
    }

    let force_device = matches!(flags.get("device"), Some(FlagValue::Bool(true)))
        || matches!(flags.get("device-code"), Some(FlagValue::Bool(true)));
    if force_device {
        let token = login_via_device(base_url, tool, browser_likely_available(env))?;
        return Ok(AcquiredKey {
            api_key: token.api_key,
            source: "device code".into(),
        });
    }

    // Machine output (`--json`, `ANYR_OUTPUT=json`, `--quiet`) never logs in,
    // even on a TTY: an agent driving anyr cannot answer a device-code page.
    let interactive = term::is_interactive()
        && !matches!(flags.get("yes"), Some(FlagValue::Bool(true)))
        && !term::machine_output_flags(flags, env);
    if !interactive {
        return Err(no_key_error());
    }

    eprintln!();
    eprintln!("{}", term::bold("Welcome to AnyRouter"));
    eprintln!(
        "{}",
        term::dim("One key runs Claude Code, Codex, Grok Build and the rest through the gateway.")
    );
    eprintln!();

    if matches!(flags.get("paste"), Some(FlagValue::Bool(true))) {
        let api_key = term::prompt_secret("Paste your AnyRouter API key (sk-ar-...): ")?;
        if api_key.is_empty() {
            return Err("No key entered.".into());
        }
        return Ok(AcquiredKey {
            api_key,
            source: "paste".into(),
        });
    }

    let open_browser = browser_likely_available(env);
    match login_via_device(base_url, tool, open_browser) {
        Ok(token) => Ok(AcquiredKey {
            api_key: token.api_key,
            source: if open_browser {
                "browser".into()
            } else {
                "device code".into()
            },
        }),
        Err(err) => {
            eprintln!("{}", term::warn(&format!("{err} — paste a key instead.")));
            let api_key = term::prompt_secret("Paste your AnyRouter API key (sk-ar-...): ")?;
            if api_key.is_empty() {
                return Err("No key entered.".into());
            }
            Ok(AcquiredKey {
                api_key,
                source: "paste".into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_start_names_the_key_after_the_device() {
        let body: serde_json::Value =
            serde_json::from_str(&device_start_body(Some("cli"), "studio anyr 2026-10-27"))
                .unwrap();
        assert_eq!(body["key_label"], "studio anyr 2026-10-27");
        assert_eq!(body["tool"], "cli");
        let bare: serde_json::Value =
            serde_json::from_str(&device_start_body(None, "studio anyr 2026-10-27")).unwrap();
        assert!(bare.get("tool").is_none());
    }

    #[test]
    fn parse_device_start_reads_fields() {
        let start = parse_device_start(
            r#"{"device_code":"dc_1","user_code":"ABCD-EFGH","verification_uri":"https://anyrouter.dev/cli/device","interval":5,"expires_in":600}"#,
        )
        .unwrap();
        assert_eq!(start.device_code, "dc_1");
        assert_eq!(start.user_code, "ABCD-EFGH");
        assert_eq!(start.interval, 5);
        assert_eq!(
            start.verification_uri,
            "https://anyrouter.dev/cli/device?code=ABCD-EFGH"
        );
    }

    #[test]
    fn key_dash_reads_stdin_without_warning() {
        let mut warn = Vec::new();
        let got = key_from_flag("-", &mut "sk-ar-v1-abc\n".as_bytes(), &mut warn)
            .unwrap()
            .unwrap();
        assert_eq!(got.api_key, "sk-ar-v1-abc");
        assert_eq!(got.source, "stdin");
        assert!(warn.is_empty(), "stdin is the safe path; nothing to warn");
    }

    #[test]
    fn key_dash_with_empty_stdin_fails_loud() {
        let err = key_from_flag("-", &mut "".as_bytes(), &mut Vec::new()).unwrap_err();
        assert!(err.contains("No key on stdin"), "{err}");
    }

    #[test]
    fn literal_key_flag_warns_about_argv_exposure() {
        let mut warn = Vec::new();
        let got = key_from_flag(" sk-ar-v1-abc ", &mut "".as_bytes(), &mut warn)
            .unwrap()
            .unwrap();
        assert_eq!(got.api_key, "sk-ar-v1-abc");
        let text = String::from_utf8(warn).unwrap();
        assert!(
            text.contains("process list") && text.contains("--key -"),
            "{text}"
        );
        assert!(!text.contains("sk-ar"), "warning must not echo the secret");
    }

    #[test]
    fn parse_device_start_prefers_complete_uri() {
        let start = parse_device_start(
            r#"{"device_code":"dc_1","user_code":"ABCD-EFGH","verification_uri":"https://anyrouter.dev/cli/device","verification_uri_complete":"https://anyrouter.dev/cli/device?code=ABCD-EFGH","interval":5,"expires_in":600}"#,
        )
        .unwrap();
        assert_eq!(
            start.verification_uri,
            "https://anyrouter.dev/cli/device?code=ABCD-EFGH"
        );
    }

    #[test]
    fn with_user_code_is_idempotent_when_already_present() {
        let url = "https://anyrouter.dev/cli/device?code=ABCD-EFGH";
        assert_eq!(with_user_code(url, "ABCD-EFGH"), url);
    }

    #[test]
    fn parse_device_token_ready_ignores_legacy_management_key() {
        let poll = parse_device_token(
            200,
            r#"{"key":"sk-ar-v1-secret","management_key":{"secret":"ak_mgmt"}}"#,
        );
        match poll {
            DevicePoll::Ready(t) => {
                assert_eq!(t.api_key, "sk-ar-v1-secret");
            }
            other => panic!("expected ready, got {other:?}"),
        }
    }

    #[test]
    fn parse_device_token_pending_nested_and_flat() {
        assert!(matches!(
            parse_device_token(400, r#"{"error":{"code":"authorization_pending"}}"#),
            DevicePoll::Pending
        ));
        assert!(matches!(
            parse_device_token(400, r#"{"error":"authorization_pending"}"#),
            DevicePoll::Pending
        ));
        assert!(matches!(
            parse_device_token(400, r#"{"error":"slow_down"}"#),
            DevicePoll::SlowDown
        ));
        assert!(matches!(
            parse_device_token(400, r#"{"error":"access_denied"}"#),
            DevicePoll::Denied
        ));
        assert!(matches!(
            parse_device_token(400, r#"{"error":"expired_token"}"#),
            DevicePoll::Expired
        ));
    }

    #[test]
    fn ssh_and_ci_skip_browser() {
        let mut env = BTreeMap::new();
        env.insert("SSH_CONNECTION".into(), "1 2 3 4".into());
        assert!(!browser_likely_available(&env));
        env.clear();
        env.insert("CI".into(), "true".into());
        assert!(!browser_likely_available(&env));
    }

    #[test]
    fn local_session_tries_browser() {
        let env = BTreeMap::new();
        assert!(browser_likely_available(&env));
    }

    #[test]
    fn poll_action_gives_up_after_five_transport_errors() {
        for n in [0u32, 1, 3, 4] {
            assert!(matches!(
                poll_action(&Err(()), 10, n, 600),
                PollAction::Continue
            ));
        }
        assert!(matches!(
            poll_action(&Err(()), 10, 5, 600),
            PollAction::GiveUp(_)
        ));
    }

    #[test]
    fn poll_action_enforces_wall_clock_expiry() {
        // Even a clean Pending answer can't outlive expires_in.
        let pending = Ok(DevicePoll::Pending);
        assert!(matches!(
            poll_action(&pending, 599, 0, 600),
            PollAction::Continue
        ));
        assert!(matches!(
            poll_action(&pending, 600, 0, 600),
            PollAction::GiveUp(_)
        ));
    }

    #[test]
    fn poll_action_maps_terminal_states() {
        assert!(matches!(
            poll_action(&Ok(DevicePoll::Denied), 10, 0, 600),
            PollAction::GiveUp(_)
        ));
        match poll_action(&Ok(DevicePoll::Expired), 10, 0, 600) {
            PollAction::GiveUp(msg) => {
                assert!(
                    msg.contains("expired"),
                    "user-visible expiry text missing: {msg}"
                );
            }
            other => panic!("expected GiveUp, got {other:?}"),
        }
        assert!(matches!(
            poll_action(&Ok(DevicePoll::SlowDown), 10, 0, 600),
            PollAction::SlowDown
        ));
    }
}
