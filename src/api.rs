//! `anyr api` — the dashboard from the terminal.
//!
//!   anyr api <resource> <verb> [args] [--json]   curated, table output on a TTY
//!   anyr api [METHOD] /path [field=value …]      raw passthrough (like `gh api`)
//!
//! Data goes to stdout, everything else to stderr. `--json` (or a non-TTY
//! stdout on raw calls) prints the server JSON unchanged.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Read};

use serde_json::{Map, Value};

use crate::config::Profile;
use crate::http::{http_delete, http_get, http_patch, http_post, http_send, join_api};
use crate::key::{load_config_if_present, no_key_error, resolve_api_key, resolve_base_url};
use crate::parse::{get_string_flag, ParsedArgs};
use crate::term;

pub const FLAGS: &[&str] = &[
    "profile",
    "base-url",
    "config",
    "key",
    "json",
    "yes",
    "limit",
    "model",
    "status",
    "type",
    "name",
    "data",
    "method",
    "idempotency-key",
];

/// (resource, description). Order is help order.
pub const RESOURCES: &[(&str, &str)] = &[
    ("me", "Profile: id, email, balance"),
    ("credits", "Balance and transactions"),
    ("keys", "API keys: list, get, create, update, revoke"),
    ("logs", "Request logs and sessions"),
    ("presets", "Routing presets (needs a management key)"),
    ("aliases", "Claude Code model aliases (local)"),
    ("models", "Model catalog"),
    ("providers", "Upstream providers"),
    ("dashboard", "Dashboard overview"),
    ("hubs", "Skill and prompt hubs"),
    ("connections", "MCP gateway connections"),
];

pub fn verbs(resource: &str) -> &'static [(&'static str, &'static str)] {
    match resource {
        "me" | "profile" => &[("get", "Show profile")],
        "credits" => &[
            ("get", "Show balance"),
            ("transactions", "List transactions"),
        ],
        "keys" => &[
            ("list", "List keys"),
            ("get", "Show one key"),
            ("create", "Create a key (secret shown once)"),
            ("update", "Update fields: name=… disabled=true limit=…"),
            ("revoke", "Delete a key"),
        ],
        "logs" => &[
            ("list", "Recent requests"),
            ("get", "One request"),
            ("sessions", "Grouped by session"),
        ],
        "presets" => &[
            ("list", "List presets"),
            ("get", "Show one preset"),
            ("create", "Create: <slug> --name … --data '<config json>'"),
            ("update", "Update: <slug> name=… or --data '<config json>'"),
            ("delete", "Delete a preset"),
        ],
        "aliases" => &[
            ("list", "Show alias slots"),
            ("set", "set <slot> <model-id>"),
        ],
        "models" | "providers" | "hubs" | "connections" => &[("list", "List"), ("get", "Show one")],
        "dashboard" => &[("get", "Overview")],
        _ => &[],
    }
}

/// Context resolved once per invocation.
pub(crate) struct Ctx {
    pub(crate) base: String,
    pub(crate) key: Option<String>,
    pub(crate) management_key: Option<String>,
    pub(crate) json: bool,
}

impl Ctx {
    pub(crate) fn key(&self) -> Result<&str, String> {
        self.key.as_deref().ok_or_else(no_key_error)
    }
}

use crate::cmd::dispatch::USAGE;

pub fn run(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    crate::cmd::dispatch::usage_exit(run_inner(parsed, env))
}

