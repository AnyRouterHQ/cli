//! Pure rendering for `relay status`, the live screen and `relay models`.
//! Color and time are injected so snapshot tests compare plain strings.

use super::state::{Conn, PoolSource, Snapshot};

pub(super) const POOL_URL: &str = "https://anyrouter.dev/pool";
pub(super) const DEVICES_URL: &str = "https://dash.anyrouter.dev/devices";

/// ANSI painter that is a no-op when color is off.
#[derive(Clone, Copy)]
pub(super) struct Paint(pub bool);

impl Paint {
    fn wrap(self, code: &str, s: &str) -> String {
        if self.0 {
            format!("{code}{s}{}", crate::term::RESET)
        } else {
            s.to_string()
        }
    }
    pub fn bold(self, s: &str) -> String {
        self.wrap(crate::term::BOLD, s)
    }
    pub fn dim(self, s: &str) -> String {
        self.wrap(crate::term::MUTED, s)
    }
    pub fn ok(self, s: &str) -> String {
        self.wrap(crate::term::SUCCESS, s)
    }
    pub fn warn(self, s: &str) -> String {
        self.wrap(crate::term::YELLOW, s)
    }
    pub fn bad(self, s: &str) -> String {
        self.wrap(crate::term::DANGER, s)
    }
}

/// `3h 4m`, `4m 5s`, `9s`.
pub(super) fn duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// Requests answered without error. `requests` counts every request frame
/// received, so failed and in-flight ones come off it: a device whose local
/// server is down must not look like it served anything.
fn served(requests: u64, in_flight: u64, errors: u64) -> u64 {
    requests.saturating_sub(in_flight.saturating_add(errors))
}

/// `1234567` -> `1,234,567`.
pub(super) fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn conn_badge(conn: &Conn, now: u64, p: Paint) -> String {
    match conn {
        Conn::Connected => p.ok("● connected"),
        Conn::Connecting => p.warn("◌ connecting"),
        Conn::Reconnecting { attempt, retry_at } => p.warn(&format!(
            "○ reconnecting in {} (attempt {attempt})",
            duration(retry_at.saturating_sub(now))
        )),
        Conn::Stopping => p.dim("■ stopping"),
    }
}

pub(super) fn pool_text(on: bool) -> &'static str {
    if on {
        "on: idle capacity serves the shared pool and earns credits"
    } else {
        "off: only your own account uses this device"
    }
}

fn row(p: Paint, label: &str, value: &str) -> String {
    format!("  {}  {value}\n", p.dim(&format!("{label:<10}")))
}

/// The status block shared by `relay status` and the live screen.
pub(super) fn render_running(s: &Snapshot, now: u64, p: Paint) -> String {
    let mut out = format!(
        "{}  {}\n",
        p.bold("AnyRouter relay"),
        conn_badge(&s.conn, now, p)
    );
    let mode = if s.detached {
        "background"
    } else {
        "foreground"
    };
    out += &row(
        p,
        "Device",
        &format!(
            "{}  {}",
            s.device,
            p.dim(&format!(
                "pid {}, up {}, {mode}",
                s.pid,
                duration(now.saturating_sub(s.started_at))
            ))
        ),
    );
    out += &row(
        p,
        "Models",
        &if s.models.is_empty() {
            p.warn("none advertised")
        } else {
            s.models.join(", ")
        },
    );
    for (i, (label, base)) in s.endpoints.iter().enumerate() {
        out += &row(
            p,
            if i == 0 { "Endpoints" } else { "" },
            &format!("{label}  {}", p.dim(base)),
        );
    }
    let errors = if s.errors > 0 {
        p.bad(&format!("{} errors", s.errors))
    } else {
        "0 errors".to_string()
    };
    out += &row(
        p,
        "Requests",
        &format!(
            "{} served · {} in flight · {errors}",
            thousands(served(s.requests, s.in_flight, s.errors)),
            s.in_flight
        ),
    );
    out += &row(
        p,
        "Tokens",
        &s.tokens.map(thousands).unwrap_or_else(|| "—".into()),
    );
    out += &row(p, "Pool", pool_text(s.pool));
    if let (Some(err), false) = (&s.last_error, s.conn == Conn::Connected) {
        out += &row(p, "Last error", &p.bad(err));
    }
    out
}

