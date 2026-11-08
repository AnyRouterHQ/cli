//! `anyr relay doctor`: one place that says why nothing is served, with the
//! exact fix for each failure. Probes sit behind `Probe` so the checks are
//! testable off-Mac and offline.

use serde_json::{json, Value};

use super::servers::{self, Absent};
use super::view::Paint;
use super::OnDeviceModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Level {
    Ok,
    /// Optional or informational: does not fail the doctor run.
    Warn,
    Fail,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Check {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
    /// Empty when there is nothing to fix.
    pub fix: String,
    /// For an on-device model row: (catalog id, state). `running` or an
    /// [`Absent`] name.
    pub model: Option<(&'static str, &'static str)>,
}

/// What the cloud said about the stored credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Auth {
    None,
    /// rk_ pairing token: no cheap way to verify it without connecting.
    Token,
    KeyOk,
    KeyRejected(u16),
    /// The check itself failed (offline, 5xx).
    Unverified(String),
}

pub(super) trait Probe {
    fn is_mac(&self) -> bool;
    fn macos_major(&self) -> Option<u32>;
    /// None = not listening; else fm serve's own model ids (system, pcc).
    fn fm_serve(&self) -> Option<Vec<String>>;
    /// The in-process Apple system model is available (macOS 26+ helper).
    fn in_process_foundation(&self) -> bool;
    fn decider(&self) -> bool;
    /// The server's app or command is on this machine (not whether it runs).
    fn installed(&self, server: OnDeviceModel) -> bool;
    /// Labels of other local servers that answered (Ollama, LM Studio, …).
    fn other_servers(&self) -> Vec<String>;
    fn network(&self) -> Result<(), String>;
    fn auth(&self) -> Auth;
    fn relay_pid(&self) -> Option<u32>;
}

fn check(
    name: &'static str,
    level: Level,
    detail: impl Into<String>,
    fix: impl Into<String>,
) -> Check {
    Check {
        name,
        level,
        detail: detail.into(),
        fix: fix.into(),
        model: None,
    }
}

fn for_model(mut c: Check, model: OnDeviceModel, state: &'static str) -> Check {
    c.model = Some((model.id(), state));
    c
}

/// The check for one on-device model that is not answering.
fn absent_check(
    name: &'static str,
    model: OnDeviceModel,
    why: Absent,
    bin: &str,
    fix_off_mac: &str,
) -> Check {
    let spec = servers::spec(model);
    let (detail, fix) = match why {
        Absent::NeedsMac => (spec.next_step(why), fix_off_mac.to_string()),
        Absent::NeedsNewerMacos => (
            format!("needs macOS {} or later", spec.min_macos.unwrap_or(26)),
            format!(
                "{} (System Settings > General > Software Update)",
                spec.next_step(why)
            ),
        ),
        Absent::NotInstalled => ("not installed".to_string(), spec.install_hint.to_string()),
        Absent::Stopped => (
            "installed, not running".to_string(),
            format!("`{bin} relay start` launches it, or: {}", spec.enable_hint),
        ),
    };
    for_model(check(name, Level::Warn, detail, fix), model, why.name())
}