/// Base URL, API key and management key for this invocation.
pub(crate) fn context(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Ctx {
    let path = crate::config::resolve_config_path(
        get_string_flag(&parsed.flags, "config").as_deref(),
        env,
    );
    let cfg = load_config_if_present(&path);
    let profile: Option<&Profile> = cfg.as_ref().and_then(|c| {
        let name = get_string_flag(&parsed.flags, "profile")
            .or_else(|| env.get("ANYROUTER_PROFILE").cloned())
            .unwrap_or_else(|| c.active_profile.clone());
        c.profiles.get(&name)
    });
    Ctx {
        base: resolve_base_url(&parsed.flags, profile),
        key: resolve_api_key(&parsed.flags, env, profile),
        management_key: env
            .get("ANYROUTER_MANAGEMENT_KEY")
            .cloned()
            .or_else(|| profile.and_then(|p| p.management_key.clone()))
            .filter(|k| !k.trim().is_empty()),
        json: parsed.flag_true("json"),
    }
}

fn run_inner(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let args: Vec<&str> = parsed.passthrough.iter().map(String::as_str).collect();
    if args.is_empty() {
        print!("{}", help());
        return Ok(0);
    }
    let ctx = context(parsed, env);

    // Raw: `api /path …` or `api METHOD /path …`.
    let method_word = args[0].to_ascii_uppercase();
    let is_method = matches!(
        method_word.as_str(),
        "GET" | "POST" | "PATCH" | "PUT" | "DELETE"
    );
    if args[0].starts_with('/') || (is_method && args.get(1).is_some_and(|a| a.starts_with('/'))) {
        let (method, rest) = if is_method {
            (method_word, &args[1..])
        } else {
            let m = get_string_flag(&parsed.flags, "method")
                .unwrap_or_else(|| "GET".into())
                .to_ascii_uppercase();
            (m, &args[..])
        };
        let (mut headers, words) = split_headers(&rest[1..])?;
        if let Some(k) = get_string_flag(&parsed.flags, "idempotency-key") {
            headers.push(("Idempotency-Key".into(), k));
        }
        let body = body_from(parsed, &words)?;
        let method = if method == "GET" && body.is_some() {
            "POST".into()
        } else {
            method
        };
        return raw(&ctx, &method, rest[0], body.as_deref(), &headers);
    }

    let resource = match args[0] {
        "profile" | "whoami" => "me",
        other => other,
    };
    if verbs(resource).is_empty() {
        return Err(unknown(
            "resource",
            args[0],
            RESOURCES.iter().map(|(n, _)| *n),
        ));
    }
    let verb = args.get(1).copied().unwrap_or(verbs(resource)[0].0);
    let verb = match verb {
        "ls" => "list",
        "show" => "get",
        "rm" | "delete" if resource == "keys" => "revoke",
        "rm" | "revoke" if resource == "presets" => "delete",
        "rm" => "delete",
        "new" | "add" => "create",
        other => other,
    };
    if !verbs(resource).iter().any(|(v, _)| *v == verb) {
        return Err(unknown(
            &format!("verb for `api {resource}`"),
            verb,
            verbs(resource).iter().map(|(v, _)| *v),
        ));
    }
    let rest = if args.len() > 2 { &args[2..] } else { &[][..] };
    let arg = |what: &str| -> Result<&str, String> {
        rest.first().copied().ok_or_else(|| {
            format!(
                "{} missing <{what}>\n{} {} api {resource} {verb} <{what}>",
                term::err("error:"),
                term::dim("usage:"),
                crate::help::invoked_bin()
            )
        })
    };

    match (resource, verb) {
        ("me", _) => show(&ctx, get(&ctx, "/v1/me", true)?, &[]),
        ("credits", "get") => show(&ctx, get(&ctx, "/v1/credits", true)?, &[]),
        ("credits", "transactions") => {
            let q = query(parsed, &["limit", "type"]);
            list(
                &ctx,
                get(&ctx, &format!("/v1/credits/transactions{q}"), true)?,
                &[
                    "created_at",
                    "type",
                    "amount",
                    "balance_after",
                    "description",
                ],
            )
        }
        ("keys", "list") => list(
            &ctx,
            get(&ctx, "/v1/keys", true)?,
            &[
                "hash",
                "name",
                "label",
                "disabled",
                "usage",
                "limit",
                "created_at",
            ],
        ),
        ("keys", "get") => show(
            &ctx,
            get(&ctx, &format!("/v1/keys/{}", arg("hash")?), true)?,
            &[],
        ),
        ("keys", "create") => {
            let name = get_string_flag(&parsed.flags, "name")
                .or_else(|| rest.first().map(|s| s.to_string()))
                .unwrap_or_else(|| "anyr-cli".into());
            let mut body = fields(rest.get(1..).unwrap_or(&[]))?;
            body.insert("name".into(), Value::String(name));
            let out = send(
                &ctx,
                "POST",
                "/v1/keys",
                Some(&Value::Object(body).to_string()),
                true,
            )?;
            if !ctx.json {
                if let Some(secret) = out.get("key").and_then(Value::as_str) {
                    println!("{secret}");
                    eprintln!(
                        "{} key created. The secret above is shown once; store it now.",
                        term::ok("✓")
                    );
                    return Ok(0);
                }
            }
            show(&ctx, out, &[])
        }
        ("keys", "update") => {
            let hash = arg("hash")?;
            let body =
                body_from(parsed, &rest[1..])?.ok_or("nothing to update: pass field=value")?;
            show(
                &ctx,
                send(
                    &ctx,
                    "PATCH",
                    &format!("/v1/keys/{hash}"),
                    Some(&body),
                    true,
                )?,
                &[],
            )
        }
        ("keys", "revoke") => {
            let hash = arg("hash")?;
            confirm(parsed, &format!("Revoke key {hash}?"))?;
            send(&ctx, "DELETE", &format!("/v1/keys/{hash}"), None, true)?;
            eprintln!("{} revoked {hash}", term::ok("✓"));
            Ok(0)
        }
        ("logs", "list") => {
            let q = query(parsed, &["limit", "model", "status"]);
            list(
                &ctx,
                get(&ctx, &format!("/v1/logs{q}"), true)?,
                &[
                    "created_at",
                    "model",
                    "status",
                    "total_tokens",
                    "cost",
                    "id",
                ],
            )
        }
        ("logs", "get") => show(
            &ctx,
            get(&ctx, &format!("/v1/logs/{}", arg("id")?), true)?,
            &[],
        ),
        ("logs", "sessions") => {
            let q = query(parsed, &["limit"]);
            list(
                &ctx,
                get(&ctx, &format!("/v1/logs/sessions{q}"), true)?,
                &["session_id", "requests", "errors", "cost_usd", "last_seen"],
            )
        }
        ("presets", _) => presets(&ctx, parsed, verb, rest),
        ("aliases", _) => aliases(parsed, env, verb, rest),
        ("models", "list") => list(
            &ctx,
            get(&ctx, "/v1/models", false)?,
            &["id", "name", "context_length"],
        ),
        ("models", "get") => show(
            &ctx,
            get(&ctx, &format!("/v1/models/{}", arg("model-id")?), false)?,
            &[],
        ),
        ("providers", "list") => list(
            &ctx,
            get(&ctx, "/v1/providers", false)?,
            &["id", "name", "status", "model_count"],
        ),
        ("providers", "get") => {
            let id = arg("provider-id")?;
            let all = get(&ctx, "/v1/providers", false)?;
            let hit = rows(&all)
                .into_iter()
                .find(|p| p.get("id").and_then(Value::as_str) == Some(id));
            show(
                &ctx,
                hit.ok_or(format!("provider \"{id}\" not found"))?,
                &[],
            )
        }
        ("dashboard", _) => show(&ctx, get(&ctx, "/v1/dashboard/overview", true)?, &[]),
        ("hubs", "list") => list(
            &ctx,
            get(&ctx, "/v1/hubs", true)?,
            &["slug", "name", "visibility", "updated_at"],
        ),
        ("hubs", "get") => show(
            &ctx,
            get(&ctx, &format!("/v1/hubs/{}", arg("slug")?), true)?,
            &[],
        ),
        ("connections", "list") => list(
            &ctx,
            get(&ctx, "/v1/connections", true)?,
            &["id", "name", "server_url", "status", "calls"],
        ),
        ("connections", "get") => show(
            &ctx,
            get(&ctx, &format!("/v1/connections/{}", arg("id")?), true)?,
            &[],
        ),
        _ => unreachable!("verb table and dispatch disagree"),
    }
}

/// A context that authenticates with the management key (`ak_…`), or the error
/// that says how to get one. `what` names the resource, `scopes` the key scopes.
pub(crate) fn management_ctx(ctx: &Ctx, what: &str, scopes: &str) -> Result<Ctx, String> {
    let Some(mk) = ctx.management_key.as_deref() else {
        return Err(crate::exit::tag(
            crate::exit::AUTH,
            format!(
            "{} {what} need a management key (ak_…); your API key cannot manage {what}.\n{} create one at https://anyrouter.dev/settings/management-keys with {scopes}, then:\n      export ANYROUTER_MANAGEMENT_KEY=ak_…",
            term::err("error:"),
            term::dim("hint:")
        ),
        ));
    };
    Ok(Ctx {
        key: Some(mk.to_string()),
        management_key: None,
        base: ctx.base.clone(),
        json: ctx.json,
    })
}

fn presets(ctx: &Ctx, parsed: &ParsedArgs, verb: &str, rest: &[&str]) -> Result<i32, String> {
    let mctx = management_ctx(ctx, "presets", "read:presets and write:presets")?;
    let slug = || {
        rest.first().copied().ok_or_else(|| {
            format!(
                "missing <slug>: {} api presets {verb} <slug>",
                crate::help::invoked_bin()
            )
        })
    };
    match verb {
        "list" => list(
            &mctx,
            get(&mctx, "/v1/presets", true)?,
            &["slug", "name", "description", "updated_at"],
        ),
        "get" => show(
            &mctx,
            get(&mctx, &format!("/v1/presets/{}", slug()?), true)?,
            &[],
        ),
        "create" => {
            let slug = slug()?;
            let mut body = fields(&rest[1..])?;
            body.insert("slug".into(), Value::String(slug.into()));
            let name = get_string_flag(&parsed.flags, "name").unwrap_or_else(|| slug.into());
            body.entry("name").or_insert(Value::String(name));
            if let Some(data) = data_flag(parsed)? {
                body.insert("config".into(), data);
            }
            if !body.contains_key("config") {
                return Err("presets create needs --data '<config json>', e.g. --data '{\"model\":\"anyrouter/auto\"}'".into());
            }
            show(
                &mctx,
                send(
                    &mctx,
                    "POST",
                    "/v1/presets",
                    Some(&Value::Object(body).to_string()),
                    true,
                )?,
                &[],
            )
        }
        "update" => {
            let slug = slug()?;
            let mut body = fields(&rest[1..])?;
            if let Some(data) = data_flag(parsed)? {
                body.insert("config".into(), data);
            }
            if let Some(name) = get_string_flag(&parsed.flags, "name") {
                body.insert("name".into(), Value::String(name));
            }
            if body.is_empty() {
                return Err("nothing to update: pass name=… or --data '<config json>'".into());
            }
            show(
                &mctx,
                send(
                    &mctx,
                    "PATCH",
                    &format!("/v1/presets/{slug}"),
                    Some(&Value::Object(body).to_string()),
                    true,
                )?,
                &[],
            )
        }
        "delete" => {
            let slug = slug()?;
            confirm(parsed, &format!("Delete preset {slug}?"))?;
            send(&mctx, "DELETE", &format!("/v1/presets/{slug}"), None, true)?;
            eprintln!("{} deleted preset {slug}", term::ok("✓"));
            Ok(0)
        }
        _ => unreachable!(),
    }
}

/// Claude Code alias slots live in the local profile; the server has no alias API.
fn aliases(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    verb: &str,
    rest: &[&str],
) -> Result<i32, String> {
    const SLOTS: &[&str] = &["haiku", "sonnet", "opus", "fable"];
    if verb == "set" {
        let (Some(slot), Some(model)) = (rest.first(), rest.get(1)) else {
            return Err(format!(
                "usage: {} api aliases set <haiku|sonnet|opus|fable> <model-id>",
                crate::help::invoked_bin()
            ));
        };
        if !SLOTS.contains(slot) {
            return Err(unknown("alias slot", slot, SLOTS.iter().copied()));
        }
        // Reuse `models --<slot>` so persistence has one code path.
        let mut next = parsed.clone();
        next.command = "models".into();
        next.passthrough.clear();
        next.flags.remove("json");
        next.flags.insert(
            (*slot).into(),
            crate::parse::FlagValue::Value((*model).into()),
        );
        return crate::cmd::models::run_models(&next, env);
    }
    let path = crate::config::resolve_config_path(
        get_string_flag(&parsed.flags, "config").as_deref(),
        env,
    );
    let cfg = load_config_if_present(&path).unwrap_or_default();
    let name = get_string_flag(&parsed.flags, "profile")
        .or_else(|| env.get("ANYROUTER_PROFILE").cloned())
        .unwrap_or_else(|| cfg.active_profile.clone());
    let p = cfg.profiles.get(&name).cloned().unwrap_or_default();
    let values = [
        p.claude_haiku(),
        p.claude_sonnet(),
        p.claude_opus(),
        p.claude_fable(),
    ];
    if parsed.flag_true("json") {
        let mut obj = Map::new();
        obj.extend(
            SLOTS
                .iter()
                .zip(values)
                .map(|(s, v)| (s.to_string(), Value::String(v.to_string()))),
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Object(obj)).unwrap_or_default()
        );
    } else {
        for (slot, v) in SLOTS.iter().zip(values) {
            println!("{slot:<8}{v}");
        }
    }
    Ok(0)
}

