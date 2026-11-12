//! `anyr telemetry status|on|off|reset`: show and change what the anonymous
//! pings send. Runs without the startup ping, so `telemetry off` as the first
//! command ever sends nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::cmd::dispatch::USAGE;
use crate::parse::ParsedArgs;
use crate::telemetry::{self as t, Kind};

pub(crate) fn run(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let json_out = parsed.flag_true("json");
    let sub = parsed.passthrough.first().map(String::as_str);
    let out = match sub {
        None | Some("status") if parsed.flag_true("dry-run") => samples(env),
        None | Some("status") => status(env),
        Some("on") => set(env, true)?,
        Some("off") => set(env, false)?,
        Some("reset") => reset(env),
        Some(other) => {
            return Err(format!(
                "{USAGE}unknown telemetry subcommand \"{other}\". Use: {} telemetry [status|on|off|reset]",
                crate::help::invoked_bin()
            ))
        }
    };
    if json_out {
        println!("{out}");
    } else {
        print!("{}", text(&out));
    }
    Ok(0)
}

fn status(env: &BTreeMap<String, String>) -> Value {
    let st = t::status(env);
    let id = t::load_state(env).install_id;
    // While off nothing is sent, so there is no next payload to show.
    let next = (st.on && id.is_some())
        .then(|| event(&st, id.as_deref().unwrap_or_default(), &Kind::Daily));
    json!({
        "enabled": st.on,
        "reason": st.why,
        "install_id": id,
        "endpoint": st.url,
        "next_daily": next,
    })
}

fn event(st: &t::Status, id: &str, kind: &Kind) -> Value {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    if st.otlp {
        t::otlp_payload(id, kind, crate::VERSION, os, arch)
    } else {
        t::payload(id, kind, crate::VERSION, os, arch)
    }
}

/// One sample of every event type, with this install's id when it has one.
fn samples(env: &BTreeMap<String, String>) -> Value {
    let st = t::status(env);
    let id = t::load_state(env)
        .install_id
        .unwrap_or_else(|| "0".repeat(32));
    let kinds = [
        Kind::Install,
        Kind::Update {
            from: "0.1.0".into(),
        },
        Kind::Daily,
    ];
    json!({
        "enabled": st.on,
        "endpoint": st.url,
        "events": kinds.iter().map(|k| event(&st, &id, k)).collect::<Vec<_>>(),
    })
}

fn set(env: &BTreeMap<String, String>, on: bool) -> Result<Value, String> {
    let path = crate::config::resolve_config_path(None, env);
    let mut cfg = crate::key::load_config_if_present(&path).unwrap_or_default();
    cfg.telemetry = Some(on);
    crate::config::write_config(&cfg, &path)?;
    let st = t::status(env);
    Ok(json!({ "enabled": st.on, "reason": st.why }))
}

/// A new random id while on; while off the old id is deleted and none is
/// written.
fn reset(env: &BTreeMap<String, String>) -> Value {
    if t::status(env).on {
        let mut state = t::load_state(env);
        state.install_id = Some(t::new_install_id());
        t::save(env, &state);
        json!({ "install_id": state.install_id })
    } else {
        let _ = std::fs::remove_file(t::state_path(env));
        json!({ "install_id": null })
    }
}

fn text(v: &Value) -> String {
    let bin = crate::help::invoked_bin();
    let pretty = |v: &Value| serde_json::to_string_pretty(v).unwrap_or_default();
    if let Some(events) = v.get("events").and_then(Value::as_array) {
        let mut out = format!(
            "Samples of every event (sent to {}):\n",
            v["endpoint"].as_str().unwrap_or_default()
        );
        for e in events {
            out.push_str(&pretty(e));
            out.push('\n');
        }
        return out;
    }
    if v.get("endpoint").is_none() {
        // on / off / reset
        return match (&v["enabled"], &v["install_id"]) {
            (Value::Bool(on), _) => {
                let mut line = format!("Telemetry is {}", if *on { "on" } else { "off" });
                if !on && v["reason"] != "telemetry: false in the config" {
                    line.push_str(&format!(" ({})", v["reason"].as_str().unwrap_or_default()));
                }
                line + ".\n"
            }
            (_, Value::String(id)) => format!("New install id: {id}\n"),
            _ => "Telemetry is off; the install id was removed.\n".into(),
        };
    }
    let on = v["enabled"] == true;
    let mut out = format!(
        "Telemetry: {} ({})\nInstall id: {}\nEndpoint: {}\n",
        if on { "on" } else { "off" },
        v["reason"].as_str().unwrap_or_default(),
        v["install_id"].as_str().unwrap_or("none yet"),
        v["endpoint"].as_str().unwrap_or_default(),
    );
    match v.get("next_daily").filter(|n| !n.is_null()) {
        Some(next) => out.push_str(&format!("Next daily ping:\n{}\n", pretty(next))),
        None if on => out.push_str("Next daily ping: none yet (no install id)\n"),
        None => out.push_str("Nothing is sent while it is off.\n"),
    }
    out.push_str(&format!(
        "Every event: `{bin} telemetry status --dry-run`.\n"
    ));
    if on {
        out.push_str(&format!("Turn off: `{bin} telemetry off`.\n"));
    } else if v["reason"] == "telemetry: false in the config" {
        out.push_str(&format!("Turn on: `{bin} telemetry on`.\n"));
    }
    out
}
