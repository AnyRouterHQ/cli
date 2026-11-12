//! `anyr relay devices` and `anyr relay models --devices`: the account's paired
//! devices, from `/v1/relay/devices`. Rendering is pure so tests compare strings.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::api::{context, encode, get, send, Ctx};
use crate::cmd::view::{print_json, print_table, text};
use crate::parse::ParsedArgs;

use super::view::duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Devices {
    List,
    Rename { id: String, name: String },
    Pool { id: String, on: bool },
    Rm { id: String, yes: bool },
}

/// Server caps names at 64 characters and rejects empty ones.
const MAX_NAME: usize = 64;

pub(super) fn parse(words: &[&str], yes: bool) -> Result<Devices, String> {
    let need_id = |verb: &str, shape: &str| {
        words.get(1).map(|s| s.to_string()).ok_or_else(|| {
            format!("`relay devices {verb}` needs a device id: `{{bin}} relay devices {shape}`.")
        })
    };
    let extra = |n: usize, verb: &str| match words.get(n) {
        Some(w) => Err(format!(
            "Unexpected argument \"{w}\" for `relay devices {verb}`."
        )),
        None => Ok(()),
    };
    match words.first().copied() {
        None | Some("list") | Some("ls") => {
            extra(1, "list")?;
            Ok(Devices::List)
        }
        Some("rename") => {
            let id = need_id("rename", "rename <id> \"Studio Mac\"")?;
            let name = words.get(2).map(|s| s.trim()).unwrap_or("");
            if name.is_empty() {
                return Err("`relay devices rename` needs a new name: `{bin} relay devices rename <id> \"Studio Mac\"`.".into());
            }
            if name.chars().count() > MAX_NAME {
                return Err(format!("Device names are at most {MAX_NAME} characters."));
            }
            extra(3, "rename")?;
            Ok(Devices::Rename {
                id,
                name: name.to_string(),
            })
        }
        Some("pool") => {
            let id = need_id("pool", "pool <id> on|off")?;
            let on = match words.get(2).copied() {
                Some("on") => true,
                Some("off") => false,
                _ => return Err(
                    "`relay devices pool` takes on or off: `{bin} relay devices pool <id> off`."
                        .into(),
                ),
            };
            extra(3, "pool")?;
            Ok(Devices::Pool { id, on })
        }
        Some("rm") | Some("remove") => {
            let id = need_id("rm", "rm <id> --yes")?;
            extra(2, "rm")?;
            Ok(Devices::Rm { id, yes })
        }
        Some(other) => Err(format!(
            "Unknown `relay devices` action \"{other}\" (use list, rename, pool or rm)."
        )),
    }
}

pub(super) fn run(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    cmd: Devices,
) -> Result<i32, String> {
    let ctx = context(parsed, env);
    match cmd {
        Devices::List => {
            let body = fetch(&ctx)?;
            if ctx.json {
                print_json(&body);
            } else {
                print_devices(&body);
            }
        }
        Devices::Rename { id, name } => {
            let body = patch(&ctx, &id, &json!({ "name": name }))?;
            if ctx.json {
                print_json(&body);
            } else {
                println!("Renamed {id} to \"{name}\".");
            }
        }
        Devices::Pool { id, on } => {
            let body = patch(&ctx, &id, &json!({ "pool_enabled": on }))?;
            if ctx.json {
                print_json(&body);
            } else {
                println!("Pool sharing {} for {id}.", if on { "on" } else { "off" });
            }
        }
        Devices::Rm { id, yes } => {
            let question = format!("Remove device {id} from your AnyRouter account?");
            if !yes {
                if !crate::term::is_interactive() {
                    return Err(crate::exit::confirm_required(&question));
                }
                if !crate::term::confirm(&question) {
                    return Err("Cancelled. Nothing was removed.".into());
                }
            }
            // DELETE is gated by requireAuth (session or management key), not by an
            // inference key, so prefer the management key when one is configured.
            let dctx = Ctx {
                key: ctx.management_key.clone().or_else(|| ctx.key.clone()),
                management_key: None,
                base: ctx.base.clone(),
                json: ctx.json,
            };
            let body = send(
                &dctx,
                "DELETE",
                &format!("/v1/relay/devices/{}", encode(&id)),
                None,
                true,
            )?;
            if ctx.json {
                print_json(&body);
            } else {
                println!("Removed {id}.");
            }
        }
    }
    Ok(0)
}

/// `anyr relay models --devices`.
pub(super) fn run_models(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let ctx = context(parsed, env);
    let body = fetch(&ctx)?;
    if ctx.json {
        print_json(&body);
    } else {
        print_model_status(&body);
    }
    Ok(0)
}

fn fetch(ctx: &Ctx) -> Result<Value, String> {
    get(ctx, "/v1/relay/devices", true)
}

fn patch(ctx: &Ctx, id: &str, body: &Value) -> Result<Value, String> {
    send(
        ctx,
        "PATCH",
        &format!("/v1/relay/devices/{}", encode(id)),
        Some(&body.to_string()),
        true,
    )
}

fn active(body: &Value) -> Vec<&Value> {
    body.get("data")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|d| d.get("status").and_then(Value::as_str) != Some("revoked"))
                .collect()
        })
        .unwrap_or_default()
}