fn raw(
    ctx: &Ctx,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(String, String)],
) -> Result<i32, String> {
    let path = if path.starts_with("/v1/") || path == "/v1" {
        path.to_string()
    } else {
        format!("/v1{path}")
    };
    let (status, text) = request_with(ctx, method, &path, body, ctx.key.as_deref(), headers)?;
    let pretty = serde_json::from_str::<Value>(&text)
        .ok()
        .filter(|_| std::io::stdout().is_terminal())
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or(text);
    if !pretty.is_empty() {
        println!("{pretty}");
    }
    if (200..300).contains(&status) {
        Ok(0)
    } else {
        let code = exit_for(status);
        crate::exit::print(code, &format!("HTTP {status} {method} {path}"));
        Ok(code)
    }
}

pub(crate) fn request(
    ctx: &Ctx,
    method: &str,
    path: &str,
    body: Option<&str>,
    key: Option<&str>,
) -> Result<(u16, String), String> {
    request_with(ctx, method, path, body, key, &[])
}

pub(crate) fn request_with(
    ctx: &Ctx,
    method: &str,
    path: &str,
    body: Option<&str>,
    key: Option<&str>,
    headers: &[(String, String)],
) -> Result<(u16, String), String> {
    let url = join_api(&ctx.base, path);
    if !headers.is_empty() {
        return match method {
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" => {
                http_send(method, &url, key, headers, body)
            }
            other => Err(format!("unsupported method {other}")),
        };
    }
    match method {
        "GET" => http_get(&url, key),
        "POST" | "PUT" => http_post(&url, key, body),
        "PATCH" => http_patch(&url, key, body),
        "DELETE" => http_delete(&url, key),
        other => Err(format!("unsupported method {other}")),
    }
}

