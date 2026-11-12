//! `anyr publish` and `anyr apps` — send structured documents to any
//! AnyRouter app that accepts submissions.
//!
//! The CLI knows no app by name. Each app serves its own JSON Schema at
//! `GET /v1/apps/<app>/schema`; `publish` validates against it locally, then
//! posts to `/v1/apps/<app>/submissions`. A new app needs no change here.

use std::collections::BTreeMap;
use std::io::IsTerminal;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::api::{
    confirm, context, exit_for, read_file_capped, read_stdin, request, request_with, Ctx,
};
use crate::cmd::dispatch::USAGE;
use crate::key::no_key_error;
use crate::parse::{get_string_flag, ParsedArgs};
use crate::{schema, term};

pub const PUBLISH_FLAGS: &[&str] = &[
    "profile",
    "base-url",
    "config",
    "key",
    "json",
    "yes",
    "visibility",
    "dry-run",
    "light",
];
pub const APPS_FLAGS: &[&str] = &[
    "profile",
    "base-url",
    "config",
    "key",
    "json",
    "yes",
    "if-exists",
];

pub const APPS_VERBS: &[(&str, &str)] = &[
    (
        "schema",
        "Print an app's JSON Schema, instructions and example",
    ),
    ("ls", "List your submissions: <app>"),
    ("rm", "Delete a submission: <app> <id>"),
    (
        "visibility",
        "Change who can see it: <app> <id> private|unlisted|public",
    ),
];

pub(crate) const VISIBILITIES: &[&str] = &["unlisted", "private", "public"];

pub fn publish_help() -> String {
    let bin = crate::help::invoked_bin();
    format!(
        "{bin} publish — send a JSON document to an AnyRouter app\n\n\
Usage:\n  {bin} publish <app> [file|-] [--visibility unlisted|private|public] [--yes] [--dry-run] [--json]\n  \
{bin} publish <app> <page.html|figure.svg> [--light <page-light.html>] [options]\n\n\
Reads the document from <file>, or from stdin with `-` (or when piped with no file).\n\
It is checked locally against the app's published JSON Schema first; problems are\n\
listed by field and nothing is uploaded (exit 2). On success the URL is printed.\n\n\
An .html or .svg file is uploaded as a page when the app takes uploads (see\n\
`upload` in `{bin} apps schema <app>`). A sibling <name>-light.<ext> is sent as\n\
the light build; pass --light <file> to name it yourself.\n\n\
Examples\n  $ {bin} apps schema explain > schema.json     # what an agent must produce\n  \
$ <agent produces explain json> | {bin} publish explain -\n  \
$ {bin} publish explain fig.json --visibility public --yes\n  \
$ {bin} publish explain fig.json --dry-run\n  \
$ {bin} publish explain page.html --dry-run    # page.html + page-light.html\n\n\
Visibility: unlisted (default; link works, not listed), private (only you),\n\
public (listed after review). Re-running the same document is safe: it carries a\n\
stable Idempotency-Key, so the server returns the original instead of a copy.\n\n\
Exit codes: 0 ok, 1 error, 2 usage or invalid document, 3 not found, 4 auth, 5 rate limited,\n\
6 needs --yes, 7 network, 8 payment required.\n"
    )
}

pub fn apps_help() -> String {
    let bin = crate::help::invoked_bin();
    let mut out = format!(
        "{bin} apps — inspect and manage what you published\n\nUsage:\n  {bin} apps <verb> <app> [id] [--json]\n\nVerbs\n"
    );
    for (v, d) in APPS_VERBS {
        out.push_str(&format!("  {v:<11} {d}\n"));
    }
    out.push_str(&format!(
        "\nExamples\n  $ {bin} apps schema explain\n  $ {bin} apps ls explain\n  $ {bin} apps visibility explain k3f9a2 private\n  $ {bin} apps rm explain k3f9a2 --yes\n"
    ));
    out
}

/// Stable per-document key: key-order of the input does not matter, a change
/// of app or content does.
pub fn idempotency_key(app: &str, payload: &Value) -> String {
    let mut h = Sha256::new();
    h.update(app.as_bytes());
    h.update([0]);
    h.update(payload.to_string().as_bytes());
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("anyr-{}", &hex[..32])
}