/// What `relay status` knows when no relay is running.
pub(super) struct Idle<'a> {
    pub credential: Option<&'a str>,
    pub pool: (bool, PoolSource),
    pub models: Option<&'a [String]>,
    pub unresponsive_pid: Option<u32>,
}

pub(super) fn render_idle(i: &Idle<'_>, p: Paint) -> String {
    let badge = match i.unresponsive_pid {
        Some(pid) => p.bad(&format!("▲ not responding (pid {pid})")),
        None => p.dim("■ not running"),
    };
    let mut out = format!("{}  {badge}\n", p.bold("AnyRouter relay"));
    out += &row(
        p,
        "Credential",
        &match i.credential {
            Some(kind) => kind.to_string(),
            None => p.warn("none"),
        },
    );
    out += &row(
        p,
        "Pool",
        &format!(
            "{}  {}",
            pool_text(i.pool.0),
            p.dim(&format!("({})", i.pool.1.name()))
        ),
    );
    out += &row(
        p,
        "Models",
        &match i.models {
            Some([]) => "none (all on-device models disabled)".to_string(),
            Some(m) => m.join(", "),
            None => "every on-device model it can serve".to_string(),
        },
    );
    out
}

/// The one command to run next, given where the user stands.
pub(super) fn next_step(
    bin: &str,
    has_credential: bool,
    running: Option<&Snapshot>,
    unresponsive: bool,
) -> String {
    if unresponsive {
        return format!("`{bin} relay stop`, then `{bin} relay start`");
    }
    match running {
        None if !has_credential => format!("`{bin} login`, then `{bin} relay start`"),
        None => format!("`{bin} relay start` (add --detach to run in the background)"),
        Some(s) if matches!(s.conn, Conn::Reconnecting { .. }) => {
            if s.detached {
                format!("`{bin} relay logs` or `{bin} relay doctor`")
            } else {
                format!("`{bin} relay doctor`")
            }
        }
        Some(s) if s.models.is_empty() => format!("`{bin} relay doctor`"),
        Some(_) => format!("`{bin} relay stop` to stop it"),
    }
}

pub(super) fn render_next(next: &str, p: Paint) -> String {
    format!("\n{} {next}\n", p.dim("Next:"))
}

/// Footer for the live screen: key hints plus recent events.
pub(super) fn render_live_footer(events: &[String], p: Paint) -> String {
    let mut out = String::new();
    if !events.is_empty() {
        out.push('\n');
        for e in events {
            out += &format!("  {}\n", p.dim(e));
        }
    }
    out += &format!(
        "\n  {}\n",
        p.dim("p pool on/off · r rescan · q quit (Ctrl-C)")
    );
    out
}

/// One row of `relay models`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModelRow {
    pub id: String,
    pub source: String,
    /// None = ready; Some(why) = not reachable now.
    pub unavailable: Option<String>,
    /// Saved choice says serve it.
    pub enabled: bool,
}

pub(super) fn render_models(rows: &[ModelRow], p: Paint) -> String {
    let w = rows.iter().map(|r| r.id.len()).max().unwrap_or(0);
    let sw = rows.iter().map(|r| r.source.len()).max().unwrap_or(0);
    let mut out = String::new();
    for r in rows {
        let mark = if r.enabled { p.ok("✓") } else { p.dim("·") };
        let state = match &r.unavailable {
            None => p.ok("ready"),
            Some(why) => p.warn(why),
        };
        out += &format!(
            "  {mark} {:<w$}  {}  {state}\n",
            r.id,
            p.dim(&format!("{:<sw$}", r.source)),
        );
    }
    out
}

/// `error: what` / `  why` / `hint: fix` — the shape every relay error uses.
pub(super) fn error(what: &str, why: &str, fix: &str) -> String {
    let mut out = format!("error: {what}");
    if !why.is_empty() {
        out += &format!("\n  {why}");
    }
    if !fix.is_empty() {
        out += &format!("\nhint: {fix}");
    }
    out
}