pub(crate) fn get(ctx: &Ctx, path: &str, auth: bool) -> Result<Value, String> {
    send(ctx, "GET", path, None, auth)
}

pub(crate) fn send(
    ctx: &Ctx,
    method: &str,
    path: &str,
    body: Option<&str>,
    auth: bool,
) -> Result<Value, String> {
    let key = if auth {
        Some(ctx.key()?)
    } else {
        ctx.key.as_deref()
    };
    let (status, text) = request(ctx, method, path, body, key)?;
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));
    if (200..300).contains(&status) {
        return Ok(value);
    }
    let msg = value
        .pointer("/error/message")
        .or_else(|| value.get("error"))
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| error_summary(status, &text));
    let hint = match status {
        401 => format!(
            "\n{} sign in again: {} login",
            term::dim("hint:"),
            crate::help::invoked_bin()
        ),
        403 => format!(
            "\n{} {}",
            term::dim("hint:"),
            crate::http::scope_error(path.trim_start_matches("/v1/"))
        ),
        _ => String::new(),
    };
    Err(crate::exit::tag(
        exit_for(status),
        format!("{} HTTP {status}: {msg}{hint}", term::err("error:")),
    ))
}

/// Error text for a body with no JSON `message`. An HTML 404 page must not
/// flood the terminal, so show the status reason plus a short single-line peek.
fn error_summary(status: u16, body: &str) -> String {
    if serde_json::from_str::<Value>(body).is_ok() {
        return body.trim().to_string();
    }
    let reason = match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    };
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut peek: String = flat.chars().take(200).collect();
    if flat.chars().count() > 200 {
        peek.push('…');
    }
    if peek.is_empty() {
        reason.to_string()
    } else {
        format!("{reason}: {peek}")
    }
}

