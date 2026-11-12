//! `anyr presets <list|show|create|edit|rm>` and `--preset <slug>` on launch commands.
//!
//! Presets are saved routing configs behind `/api/v1/presets` (management key). At launch
//! a preset is just a model id, `@preset/<slug>`.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::api::{confirm, context, get, management_ctx, request, send, Ctx};
use crate::cmd::dispatch::hint;
use crate::cmd::view::{print_json, print_table, text};
use crate::parse::{get_string_flag, FlagValue, ParsedArgs};
use crate::term;

pub(crate) const SUBCOMMANDS: &[&str] = &["list", "ls", "show", "create", "edit", "rm"];
const SORTS: &[&str] = &["price", "throughput", "latency", "exacto"];
const SCOPES: &str = "read:presets and write:presets";

const USAGE_LINE: &str = "Usage: {bin} presets [list|show <slug>|create <slug> --model a,b|edit <slug>|rm <slug> --yes] [--json]";

fn path_for(slug: &str) -> String {
    format!("/v1/presets/{}", crate::api::encode(slug))
}

/// Same rule as the API (`[a-z0-9][a-z0-9-]*`).
pub(crate) fn valid_slug(slug: &str) -> bool {
    let mut chars = slug.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn check_slug(slug: &str) -> Result<(), String> {
    if valid_slug(slug) {
        Ok(())
    } else {
        Err(hint(&format!(
            "Invalid preset slug \"{slug}\": use lowercase letters, digits and dashes, starting with a letter or digit."
        )))
    }
}

/// Model ids from `--model a,b,c`: trimmed, empty entries dropped, duplicates removed.
fn parse_models(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in raw.split(',').map(str::trim).filter(|m| !m.is_empty()) {
        if !out.iter().any(|x| x == m) {
            out.push(m.to_string());
        }
    }
    out
}

/// What the flags change in a preset config. `None` leaves a field alone.
#[derive(Default)]
struct Changes {
    models: Option<Vec<String>>,
    paid_fallback: Option<bool>,
    sort: Option<String>,
}

impl Changes {
    fn from_flags(parsed: &ParsedArgs) -> Result<Self, String> {
        let models = match get_string_flag(&parsed.flags, "model") {
            Some(raw) => {
                let list = parse_models(&raw);
                if list.is_empty() {
                    return Err(hint("--model needs at least one model id: --model a,b"));
                }
                Some(list)
            }
            None => None,
        };
        let sort = get_string_flag(&parsed.flags, "sort");
        if let Some(s) = &sort {
            if !SORTS.contains(&s.as_str()) {
                return Err(hint(&format!(
                    "--sort must be one of: {} (got \"{s}\")",
                    SORTS.join(", ")
                )));
            }
        }
        Ok(Self {
            models,
            paid_fallback: match parsed.flags.get("fallback") {
                Some(FlagValue::Bool(true)) => Some(true),
                Some(FlagValue::Value(v)) if v == "true" => Some(true),
                Some(FlagValue::Value(v)) if v == "false" => Some(false),
                _ => None,
            },
            sort,
        })
    }

    fn is_empty(&self) -> bool {
        self.models.is_none() && self.paid_fallback.is_none() && self.sort.is_none()
    }

    /// Overlay onto an existing config. Setting models drops v2 graph keys so a
    /// stale `branches` list cannot keep routing to the old models.
    fn apply(&self, config: &mut Map<String, Value>) {
        if let Some(models) = &self.models {
            for k in ["version", "mode", "branches", "ui"] {
                config.remove(k);
            }
            config.insert("model".into(), json!(models[0]));
            if models.len() > 1 {
                config.insert("models".into(), json!(models));
            } else {
                config.remove("models");
            }
        }
        if let Some(p) = self.paid_fallback {
            config.insert("paid_fallback".into(), json!(p));
        }
        if let Some(sort) = &self.sort {
            let provider = config
                .entry("provider")
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(obj) = provider.as_object_mut() {
                obj.insert("sort".into(), json!(sort));
            }
        }
    }
}

/// Preset config JSON for `create`: the models are the ordered fallback chain.
fn build_config(changes: &Changes) -> Value {
    let mut config = Map::new();
    changes.apply(&mut config);
    Value::Object(config)
}

fn models_of(preset: &Value) -> Vec<String> {
    let cfg = preset.get("config").unwrap_or(&Value::Null);
    let strings = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    if let Some(m) = cfg.get("model").and_then(Value::as_str) {
        out.push(m.to_string());
    }
    for m in strings(cfg.get("models")) {
        if !out.contains(&m) {
            out.push(m);
        }
    }
    if out.is_empty() {
        let mut branches: Vec<(i64, String)> = cfg
            .get("branches")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|b| {
                        Some((
                            b.get("priority").and_then(Value::as_i64).unwrap_or(0),
                            b.pointer("/route/model_id")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        crate::sort::sort(&mut branches);
        out = branches.into_iter().map(|(_, m)| m).collect();
    }
    out
}

fn routing_of(preset: &Value) -> String {
    let cfg = preset.get("config").unwrap_or(&Value::Null);
    let mut parts: Vec<String> = Vec::new();
    if models_of(preset).len() > 1 {
        parts.push(
            cfg.get("mode")
                .and_then(Value::as_str)
                .unwrap_or("fallback")
                .into(),
        );
    }
    if cfg.get("paid_fallback").and_then(Value::as_bool) == Some(true) {
        parts.push("paid-fallback".into());
    }
    if let Some(sort) = cfg.pointer("/provider/sort") {
        let s = sort
            .as_str()
            .map(String::from)
            .or_else(|| sort.get("by").and_then(Value::as_str).map(String::from));
        if let Some(s) = s {
            parts.push(format!("sort={s}"));
        }
    }
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join(", ")
    }
}

fn rows_of(body: &Value) -> Vec<Value> {
    body.get("data")
        .and_then(Value::as_array)
        .or_else(|| body.as_array())
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn list_rows(body: &Value) -> Vec<Vec<String>> {
    rows_of(body)
        .iter()
        .map(|p| {
            let models = models_of(p);
            vec![
                text(p.get("slug")),
                text(p.get("name")),
                if models.is_empty() {
                    "-".into()
                } else {
                    models.join(", ")
                },
                routing_of(p),
            ]
        })
        .collect()
}

pub(crate) fn detail_lines(p: &Value) -> Vec<(&'static str, String)> {
    let models = models_of(p);
    vec![
        ("slug", text(p.get("slug"))),
        ("name", text(p.get("name"))),
        ("description", text(p.get("description"))),
        (
            "models",
            if models.is_empty() {
                "-".into()
            } else {
                models.join(", ")
            },
        ),
        ("routing", routing_of(p)),
        ("source", text(p.get("source"))),
        ("updated", text(p.get("updated_at"))),
    ]
}

pub(crate) fn run_presets(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let words: Vec<&str> = parsed.passthrough.iter().map(String::as_str).collect();
    let (verb, rest) = match words.split_first() {
        None => ("list", &[][..]),
        Some((v, rest)) => (*v, rest),
    };
    if !SUBCOMMANDS.contains(&verb) {
        return Err(hint(&format!(
            "{USAGE_LINE}\nunknown presets subcommand \"{verb}\""
        )));
    }
    let mut ctx = context(parsed, env);
    // `--key ak_…` is a management key too.
    if ctx.management_key.is_none() && ctx.key.as_deref().is_some_and(|k| k.starts_with("ak_")) {
        ctx.management_key = ctx.key.clone();
    }
    let slug = || -> Result<&str, String> {
        match rest {
            [slug] => {
                check_slug(slug)?;
                Ok(slug)
            }
            _ => Err(hint(&format!("Usage: {{bin}} presets {verb} <slug>"))),
        }
    };
    // Flags are validated before any network call or key check.
    let changes = match verb {
        "create" | "edit" => Changes::from_flags(parsed)?,
        _ => Changes::default(),
    };
    match verb {
        "list" | "ls" if !rest.is_empty() => {
            return Err(hint("Usage: {bin} presets [list] [--json]"))
        }
        "create" => {
            slug()?;
            if changes.models.is_none() {
                return Err(hint(
                    "presets create needs --model <a,b>: {bin} presets create coding --model a,b --fallback --sort price",
                ));
            }
        }
        "edit" => {
            slug()?;
            if changes.is_empty()
                && get_string_flag(&parsed.flags, "name").is_none()
                && get_string_flag(&parsed.flags, "description").is_none()
            {
                return Err(hint(
                    "nothing to change: pass --model, --fallback, --sort, --name or --description",
                ));
            }
        }
        "show" | "rm" => {
            slug()?;
        }
        _ => {}
    }
    let mctx = management_ctx(&ctx, "presets", SCOPES)?;
    match verb {
        "list" | "ls" => {
            let body = get(&mctx, "/v1/presets", true)?;
            if mctx.json {
                print_json(&body);
            } else if rows_of(&body).is_empty() {
                eprintln!("{}", term::dim("(none)"));
            } else {
                print_table(&["slug", "name", "models", "routing"], &list_rows(&body));
            }
        }
        "show" => {
            let body = get(&mctx, &path_for(slug()?), true)?;
            if mctx.json {
                print_json(&body);
            } else {
                for (label, value) in detail_lines(&body) {
                    println!("{label:<13}{value}");
                }
            }
        }
        "create" => {
            let slug = slug()?;
            let mut body = json!({
                "slug": slug,
                "name": get_string_flag(&parsed.flags, "name").unwrap_or_else(|| slug.into()),
                "config": build_config(&changes),
            });
            if let Some(d) = get_string_flag(&parsed.flags, "description") {
                body["description"] = json!(d);
            }
            let out = send(&mctx, "POST", "/v1/presets", Some(&body.to_string()), true)?;
            finish(&mctx, &out, &format!("created preset {slug}"));
        }
        "edit" => {
            let slug = slug()?;
            let current = get(&mctx, &path_for(slug), true)?;
            let mut config = current
                .get("config")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            changes.apply(&mut config);
            let mut body = json!({"config": config});
            if let Some(n) = get_string_flag(&parsed.flags, "name") {
                body["name"] = json!(n);
            }
            if let Some(d) = get_string_flag(&parsed.flags, "description") {
                body["description"] = json!(d);
            }
            let out = send(
                &mctx,
                "PATCH",
                &path_for(slug),
                Some(&body.to_string()),
                true,
            )?;
            finish(&mctx, &out, &format!("updated preset {slug}"));
        }
        _ => {
            let slug = slug()?;
            confirm(parsed, &format!("Delete preset {slug}?"))?;
            let out = match send(&mctx, "DELETE", &path_for(slug), None, true) {
                Err(e) if crate::api::absent_ok(parsed, &e, "preset", slug) => return Ok(0),
                other => other?,
            };
            if mctx.json {
                print_json(&out);
            } else {
                eprintln!("{} deleted preset {slug}", term::ok("✓"));
            }
        }
    }
    Ok(0)
}

fn finish(ctx: &Ctx, body: &Value, done: &str) {
    if ctx.json {
        print_json(body);
    } else {
        eprintln!("{} {done}", term::ok("✓"));
        for (label, value) in detail_lines(body) {
            println!("{label:<13}{value}");
        }
    }
}

/// `--preset <slug>` on a launch command: rejects `--model`, and returns args that
/// carry the model id `@preset/<slug>` (and no `--preset`, which login also reads).
pub(crate) fn launch_args(parsed: &ParsedArgs) -> Result<Option<(ParsedArgs, String)>, String> {
    let Some(slug) = get_string_flag(&parsed.flags, "preset") else {
        return Ok(None);
    };
    if get_string_flag(&parsed.flags, "model").is_some() {
        return Err(hint(
            "--preset and --model cannot be combined: a preset already chooses the models. Use one of them.",
        ));
    }
    check_slug(&slug)?;
    let mut next = parsed.clone();
    next.flags.remove("preset");
    next.flags
        .insert("model".into(), FlagValue::Value(format!("@preset/{slug}")));
    Ok(Some((next, slug)))
}

/// Fail before the agent starts when the preset does not exist. Prefers the
/// management key, falls back to the launch API key; when that key may not
/// read presets, launch without the check.
pub(crate) fn verify_for_launch(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    base: &str,
    api_key: &str,
    slug: &str,
) -> Result<(), String> {
    let ctx = context(parsed, env);
    let has_management = ctx.management_key.is_some();
    let key = ctx.management_key.clone().unwrap_or_else(|| api_key.into());
    let ctx = Ctx {
        base: base.to_string(),
        key: Some(key.clone()),
        management_key: None,
        json: false,
    };
    let (status, _) = request(&ctx, "GET", &path_for(slug), None, Some(&key))?;
    let bin = crate::help::invoked_bin();
    match status {
        200..=299 => Ok(()),
        404 => Err(crate::exit::tag(
            crate::exit::NOT_FOUND,
            format!(
                "{} preset \"{slug}\" not found. Nothing was launched.\n{} list yours with `{bin} presets`, or create it: `{bin} presets create {slug} --model <a,b>`",
                term::err("error:"),
                term::dim("hint:")
            ),
        )),
        // An inference key cannot read /presets, but the gateway still resolves
        // `@preset/<slug>` for it, so launch and let a missing preset fail there.
        401 | 403 if !has_management => {
            eprintln!(
                "{} could not check preset \"{slug}\" without a management key (ak_…); launching anyway. A missing preset fails on the first request.",
                term::dim("note:")
            );
            Ok(())
        }
        s => Err(format!(
            "{} could not verify preset \"{slug}\" (HTTP {s}). Nothing was launched.",
            term::err("error:")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_cli_args;

    fn changes(args: &[&str]) -> Changes {
        Changes::from_flags(&parse_cli_args(args).unwrap()).unwrap()
    }

    #[test]
    fn flags_map_to_the_preset_config_json() {
        let c = changes(&[
            "presets",
            "create",
            "x",
            "--model",
            "a, b,a",
            "--fallback",
            "--sort",
            "price",
        ]);
        assert_eq!(
            build_config(&c),
            json!({"model": "a", "models": ["a", "b"], "paid_fallback": true, "provider": {"sort": "price"}})
        );
    }

    #[test]
    fn single_model_has_no_fallback_list() {
        let c = changes(&["presets", "create", "x", "--model", "only/one"]);
        assert_eq!(build_config(&c), json!({"model": "only/one"}));
    }

    #[test]
    fn bad_sort_and_empty_model_are_usage_errors() {
        let p = parse_cli_args(["presets", "create", "x", "--sort", "cheap"]).unwrap();
        assert!(Changes::from_flags(&p)
            .err()
            .unwrap()
            .contains("--sort must be one of"));
        let p = parse_cli_args(["presets", "create", "x", "--model", " , "]).unwrap();
        assert!(Changes::from_flags(&p).is_err());
    }

    #[test]
    fn edit_keeps_untouched_fields_and_drops_stale_branches() {
        let mut cfg = json!({
            "version": 2, "mode": "fallback", "paid_fallback": true,
            "branches": [{"priority": 1, "route": {"model_id": "old"}}],
            "provider": {"sort": "latency", "zdr": true}, "system": "be brief"
        })
        .as_object()
        .unwrap()
        .clone();
        changes(&["presets", "edit", "x", "--model", "n1,n2"]).apply(&mut cfg);
        let v = Value::Object(cfg);
        assert!(v.get("branches").is_none() && v.get("version").is_none());
        assert_eq!(v["models"], json!(["n1", "n2"]));
        assert_eq!(v["paid_fallback"], true);
        assert_eq!(v["provider"], json!({"sort": "latency", "zdr": true}));
        assert_eq!(v["system"], "be brief");
    }

    #[test]
    fn slug_rule_matches_the_api() {
        for ok in ["coding", "a1", "9-lives", "a-b-c"] {
            assert!(valid_slug(ok), "{ok}");
        }
        for bad in ["", "-x", "Coding", "a_b", "a/b", "a b"] {
            assert!(!valid_slug(bad), "{bad}");
        }
    }

    #[test]
    fn list_rows_show_models_and_routing_for_v1_and_v2() {
        let body = json!({"data": [
            {"slug": "coding", "name": "Coding", "config": {"model": "a", "models": ["a", "b"], "paid_fallback": true, "provider": {"sort": "price"}}},
            {"slug": "graph", "name": "Graph", "config": {"version": 2, "mode": "weighted", "branches": [
                {"priority": 2, "route": {"model_id": "m2"}}, {"priority": 1, "route": {"model_id": "m1"}}]}},
            {"slug": "bare", "name": "Bare", "config": {}}]});
        let rows = list_rows(&body);
        assert_eq!(
            rows[0],
            [
                "coding",
                "Coding",
                "a, b",
                "fallback, paid-fallback, sort=price"
            ]
        );
        assert_eq!(rows[1], ["graph", "Graph", "m1, m2", "weighted"]);
        assert_eq!(rows[2], ["bare", "Bare", "-", "-"]);
    }

    #[test]
    fn launch_args_rewrite_preset_into_the_model_id() {
        let p = parse_cli_args(["claude", "--preset", "coding"]).unwrap();
        let (next, slug) = launch_args(&p).unwrap().unwrap();
        assert_eq!(slug, "coding");
        assert_eq!(
            get_string_flag(&next.flags, "model").as_deref(),
            Some("@preset/coding")
        );
        assert!(!next.flags.contains_key("preset"));
        let p = parse_cli_args(["claude"]).unwrap();
        assert!(launch_args(&p).unwrap().is_none());
    }

    #[test]
    fn launch_args_reject_model_combo_and_bad_slug() {
        let p = parse_cli_args(["claude", "--preset", "coding", "--model", "a/b"]).unwrap();
        assert!(launch_args(&p)
            .err()
            .unwrap()
            .contains("cannot be combined"));
        let p = parse_cli_args(["claude", "--preset", "Bad Slug"]).unwrap();
        assert!(launch_args(&p)
            .err()
            .unwrap()
            .contains("Invalid preset slug"));
    }
}