pub(super) fn run_checks(p: &dyn Probe, bin: &str) -> Vec<Check> {
    let mut out = Vec::new();
    let mac = p.is_mac();
    let major = p.macos_major();

    let apple_ok = match (mac, major) {
        (false, _) => {
            out.push(check(
                "macOS",
                Level::Warn,
                "not a Mac: Apple Foundation Models are unavailable here",
                "serve Ollama or LM Studio, or pass --target <url>",
            ));
            false
        }
        (true, Some(v)) if v < 26 => {
            out.push(check(
                "macOS",
                Level::Warn,
                format!("macOS {v}: Apple Foundation Models need macOS 26 or later"),
                "update in System Settings > General > Software Update",
            ));
            false
        }
        (true, v) => {
            out.push(check(
                "macOS",
                Level::Ok,
                v.map(|v| format!("macOS {v}"))
                    .unwrap_or_else(|| "macOS".into()),
                "",
            ));
            true
        }
    };

    let fm_ids = if apple_ok { p.fm_serve() } else { None };
    let in_process = apple_ok && p.in_process_foundation();
    let fm = fm_ids.is_some() || in_process;
    let off_mac_fix = "serve Ollama or LM Studio, or pass --target <url>";
    let apple = OnDeviceModel::AppleFoundation;
    if !apple_ok {
        let why = if mac {
            Absent::NeedsNewerMacos
        } else {
            Absent::NeedsMac
        };
        let c = absent_check("fm serve", apple, why, bin, off_mac_fix);
        out.push(c);
    } else {
        out.push(match &fm_ids {
            Some(ids) if ids.is_empty() => for_model(
                check(
                    "fm serve",
                    Level::Ok,
                    "listening on 127.0.0.1:1976 (no models listed)",
                    "",
                ),
                apple,
                "running",
            ),
            Some(ids) => for_model(
                check(
                    "fm serve",
                    Level::Ok,
                    format!("listening on 127.0.0.1:1976, models: {}", ids.join(", ")),
                    "",
                ),
                apple,
                "running",
            ),
            None if in_process => for_model(
                check(
                    "fm serve",
                    Level::Ok,
                    "Apple system model serving in-process (foundation-model); fm serve not needed",
                    "",
                ),
                apple,
                "running",
            ),
            None if p.installed(apple) => {
                absent_check("fm serve", apple, Absent::Stopped, bin, off_mac_fix)
            }
            None => absent_check("fm serve", apple, Absent::NotInstalled, bin, off_mac_fix),
        });
    }

    let decider = p.decider();
    let dm = OnDeviceModel::DeciderFlash;
    out.push(if decider {
        for_model(
            check("Decider app", Level::Ok, "answering on 127.0.0.1:8080", ""),
            dm,
            "running",
        )
    } else if !mac {
        absent_check("Decider app", dm, Absent::NeedsMac, bin, off_mac_fix)
    } else if p.installed(dm) {
        absent_check("Decider app", dm, Absent::Stopped, bin, off_mac_fix)
    } else {
        absent_check("Decider app", dm, Absent::NotInstalled, bin, off_mac_fix)
    });

    let others = p.other_servers();
    if !others.is_empty() {
        out.push(check("Local servers", Level::Ok, others.join(", "), ""));
    }
    if !fm && !decider && others.is_empty() {
        out.push(check(
            "Models",
            Level::Fail,
            "nothing on this machine can be served",
            "run `fm serve`, open the Decider app, or start Ollama / LM Studio",
        ));
    }

    out.push(match p.network() {
        Ok(()) => check("Network", Level::Ok, "anyrouter.dev reachable", ""),
        Err(e) => check(
            "Network",
            Level::Fail,
            format!(
                "cannot reach anyrouter.dev: {}",
                crate::exit::classify(&e).1
            ),
            "check your connection, VPN or HTTPS_PROXY",
        ),
    });

    out.push(match p.auth() {
        Auth::None => check(
            "Auth",
            Level::Fail,
            "no AnyRouter key or relay token",
            format!("`{bin} login`"),
        ),
        Auth::Token => check("Auth", Level::Ok, "relay pairing token stored", ""),
        Auth::KeyOk => check("Auth", Level::Ok, "AnyRouter key accepted", ""),
        Auth::KeyRejected(code) => check(
            "Auth",
            Level::Fail,
            format!("AnyRouter key rejected ({code})"),
            format!("`{bin} login` for a fresh key"),
        ),
        Auth::Unverified(e) => check(
            "Auth",
            Level::Warn,
            format!(
                "key stored but not verified: {}",
                crate::exit::classify(&e).1
            ),
            format!("re-run `{bin} relay doctor` once online"),
        ),
    });

    out.push(match p.relay_pid() {
        Some(pid) => check("Relay", Level::Ok, format!("running (pid {pid})"), ""),
        None => check(
            "Relay",
            Level::Warn,
            "not running",
            format!("`{bin} relay start --detach`"),
        ),
    });
    out
}

pub(super) fn failed(checks: &[Check]) -> bool {
    checks.iter().any(|c| c.level == Level::Fail)
}

pub(super) fn render(checks: &[Check], p: Paint) -> String {
    let w = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut out = String::new();
    for c in checks {
        let mark = match c.level {
            Level::Ok => p.ok("✓"),
            Level::Warn => p.warn("!"),
            Level::Fail => p.bad("✗"),
        };
        out += &format!("  {mark} {:<w$}  {}\n", c.name, c.detail);
        if !c.fix.is_empty() {
            out += &format!("    {:<w$}  {} {}\n", "", p.dim("fix:"), c.fix);
        }
    }
    out
}