fn usage(msg: impl std::fmt::Display) -> String {
    format!("{USAGE}{} {msg}", term::err("error:"))
}

fn app_arg(raw: Option<&&str>, bin_usage: &str) -> Result<String, String> {
    let app = raw.ok_or_else(|| {
        usage(format!(
            "missing <app>\n{} {bin_usage}",
            term::dim("usage:")
        ))
    })?;
    if app.is_empty()
        || !app
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(usage(format!(
            "app names are lowercase letters, digits and dashes, got \"{app}\""
        )));
    }
    Ok(app.to_string())
}

fn visibility_arg(v: &str) -> Result<&str, String> {
    VISIBILITIES
        .iter()
        .copied()
        .find(|x| *x == v)
        .ok_or_else(|| {
            usage(format!(
                "visibility must be one of {}, got \"{v}\"",
                VISIBILITIES.join(", ")
            ))
        })
}

/// What the new visibility means, from the server's `status`.
fn visibility_note(visibility: &str, status: Option<&str>) -> &'static str {
    match (visibility, status) {
        ("public", Some("pending_review")) => ": pending review before it is listed publicly",
        ("public", Some("accepted")) => ": listed publicly",
        ("public", Some("rejected")) => ": rejected in review, so it is not listed",
        ("unlisted", _) => ": anyone with the link can open it; not listed",
        ("private", _) => ": only you can open it",
        _ => "",
    }
}

/// Server message for a failed call, plus the exit code it maps to.
fn failure(status: u16, text: &str, what: &str) -> (String, i32) {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let msg = v
        .pointer("/error/message")
        .or_else(|| v.get("error"))
        .or_else(|| v.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            text.split_whitespace()
                .take(24)
                .collect::<Vec<_>>()
                .join(" ")
        });
    let hint = match status {
        401 => format!(
            "\n{} sign in again: {} login",
            term::dim("hint:"),
            crate::help::invoked_bin()
        ),
        403 => format!("\n{} this key cannot use this endpoint", term::dim("hint:")),
        _ => String::new(),
    };
    let issues = v
        .pointer("/error/metadata/issues")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|i| {
                    let field = |k: &str| i.get(k).and_then(Value::as_str).unwrap_or("");
                    let fix = field("fix");
                    let fix = if fix.is_empty() {
                        String::new()
                    } else {
                        format!(" {} {fix}", term::dim("fix:"))
                    };
                    format!("\n  {}: {}{fix}", field("path"), field("message"))
                })
                .collect::<String>()
        })
        .unwrap_or_default();
    (
        format!(
            "{} HTTP {status} {what}: {msg}{issues}{hint}",
            term::err("error:")
        ),
        exit_for(status),
    )
}

/// One call. `Ok(Ok(json))` on 2xx, `Ok(Err(exit))` after printing the failure.
fn call(
    ctx: &Ctx,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(String, String)],
    authed: bool,
) -> Result<Result<(u16, Value), i32>, String> {
    let key = if authed {
        Some(ctx.key.as_deref().ok_or_else(no_key_error)?)
    } else {
        ctx.key.as_deref()
    };
    let (status, text) = if headers.is_empty() {
        request(ctx, method, path, body, key)?
    } else {
        request_with(ctx, method, path, body, key, headers)?
    };
    if (200..300).contains(&status) {
        let v = serde_json::from_str(&text).unwrap_or(Value::Null);
        return Ok(Ok((status, v)));
    }
    let (msg, code) = failure(status, &text, &format!("{method} {path}"));
    crate::exit::print(code, &msg);
    Ok(Err(code))
}

fn read_document(source: Option<&&str>) -> Result<Value, String> {
    let text = match source {
        Some(&"-") => read_stdin().map_err(usage)?,
        Some(path) => read_file_capped(path).map_err(usage)?,
        None if !std::io::stdin().is_terminal() => read_stdin().map_err(usage)?,
        None => {
            return Err(usage(
                "no document: pass a <file>, or pipe JSON and use `-`",
            ))
        }
    };
    serde_json::from_str(&text).map_err(|e| usage(format!("input is not valid JSON: {e}")))
}