/// `2026-10-24T09:05:07Z` from unix seconds, for log lines.
pub(super) fn utc_stamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::super::state::sample_snapshot;
    use super::*;

    const NO: Paint = Paint(false);

    #[test]
    fn served_leaves_out_failed_and_in_flight_requests() {
        // Live: 1 answered + 3 failed showed "4 served · 3 errors".
        assert_eq!(served(4, 0, 3), 1);
        assert_eq!(served(5, 2, 0), 3);
        assert_eq!(served(1, 1, 1), 0);
    }

    #[test]
    fn running_status_snapshot() {
        let s = sample_snapshot();
        assert_eq!(
            render_running(&s, 1_200, NO),
            "\
AnyRouter relay  ● connected
  Device      Desk Mac  pid 4242, up 3m 20s, background
  Models      foundation-model, pplx-decider-flash-v1.2
  Endpoints   fm serve (Apple Foundation Models)  http://127.0.0.1:1976/v1
  Requests    11 served · 1 in flight · 0 errors
  Tokens      3,456
  Pool        on: idle capacity serves the shared pool and earns credits
"
        );
    }

    #[test]
    fn reconnecting_status_shows_countdown_unknown_tokens_and_the_error() {
        let mut s = sample_snapshot();
        s.conn = Conn::Reconnecting {
            attempt: 3,
            retry_at: 1_204,
        };
        s.tokens = None;
        s.pool = false;
        s.errors = 2;
        s.last_error = Some("could not connect to relay: refused".into());
        assert_eq!(
            render_running(&s, 1_200, NO),
            "\
AnyRouter relay  ○ reconnecting in 4s (attempt 3)
  Device      Desk Mac  pid 4242, up 3m 20s, background
  Models      foundation-model, pplx-decider-flash-v1.2
  Endpoints   fm serve (Apple Foundation Models)  http://127.0.0.1:1976/v1
  Requests    9 served · 1 in flight · 2 errors
  Tokens      —
  Pool        off: only your own account uses this device
  Last error  could not connect to relay: refused
"
        );
    }

    #[test]
    fn idle_status_snapshot() {
        let saved = vec!["apple/foundation-model".to_string()];
        let out = render_idle(
            &Idle {
                credential: Some("AnyRouter key (profile default)"),
                pool: (false, PoolSource::Saved),
                models: Some(&saved),
                unresponsive_pid: None,
            },
            NO,
        );
        assert_eq!(
            out,
            "\
AnyRouter relay  ■ not running
  Credential  AnyRouter key (profile default)
  Pool        off: only your own account uses this device  (saved)
  Models      apple/foundation-model
"
        );
    }

    #[test]
    fn color_off_means_no_escape_codes_anywhere() {
        let s = sample_snapshot();
        let all = render_running(&s, 1_200, NO) + &render_live_footer(&["x".into()], NO);
        assert!(!all.contains('\x1b'), "{all:?}");
        assert!(render_running(&s, 1_200, Paint(true)).contains('\x1b'));
    }

    #[test]
    fn next_step_points_at_the_one_command_that_unblocks_the_user() {
        let mut s = sample_snapshot();
        assert!(next_step("anyr", false, None, false).contains("anyr login"));
        assert!(next_step("anyr", true, None, false).contains("anyr relay start"));
        assert!(next_step("anyr", true, None, true).contains("anyr relay stop"));
        assert!(next_step("anyr", true, Some(&s), false).contains("relay stop"));
        s.models.clear();
        assert!(next_step("anyr", true, Some(&s), false).contains("relay doctor"));
        s.conn = Conn::Reconnecting {
            attempt: 1,
            retry_at: 0,
        };
        assert!(next_step("anyr", true, Some(&s), false).contains("relay logs"));
    }

    #[test]
    fn models_table_aligns_and_marks_enabled_rows() {
        let rows = vec![
            ModelRow {
                id: "apple/foundation-model".into(),
                source: "Apple Foundation Models".into(),
                unavailable: Some("run `fm serve`".into()),
                enabled: true,
            },
            ModelRow {
                id: "llama3".into(),
                source: "Ollama".into(),
                unavailable: None,
                enabled: true,
            },
        ];
        assert_eq!(
            render_models(&rows, NO),
            "  ✓ apple/foundation-model  Apple Foundation Models  run `fm serve`\n  ✓ llama3                  Ollama                   ready\n"
        );
    }

    #[test]
    fn error_shape_is_what_why_hint() {
        assert_eq!(
            error("relay is not running", "no pidfile", "anyr relay start"),
            "error: relay is not running\n  no pidfile\nhint: anyr relay start"
        );
        assert_eq!(error("x", "", ""), "error: x");
    }

    #[test]
    fn helpers_format_numbers_durations_and_timestamps() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(duration(9), "9s");
        assert_eq!(duration(3_725), "1h 2m");
        assert_eq!(utc_stamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_stamp(1_792_833_907), "2026-10-24T09:25:07Z");
    }
}