fn models_of(d: &Value) -> Vec<&str> {
    d.get("models")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn pool_on(d: &Value) -> bool {
    d.get("pool_enabled").and_then(Value::as_bool) == Some(true)
}

fn connected(d: &Value) -> bool {
    d.get("connected").and_then(Value::as_bool) == Some(true)
}

fn seen(d: &Value, now: u64) -> String {
    match d.get("last_seen_at").and_then(Value::as_u64) {
        Some(t) if t <= now => format!("{} ago", duration(now - t)),
        Some(_) => "just now".into(),
        None => "never".into(),
    }
}

/// Table rows: id, name, state, models, last seen, pool.
pub(super) fn device_rows(body: &Value, now: u64) -> Vec<Vec<String>> {
    active(body)
        .into_iter()
        .map(|d| {
            let models = models_of(d);
            vec![
                text(d.get("id")),
                text(d.get("name")),
                if connected(d) { "online" } else { "offline" }.into(),
                if models.is_empty() {
                    "-".into()
                } else {
                    models.join(", ")
                },
                seen(d, now),
                if pool_on(d) { "on" } else { "off" }.into(),
            ]
        })
        .collect()
}

/// Per advertised model: devices serving it, how many are online, pool-shared.
pub(super) fn model_rows(body: &Value) -> Vec<Vec<String>> {
    let mut per: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for d in active(body) {
        for m in crate::sort::set(models_of(d)) {
            let e = per.entry(m).or_default();
            e.0 += 1;
            e.1 += usize::from(connected(d));
            e.2 += usize::from(pool_on(d));
        }
    }
    per.into_iter()
        .map(|(m, (all, online, pool))| {
            vec![
                m.to_string(),
                all.to_string(),
                online.to_string(),
                pool.to_string(),
            ]
        })
        .collect()
}

fn print_devices(body: &Value) {
    let rows = device_rows(body, super::state::now_secs());
    if rows.is_empty() {
        println!(
            "No paired devices. Pair this one with `{} relay pair`.",
            crate::help::invoked_bin()
        );
        return;
    }
    print_table(
        &["id", "name", "state", "models", "last seen", "pool"],
        &rows,
    );
}

fn print_model_status(body: &Value) {
    let rows = model_rows(body);
    if rows.is_empty() {
        println!(
            "No device advertises a model yet. Start one with `{} relay start`.",
            crate::help::invoked_bin()
        );
        return;
    }
    print_table(&["model", "devices", "online", "pool"], &rows);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<&str> {
        s.split_whitespace().collect()
    }

    #[test]
    fn verbs_parse() {
        assert_eq!(parse(&[], false).unwrap(), Devices::List);
        assert_eq!(parse(&w("ls"), false).unwrap(), Devices::List);
        assert_eq!(
            parse(&["rename", "d1", "Studio Mac"], false).unwrap(),
            Devices::Rename {
                id: "d1".into(),
                name: "Studio Mac".into()
            }
        );
        assert_eq!(
            parse(&w("pool d1 off"), false).unwrap(),
            Devices::Pool {
                id: "d1".into(),
                on: false
            }
        );
        assert_eq!(
            parse(&w("rm d1"), true).unwrap(),
            Devices::Rm {
                id: "d1".into(),
                yes: true
            }
        );
        assert_eq!(
            parse(&w("rm d1"), false).unwrap(),
            Devices::Rm {
                id: "d1".into(),
                yes: false
            }
        );
    }

    #[test]
    fn bad_input_is_rejected() {
        for (argv, needle) in [
            ("rename", "needs a device id"),
            ("rename d1", "needs a new name"),
            ("pool d1", "on or off"),
            ("pool d1 maybe", "on or off"),
            ("rm", "needs a device id"),
            ("rm d1 d2", "Unexpected argument"),
            ("wat", "Unknown `relay devices` action"),
        ] {
            let e = parse(&w(argv), false).unwrap_err();
            assert!(e.contains(needle), "{argv}: {e}");
        }
        assert!(parse(&["rename", "d1", "  "], false).is_err());
        assert!(parse(&["rename", "d1", &"x".repeat(65)], false).is_err());
    }

    fn body() -> Value {
        json!({"object":"list","data":[
            {"id":"d1","name":"Studio Mac","status":"active","connected":true,"pool_enabled":true,
             "models":["apple/foundation-model","x/y"],"last_seen_at":1000},
            {"id":"d2","name":"Laptop","status":"active","connected":false,"pool_enabled":false,
             "models":["apple/foundation-model"],"last_seen_at":null},
            {"id":"d3","name":"Old","status":"revoked","connected":false,"pool_enabled":true,
             "models":["gone/model"],"last_seen_at":5}]})
    }

    #[test]
    fn rows_show_state_pool_and_skip_revoked() {
        let rows = device_rows(&body(), 1090);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][..3], ["d1", "Studio Mac", "online"]);
        assert_eq!(rows[0][3], "apple/foundation-model, x/y");
        assert!(rows[0][4].ends_with("ago"), "{:?}", rows[0]);
        assert_eq!(rows[0][5], "on");
        assert_eq!(rows[1][2], "offline");
        assert_eq!(rows[1][4], "never");
        assert_eq!(rows[1][5], "off");
    }

    #[test]
    fn model_rows_count_devices_online_and_pooled() {
        let rows = model_rows(&body());
        assert_eq!(rows[0], ["apple/foundation-model", "2", "1", "1"]);
        assert_eq!(rows[1], ["x/y", "1", "1", "1"]);
        assert_eq!(rows.len(), 2);
    }
}