pub(crate) fn exit_for(status: u16) -> i32 {
    crate::exit::for_status(status)
}

/// `--data '<json>'`, `--data @file` or `--data -` (stdin) wins; otherwise
/// `k=v` words become a JSON object.
fn body_from(parsed: &ParsedArgs, words: &[&str]) -> Result<Option<String>, String> {
    if let Some(data) = data_flag(parsed)? {
        return Ok(Some(data.to_string()));
    }
    let map = fields(words)?;
    Ok((!map.is_empty()).then(|| Value::Object(map).to_string()))
}

/// `k=v` → string, `k:=json` → raw JSON (httpie convention).
fn fields(words: &[&str]) -> Result<Map<String, Value>, String> {
    let mut map = Map::new();
    for w in words {
        if let Some((k, v)) = w.split_once(":=") {
            map.insert(k.into(), parse_json(v)?);
        } else if let Some((k, v)) = w.split_once('=') {
            let v = match v {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => v
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .filter(|_| !v.starts_with('0') || v == "0" || v.starts_with("0."))
                    .map(Value::Number)
                    .unwrap_or(Value::String(v.into())),
            };
            map.insert(k.into(), v);
        } else {
            return Err(format!("expected field=value, got \"{w}\""));
        }
    }
    Ok(map)
}