/// `.html`/`.htm` and `.svg` files are pages to upload, not JSON documents.
fn page_ext(path: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(path).extension()?.to_str()?;
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => Some("html"),
        "svg" => Some("svg"),
        _ => None,
    }
}

/// `fig.html` -> `fig-light.html` in the same folder.
fn light_sibling(path: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or_default();
    p.with_file_name(format!("{stem}-light.{ext}"))
}

fn read_page(path: &str, max: u64) -> Result<String, String> {
    let len = std::fs::metadata(path)
        .map_err(|e| usage(format!("cannot read {path}: {e}")))?
        .len();
    if len > max {
        return Err(usage(format!(
            "{path} is {}, over the {} upload limit",
            size(len as usize),
            size(max as usize)
        )));
    }
    std::fs::read_to_string(path).map_err(|e| usage(format!("cannot read {path}: {e}")))
}

/// Wrap a page file as the app's upload payload, checked against the
/// schema's `upload` block. The format is the enum value ending in `-<ext>`,
/// so the app names it, not the CLI.
fn page_payload<'s>(
    spec: &'s Value,
    app: &str,
    path: &str,
    ext: &str,
    light: Option<&str>,
) -> Result<(Value, &'s Value), String> {
    let bin = crate::help::invoked_bin();
    let upload = spec.get("upload").and_then(|u| u.get("json_schema"));
    let Some(schema) = upload else {
        return Err(usage(format!(
            "the {app} app takes JSON documents, not .{ext} pages\n{} {bin} apps schema {app}",
            term::dim("see:")
        )));
    };
    let formats: Vec<&str> = schema
        .pointer("/properties/format/enum")
        .and_then(Value::as_array)
        .map(|v| v.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let suffix = format!("-{ext}");
    let format = formats
        .iter()
        .find(|f| f.ends_with(&suffix))
        .ok_or_else(|| {
            usage(format!(
                "the {app} app does not take .{ext} uploads (formats: {})",
                if formats.is_empty() {
                    "none".to_string()
                } else {
                    formats.join(", ")
                }
            ))
        })?;
    let max = spec
        .pointer("/upload/max_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(crate::api::MAX_INPUT_BYTES);
    let mut payload = json!({ "format": format, "content": read_page(path, max)? });
    let light_path = match light {
        Some(l) => Some(std::path::PathBuf::from(l)),
        None => Some(light_sibling(path)).filter(|p| p.is_file()),
    };
    if let Some(lp) = light_path {
        if schema.pointer("/properties/content_light").is_none() {
            return Err(usage(format!("the {app} app takes no light build")));
        }
        let lp = lp.to_string_lossy().into_owned();
        payload["content_light"] = json!(read_page(&lp, max)?);
        if light.is_none() {
            eprintln!("{} {lp}", term::dim("light build:"));
        }
    }
    Ok((payload, schema))
}

fn site_origin(base: &str) -> String {
    let b = base.trim_end_matches('/');
    b.strip_suffix("/api").unwrap_or(b).to_string()
}

fn size(bytes: usize) -> String {
    format!("{:.1} KiB", bytes as f64 / 1024.0)
}

pub fn run_publish(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    crate::cmd::dispatch::usage_exit(publish_inner(parsed, env))
}

fn publish_inner(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let args: Vec<&str> = parsed.passthrough.iter().map(String::as_str).collect();
    let bin = crate::help::invoked_bin();
    let app = app_arg(args.first(), &format!("{bin} publish <app> [file|-]"))?;
    let visibility = get_string_flag(&parsed.flags, "visibility")
        .map(|v| visibility_arg(&v).map(str::to_string))
        .transpose()?;
    let page = args
        .get(1)
        .copied()
        .and_then(|path| page_ext(path).map(|ext| (path, ext)));
    let light = get_string_flag(&parsed.flags, "light");
    if light.is_some() && page.is_none() {
        return Err(usage("--light goes with an .html or .svg page"));
    }
    let document = match page {
        Some(_) => None,
        None => Some(read_document(args.get(1))?),
    };
    let ctx = context(parsed, env);

    let schema_path = format!("/v1/apps/{app}/schema");
    let spec = match call(&ctx, "GET", &schema_path, None, &[], false)? {
        Ok((_, v)) => v,
        Err(code) => return Ok(code),
    };
    let (payload, json_schema) = match (document, page) {
        (Some(doc), _) => {
            let schema = spec.get("json_schema").ok_or_else(|| {
                format!(
                    "{} {schema_path} returned no json_schema",
                    term::err("error:")
                )
            })?;
            (doc, schema)
        }
        (None, Some((path, ext))) => page_payload(&spec, &app, path, ext, light.as_deref())?,
        (None, None) => unreachable!("a document is read whenever there is no page"),
    };
    let problems = schema::validate(json_schema, &payload);
    let bytes = payload.to_string().len();
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("  {p}");
        }
        eprintln!(
            "{} document does not match the {app} schema ({} problem{}); nothing uploaded.\n{} {bin} apps schema {app}",
            term::err("error:"),
            problems.len(),
            if problems.len() == 1 { "" } else { "s" },
            term::dim("see:"),
        );
        return Ok(2);
    }
    let mut body = json!({ "payload": payload });
    if let Some(v) = &visibility {
        body["visibility"] = json!(v);
    }
    let path = format!("/v1/apps/{app}/submissions");
    if parsed.flag_true("dry-run") {
        // The server runs every check (the schema covers types only); nothing is stored.
        let check = format!("{path}?validate_only=1");
        if let Err(code) = call(&ctx, "POST", &check, Some(&body.to_string()), &[], true)? {
            return Ok(code);
        }
        let version = spec
            .get("version")
            .map(|v| {
                format!(
                    " v{}",
                    v.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string())
                )
            })
            .unwrap_or_default();
        println!(
            "ok: valid against {app}{version} ({}); not uploaded",
            size(bytes)
        );
        return Ok(0);
    }
    if visibility.as_deref() == Some("public") {
        confirm(
            parsed,
            "Publish publicly? It is listed after review and anyone can open it.",
        )?;
    }

    let headers = [(
        "Idempotency-Key".to_string(),
        idempotency_key(&app, &payload),
    )];
    let (status, out) = match call(&ctx, "POST", &path, Some(&body.to_string()), &headers, true)? {
        Ok(r) => r,
        Err(code) => return Ok(code),
    };
    if parsed.flag_true("json") {
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(0);
    }
    let url = out
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{} server reply had no url", term::err("error:")))?;
    let url = if url.starts_with('/') {
        format!("{}{url}", site_origin(&ctx.base))
    } else {
        url.to_string()
    };
    println!("{url}");
    if status == 200 {
        eprintln!(
            "{}",
            term::dim("already published (same content); reusing the original")
        );
        let got = out.get("visibility").and_then(Value::as_str);
        if let (Some(want), Some(got)) = (visibility.as_deref(), got) {
            if want != got {
                eprintln!(
                    "{} it is {got}, not {want}: change it with `{bin} apps visibility {app} <id> {want}`",
                    term::dim("note:")
                );
            }
        }
    }
    if out.get("status").and_then(Value::as_str) == Some("pending_review") {
        eprintln!(
            "{}",
            term::dim("pending review before it is listed publicly")
        );
    }
    Ok(0)
}