pub(super) fn to_json(checks: &[Check]) -> Value {
    json!({
        "ok": !failed(checks),
        "checks": checks.iter().map(|c| json!({
            "name": c.name,
            "status": c.level.name(),
            "detail": c.detail,
            "fix": if c.fix.is_empty() { Value::Null } else { json!(c.fix) },
        })).collect::<Vec<_>>(),
        "models": checks.iter().filter_map(|c| c.model.map(|(id, state)| json!({
            "id": id,
            "state": state,
            "fix": if c.fix.is_empty() { Value::Null } else { json!(c.fix) },
        }))).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        mac: bool,
        major: Option<u32>,
        fm: Option<Vec<String>>,
        in_process: bool,
        decider: bool,
        installed: bool,
        others: Vec<String>,
        net: Result<(), String>,
        auth: Auth,
        pid: Option<u32>,
    }

    impl Default for Fake {
        fn default() -> Self {
            Fake {
                mac: true,
                major: Some(26),
                fm: Some(vec!["system".into(), "pcc".into()]),
                in_process: false,
                decider: true,
                installed: true,
                others: vec![],
                net: Ok(()),
                auth: Auth::KeyOk,
                pid: Some(42),
            }
        }
    }

    impl Probe for Fake {
        fn is_mac(&self) -> bool {
            self.mac
        }
        fn macos_major(&self) -> Option<u32> {
            self.major
        }
        fn fm_serve(&self) -> Option<Vec<String>> {
            self.fm.clone()
        }
        fn in_process_foundation(&self) -> bool {
            self.in_process
        }
        fn decider(&self) -> bool {
            self.decider
        }
        fn installed(&self, _: OnDeviceModel) -> bool {
            self.installed
        }
        fn other_servers(&self) -> Vec<String> {
            self.others.clone()
        }
        fn network(&self) -> Result<(), String> {
            self.net.clone()
        }
        fn auth(&self) -> Auth {
            self.auth.clone()
        }
        fn relay_pid(&self) -> Option<u32> {
            self.pid
        }
    }

    fn get<'a>(checks: &'a [Check], name: &str) -> &'a Check {
        checks.iter().find(|c| c.name == name).unwrap()
    }

    #[test]
    fn healthy_mac_passes_every_check() {
        let c = run_checks(&Fake::default(), "anyr");
        assert!(!failed(&c));
        assert!(c.iter().all(|c| c.level == Level::Ok), "{c:?}");
        // WHY: fm serve names its model `system`, not the catalog id; the
        // doctor must show the real ids so a mismatch is visible.
        assert!(get(&c, "fm serve").detail.contains("system, pcc"));
    }

    #[test]
    fn old_macos_skips_fm_serve_and_says_how_to_update() {
        let c = run_checks(
            &Fake {
                major: Some(15),
                ..Fake::default()
            },
            "anyr",
        );
        assert!(get(&c, "macOS").fix.contains("Software Update"));
        assert_eq!(get(&c, "fm serve").model.unwrap().1, "needs_newer_macos");
        assert!(get(&c, "fm serve").detail.contains("macOS 26"));
        assert!(get(&c, "fm serve")
            .fix
            .contains("update to macOS 26 or later"));
        // Decider still serves, so this is not a failure.
        assert!(!failed(&c));
    }

    #[test]
    fn non_mac_is_needs_mac_not_needs_newer_macos() {
        let c = run_checks(
            &Fake {
                mac: false,
                major: None,
                ..Fake::default()
            },
            "anyr",
        );
        assert_eq!(get(&c, "fm serve").model.unwrap().1, "needs_mac");
    }

    #[test]
    fn nothing_servable_fails_with_a_fix_for_each_source() {
        let c = run_checks(
            &Fake {
                fm: None,
                decider: false,
                ..Fake::default()
            },
            "anyr",
        );
        assert!(failed(&c));
        assert!(get(&c, "fm serve").fix.contains("fm serve"));
        assert!(get(&c, "Decider app").fix.contains("Decider"));
        assert_eq!(get(&c, "Models").level, Level::Fail);
    }

    fn decider_state(c: &[Check]) -> (&'static str, String, String) {
        let d = get(c, "Decider app");
        (d.model.unwrap().1, d.detail.clone(), d.fix.clone())
    }

    #[test]
    fn decider_states_each_end_in_a_fix() {
        // WHY: stopped, not installed and off-Mac need three different next
        // steps; a bare "not answering" left the owner guessing.
        let stopped = run_checks(
            &Fake {
                decider: false,
                ..Fake::default()
            },
            "anyr",
        );
        let (st, _, fix) = decider_state(&stopped);
        assert_eq!(st, "stopped");
        assert!(fix.contains("`anyr relay start` launches it"), "{fix}");

        let missing = run_checks(
            &Fake {
                decider: false,
                installed: false,
                ..Fake::default()
            },
            "anyr",
        );
        let (st, detail, fix) = decider_state(&missing);
        assert_eq!((st, detail.as_str()), ("not_installed", "not installed"));
        assert!(fix.contains("local-relay"), "{fix}");

        let linux = run_checks(
            &Fake {
                mac: false,
                major: None,
                fm: None,
                decider: false,
                ..Fake::default()
            },
            "anyr",
        );
        let (st, detail, _) = decider_state(&linux);
        assert_eq!((st, detail.as_str()), ("needs_mac", "needs a Mac"));
    }

    #[test]
    fn json_lists_each_model_with_state_and_fix() {
        let c = run_checks(
            &Fake {
                decider: false,
                installed: false,
                ..Fake::default()
            },
            "anyr",
        );
        let j = to_json(&c);
        let models = j["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["id"], "apple/foundation-model");
        assert_eq!(models[0]["state"], "running");
        assert_eq!(models[1]["id"], "perplexity/pplx-decider-flash-v1.2");
        assert_eq!(models[1]["state"], "not_installed");
        assert!(models[1]["fix"].as_str().unwrap().contains("Decider"));
    }

    #[test]
    fn linux_with_ollama_is_healthy() {
        let c = run_checks(
            &Fake {
                mac: false,
                major: None,
                fm: None,
                decider: false,
                others: vec!["Ollama".into()],
                ..Fake::default()
            },
            "anyr",
        );
        assert!(!failed(&c), "{c:?}");
        assert_eq!(get(&c, "macOS").level, Level::Warn);
    }

    #[test]
    fn auth_and_network_failures_name_the_fix_command() {
        let c = run_checks(
            &Fake {
                auth: Auth::None,
                net: Err("dns".into()),
                pid: None,
                ..Fake::default()
            },
            "anyr",
        );
        assert!(failed(&c));
        assert_eq!(get(&c, "Auth").fix, "`anyr login`");
        assert!(get(&c, "Network").detail.contains("dns"));
        assert!(get(&c, "Relay").fix.contains("relay start --detach"));
        let c = run_checks(
            &Fake {
                auth: Auth::KeyRejected(401),
                ..Fake::default()
            },
            "anyr",
        );
        assert!(get(&c, "Auth").detail.contains("401"));
        // Offline auth check is a warning, not a hard failure.
        let c = run_checks(
            &Fake {
                auth: Auth::Unverified("timeout".into()),
                ..Fake::default()
            },
            "anyr",
        );
        assert_eq!(get(&c, "Auth").level, Level::Warn);
    }

    #[test]
    fn render_and_json_snapshot() {
        let c = vec![
            check("macOS", Level::Ok, "macOS 26", ""),
            check("Auth", Level::Fail, "no key", "`anyr login`"),
        ];
        assert_eq!(
            render(&c, Paint(false)),
            "  ✓ macOS  macOS 26\n  ✗ Auth   no key\n           fix: `anyr login`\n"
        );
        let j = to_json(&c);
        assert_eq!(j["ok"], false);
        assert_eq!(j["checks"][0]["fix"], Value::Null);
        assert_eq!(j["checks"][1]["status"], "fail");
    }

    #[test]
    fn in_process_foundation_counts_as_served() {
        let c = run_checks(
            &Fake {
                fm: None,
                in_process: true,
                decider: false,
                installed: false,
                ..Fake::default()
            },
            "anyr",
        );
        assert!(!failed(&c), "{c:?}");
        let check = get(&c, "fm serve");
        assert_eq!(check.level, Level::Ok);
        assert!(check.detail.contains("in-process"), "{}", check.detail);
        assert_eq!(check.model.unwrap().1, "running");
    }
}