/// The `--data` value as JSON, reading `@file` or `-` (stdin) when asked.
fn data_flag(parsed: &ParsedArgs) -> Result<Option<Value>, String> {
    let Some(spec) = get_string_flag(&parsed.flags, "data") else {
        return Ok(None);
    };
    let text = if spec == "-" {
        read_stdin()?
    } else if let Some(path) = spec.strip_prefix('@') {
        read_file_capped(path)?
    } else {
        spec
    };
    parse_json(&text).map(Some)
}

/// Largest document read from a file or stdin (the server caps bodies at 256 KiB).
pub(crate) const MAX_INPUT_BYTES: u64 = 256 * 1024;

fn read_capped(r: impl std::io::Read, what: &str) -> Result<String, String> {
    let mut buf = String::new();
    r.take(MAX_INPUT_BYTES + 1)
        .read_to_string(&mut buf)
        .map_err(|e| format!("cannot read {what}: {e}"))?;
    if buf.len() as u64 > MAX_INPUT_BYTES {
        return Err(format!(
            "{what} is larger than {} KiB, the server limit",
            MAX_INPUT_BYTES / 1024
        ));
    }
    Ok(buf)
}

pub(crate) fn read_stdin() -> Result<String, String> {
    read_capped(std::io::stdin(), "stdin")
}