pub fn run_apps(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    crate::cmd::dispatch::usage_exit(apps_inner(parsed, env))
}

fn apps_inner(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let args: Vec<&str> = parsed.passthrough.iter().map(String::as_str).collect();
    let bin = crate::help::invoked_bin();
    let Some(verb) = args.first().copied() else {
        print!("{}", apps_help());
        return Ok(0);
    };
    let verb = match verb {
        "list" => "ls",
        "delete" => "rm",
        "show" => "schema",
        other => other,
    };
    if !APPS_VERBS.iter().any(|(v, _)| *v == verb) {
        return Err(usage(format!(
            "unknown verb \"{verb}\"\n{} one of: {}",
            term::dim("hint:"),
            APPS_VERBS
                .iter()
                .map(|(v, _)| *v)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let app = app_arg(args.get(1), &format!("{bin} apps {verb} <app>"))?;
    let ctx = context(parsed, env);
    let id = |i: usize| -> Result<&str, String> {
        args.get(i)
            .copied()
            .filter(|s| {
                !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            })
            .ok_or_else(|| {
                usage(format!(
                    "missing <id>\n{} {bin} apps {verb} <app> <id>",
                    term::dim("usage:")
                ))
            })
    };
    let base = format!("/v1/apps/{app}");
    match verb {
        "schema" => {
            let (_, v) = match call(&ctx, "GET", &format!("{base}/schema"), None, &[], false)? {
                Ok(r) => r,
                Err(code) => return Ok(code),
            };
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            Ok(0)
        }
        "ls" => {
            let (_, v) = match call(
                &ctx,
                "GET",
                &format!("{base}/submissions?mine=1"),
                None,
                &[],
                true,
            )? {
                Ok(r) => r,
                Err(code) => return Ok(code),
            };
            crate::api::list(&ctx, v, &["id", "visibility", "status", "title", "url"])
        }
        "rm" => {
            let id = id(2)?;
            confirm(
                parsed,
                &format!("Delete {app}/{id}? This cannot be undone."),
            )?;
            let path = format!("{base}/submissions/{id}");
            if parsed.flag_true("if-exists") {
                // One DELETE; a 404 is success under --if-exists.
                let key = ctx.key.as_deref().ok_or_else(no_key_error)?;
                let (status, text) = request(&ctx, "DELETE", &path, None, Some(key))?;
                let tagged = crate::exit::tag(crate::exit::for_status(status), "");
                if crate::api::absent_ok(parsed, &tagged, "id", id) {
                    return Ok(0);
                }
                if !(200..300).contains(&status) {
                    let (msg, code) = failure(status, &text, &format!("DELETE {path}"));
                    crate::exit::print(code, &msg);
                    return Ok(code);
                }
                eprintln!("deleted {app}/{id}");
                return Ok(0);
            }
            match call(&ctx, "DELETE", &path, None, &[], true)? {
                Ok(_) => {
                    eprintln!("deleted {app}/{id}");
                    Ok(0)
                }
                Err(code) => Ok(code),
            }
        }
        _ => {
            let id = id(2)?;
            let want = args.get(3).ok_or_else(|| {
                usage(format!("missing <visibility>\n{} {bin} apps visibility <app> <id> private|unlisted|public", term::dim("usage:")))
            })?;
            let want = visibility_arg(want)?;
            let body = json!({ "visibility": want }).to_string();
            match call(
                &ctx,
                "PATCH",
                &format!("{base}/submissions/{id}"),
                Some(&body),
                &[],
                true,
            )? {
                Ok((_, v)) => {
                    if parsed.flag_true("json") {
                        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    } else {
                        let got = v.get("visibility").and_then(Value::as_str).unwrap_or(want);
                        let status = v.get("status").and_then(Value::as_str);
                        eprintln!("{app}/{id} is now {got}{}", visibility_note(got, status));
                    }
                    Ok(0)
                }
                Err(code) => Ok(code),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotency_key_ignores_key_order_but_not_content_or_app() {
        let a = idempotency_key("x", &json!({"a": 1, "b": [1, 2]}));
        let b = idempotency_key("x", &serde_json::from_str(r#"{"b":[1,2],"a":1}"#).unwrap());
        assert_eq!(a, b);
        assert_ne!(a, idempotency_key("y", &json!({"a": 1, "b": [1, 2]})));
        assert_ne!(a, idempotency_key("x", &json!({"a": 2, "b": [1, 2]})));
        assert!(a.starts_with("anyr-") && a.len() == 37, "{a}");
    }

    #[test]
    fn app_names_are_path_safe() {
        assert!(app_arg(Some(&"my-app2"), "u").is_ok());
        for bad in ["../x", "A", "a/b", ""] {
            assert!(app_arg(Some(&bad), "u").is_err(), "{bad}");
        }
        assert!(app_arg(None, "u").is_err());
    }

    #[test]
    fn site_origin_drops_the_api_prefix() {
        assert_eq!(
            site_origin("https://anyrouter.dev/api/"),
            "https://anyrouter.dev"
        );
    }
}