pub(crate) fn read_file_capped(path: &str) -> Result<String, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    read_capped(f, path)
}

/// Pull `-H key:value` pairs (repeatable) out of the positional words.
type Headers = Vec<(String, String)>;

fn split_headers<'a>(words: &[&'a str]) -> Result<(Headers, Vec<&'a str>), String> {
    let mut headers = Vec::new();
    let mut rest = Vec::new();
    let mut it = words.iter();
    while let Some(&w) = it.next() {
        if w == "-H" {
            let raw = it
                .next()
                .ok_or_else(|| format!("{USAGE}-H needs a header, e.g. -H 'X-Trace: 1'"))?;
            let (k, v) = raw
                .split_once(':')
                .filter(|(k, _)| !k.trim().is_empty())
                .ok_or_else(|| format!("{USAGE}header must look like key:value, got \"{raw}\""))?;
            headers.push((k.trim().to_string(), v.trim().to_string()));
        } else {
            rest.push(w);
        }
    }
    Ok((headers, rest))
}

fn parse_json(s: &str) -> Result<Value, String> {
    serde_json::from_str(s).map_err(|e| format!("invalid JSON ({e}): {s}"))
}

pub(crate) fn query(parsed: &ParsedArgs, names: &[&str]) -> String {
    let parts: Vec<String> = names
        .iter()
        .filter_map(|n| get_string_flag(&parsed.flags, n).map(|v| format!("{n}={}", encode(&v))))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

pub(crate) fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `--if-exists` on a delete verb: when `err` says the target is missing,
/// print `{"deleted": false, <what>: <id>}` (or a note) and report success.
pub(crate) fn absent_ok(parsed: &ParsedArgs, err: &str, what: &str, id: &str) -> bool {
    if !parsed.flag_true("if-exists") || crate::exit::classify(err).0 != crate::exit::NOT_FOUND {
        return false;
    }
    if parsed.flag_true("json") {
        println!("{}", serde_json::json!({ "deleted": false, what: id }));
    } else {
        eprintln!("No {what} {id}; nothing to delete.");
    }
    true
}

/// Off a terminal without `--yes`/`--ok`: the exit-6 error whose hint is the
/// rerun. Call it before any request, so a refused run sends nothing.
pub(crate) fn require_yes_off_tty(parsed: &ParsedArgs, question: &str) -> Result<(), String> {
    if parsed.skip_confirm() || term::is_interactive() {
        Ok(())
    } else {
        Err(crate::exit::confirm_required(question))
    }
}

/// Ask `question` unless `--yes`; off a terminal, exit 6.
pub(crate) fn confirm(parsed: &ParsedArgs, question: &str) -> Result<(), String> {
    require_yes_off_tty(parsed, question)?;
    if parsed.skip_confirm() || term::confirm(question) {
        Ok(())
    } else {
        Err("cancelled".into())
    }
}

fn unknown<'a>(what: &str, got: &str, options: impl Iterator<Item = &'a str>) -> String {
    let opts: Vec<&str> = options.collect();
    format!(
        "{USAGE}{} unknown {what} \"{got}\"\n{} one of: {}",
        term::err("error:"),
        term::dim("hint:"),
        opts.join(", ")
    )
}

pub(crate) fn rows(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn cell(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "-".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| cell(Some(x)))
            .collect::<Vec<_>>()
            .join(","),
        Some(other) => other.to_string(),
    }
}

/// Table on a TTY, TSV when piped, raw JSON with `--json`.
pub(crate) fn list(ctx: &Ctx, value: Value, columns: &[&str]) -> Result<i32, String> {
    if ctx.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
        return Ok(0);
    }
    let items = rows(&value);
    let tty = std::io::stdout().is_terminal();
    if items.is_empty() {
        eprintln!("{}", term::dim("(none)"));
        return Ok(0);
    }
    let table: Vec<Vec<String>> = items
        .iter()
        .map(|r| columns.iter().map(|c| cell(r.get(*c))).collect())
        .collect();
    if !tty {
        for row in table {
            println!("{}", row.join("\t"));
        }
        return Ok(0);
    }
    let max = 48usize;
    let widths: Vec<usize> = (0..columns.len())
        .map(|i| {
            table
                .iter()
                .map(|r| r[i].chars().count())
                .chain([columns[i].len()])
                .max()
                .unwrap_or(0)
                .min(max)
        })
        .collect();
    let fit = |s: &str, w: usize| -> String {
        if s.chars().count() > w {
            format!("{}…", s.chars().take(w - 1).collect::<String>())
        } else {
            format!("{s:<w$}")
        }
    };
    let header: Vec<String> = columns
        .iter()
        .zip(&widths)
        .map(|(c, w)| fit(&c.to_uppercase(), *w))
        .collect();
    println!("{}", term::dim(header.join("  ").trim_end()));
    for row in &table {
        let line: Vec<String> = row.iter().zip(&widths).map(|(c, w)| fit(c, *w)).collect();
        println!("{}", line.join("  ").trim_end());
    }
    Ok(0)
}

/// Key/value view on a TTY, JSON otherwise.
pub(crate) fn show(ctx: &Ctx, value: Value, _hide: &[&str]) -> Result<i32, String> {
    let obj = value
        .get("data")
        .filter(|d| d.is_object())
        .cloned()
        .unwrap_or(value);
    if ctx.json || !std::io::stdout().is_terminal() {
        println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
        return Ok(0);
    }
    match &obj {
        Value::Object(map) => {
            let w = map.keys().map(|k| k.len()).max().unwrap_or(0);
            for (k, v) in map {
                let text = match v {
                    Value::Object(_) | Value::Array(_) if v.to_string().len() > 80 => {
                        serde_json::to_string_pretty(v)
                            .unwrap_or_default()
                            .replace('\n', &format!("\n{:w$}  ", ""))
                    }
                    _ => cell(Some(v)),
                };
                println!("{}  {text}", term::dim(&format!("{k:<w$}")));
            }
        }
        other => println!(
            "{}",
            serde_json::to_string_pretty(other).unwrap_or_default()
        ),
    }
    Ok(0)
}

pub fn help() -> String {
    let bin = crate::help::invoked_bin();
    let mut out = crate::help::doc("api").replace("{bin}", &bin);
    for (name, desc) in RESOURCES {
        let verbs: Vec<&str> = verbs(name).iter().map(|(v, _)| *v).collect();
        out.push_str(&format!("  {name:<12}{desc}  [{}]\n", verbs.join(" ")));
    }
    out.push_str(&crate::help::doc("api_flags"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_types_values() {
        let m = fields(&[
            "name=ci",
            "disabled=true",
            "limit=25",
            "tags:=[\"a\"]",
            "code=007",
        ])
        .unwrap();
        assert_eq!(m["name"], Value::String("ci".into()));
        assert_eq!(m["disabled"], Value::Bool(true));
        assert_eq!(m["limit"].as_f64(), Some(25.0));
        assert_eq!(m["tags"], serde_json::json!(["a"]));
        // Leading zeros stay strings so ids are not mangled.
        assert_eq!(m["code"], Value::String("007".into()));
        assert!(fields(&["oops"]).is_err());
    }

    #[test]
    fn every_resource_has_verbs() {
        for (r, _) in RESOURCES {
            assert!(!verbs(r).is_empty(), "{r} has no verbs");
        }
    }

    #[test]
    fn rows_reads_data_envelope_and_bare_arrays() {
        assert_eq!(rows(&serde_json::json!({"data":[1,2]})).len(), 2);
        assert_eq!(rows(&serde_json::json!([1])).len(), 1);
    }

    #[test]
    fn auth_failures_get_distinct_exit_code() {
        assert_eq!(exit_for(401), 4);
        assert_eq!(exit_for(404), 3);
        assert_eq!(exit_for(500), 1);
    }
}
