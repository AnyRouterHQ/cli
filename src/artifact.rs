//! `anyr artifact` — publish a page or folder and manage the result.
//!
//!   anyr artifact publish <file|dir> [--title T] [--visibility V] [--id ID] [--label L]
//!                         [--show-author|--no-show-author]
//!   anyr artifact ls [--limit N] [--cursor C | --all] | rm <id> | visibility <id> [<v>] [--show-author|--no-show-author]
//!
//! stdout carries the URL (or the server JSON with `--json`); progress and
//! errors go to stderr. Exit codes: 0 ok, 1 error, 2 usage or version
//! conflict, 3 not found, 4 auth.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::api::{confirm, context, exit_for, request, Ctx};
use crate::cmd::dispatch::{usage_exit, USAGE};
use crate::parse::{get_string_flag, ParsedArgs};
use crate::term;

pub const FLAGS: &[&str] = &[
    "profile",
    "base-url",
    "config",
    "key",
    "json",
    "yes",
    "title",
    "visibility",
    "id",
    "label",
    "limit",
    "cursor",
    "all",
    "show-author",
    "no-show-author",
];

pub const VERBS: &[(&str, &str)] = &[
    ("publish", "Publish a file or folder, print its URL"),
    ("ls", "List your artifacts"),
    ("rm", "Delete an artifact"),
    (
        "visibility",
        "Set private, unlisted or public, and whether your name shows",
    ),
];

const VISIBILITIES: [&str; 3] = ["private", "unlisted", "public"];

/// The server's name for a visibility. `link` is the old name for `unlisted`
/// and stays accepted so existing scripts keep working.
fn canonical_visibility(v: &str) -> Option<&'static str> {
    match v {
        "link" => Some("unlisted"),
        _ => VISIBILITIES.iter().copied().find(|known| *known == v),
    }
}
/// A single file is sent inline in one JSON body, so it keeps the inline cap.
const MAX_INLINE_BYTES: u64 = 5 * 1024 * 1024;
/// Folder caps match the server: 25 MB per asset (the server holds page and
/// source files to 5 MB and says so), 200 files and 100 MB per publish.
const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_FILES: usize = 200;
const MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024;

/// A request that did not succeed, carrying the exit code to return.
struct Fail {
    code: i32,
    message: String,
}

type Res<T> = Result<T, Fail>;

impl Fail {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// One file in a folder publish. Built once so the manifest and the upload
/// loop share the same hash and size.
struct Entry {
    rel: String,
    abs: PathBuf,
    size: u64,
    sha256: String,
}

pub fn run(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    usage_exit(run_inner(parsed, env))
}

fn run_inner(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let bin = crate::help::invoked_bin();
    let args: Vec<&str> = parsed.passthrough.iter().map(String::as_str).collect();
    let Some(verb) = args.first().copied() else {
        return Err(format!(
            "{USAGE}Usage: {bin} artifact <{}>",
            VERBS.iter().map(|(v, _)| *v).collect::<Vec<_>>().join("|")
        ));
    };
    let rest = &args[1..];
    let need = |what: &str| -> Result<&str, String> {
        rest.first()
            .copied()
            .ok_or_else(|| format!("{USAGE}Usage: {bin} artifact {verb} <{what}>"))
    };
    let ctx = context(parsed, env);
    let outcome = match verb {
        "publish" => publish(&ctx, parsed, need("file|dir")?),
        "ls" | "list" => list(&ctx, parsed),
        "rm" | "delete" => remove(&ctx, parsed, need("id")?),
        "visibility" => {
            let id = need("id")?;
            set_visibility(&ctx, parsed, id, rest.get(1).copied())
        }
        other => {
            return Err(format!(
                "{USAGE}unknown artifact command \"{other}\"; use: {}",
                VERBS.iter().map(|(v, _)| *v).collect::<Vec<_>>().join(", ")
            ))
        }
    };
    match outcome {
        Ok(()) => Ok(0),
        Err(f) => {
            crate::exit::print(f.code, &f.message);
            Ok(f.code)
        }
    }
}

fn publish(ctx: &Ctx, parsed: &ParsedArgs, path: &str) -> Res<()> {
    let visibility = visibility_flag(parsed)?;
    let show_author = show_author_flag(parsed)?;
    let id = match get_string_flag(&parsed.flags, "id") {
        Some(id) => Some(checked_id(&id)?),
        None => None,
    };
    let src = Path::new(path);
    let meta = fs::metadata(src).map_err(|e| usage(format!("Cannot read {path}: {e}")))?;
    let entries = if meta.is_dir() {
        Some(scan_dir(src)?)
    } else {
        None
    };
    // Local checks first, so a bad path never costs a network call.
    let inline_files = match &entries {
        Some(_) => None,
        None => Some(inline_file(src, meta.len())?),
    };
    ctx.key().map_err(|e| Fail::new(4, e))?;
    if visibility.as_deref() == Some("public") {
        confirm_public(parsed, "Publish this artifact as public?")?;
    }

    let mut body = Map::new();
    if let Some(t) = get_string_flag(&parsed.flags, "title") {
        body.insert("title".into(), json!(t));
    }
    if let Some(v) = &visibility {
        body.insert("visibility".into(), json!(v));
    }
    if let Some(l) = get_string_flag(&parsed.flags, "label") {
        body.insert("label".into(), json!(l));
    }
    // Sent only when asked: servers without the field reject unknown keys.
    if let Some(s) = show_author {
        body.insert("show_author".into(), json!(s));
    }
    let versions_path = match &id {
        Some(id) => {
            let current = current_version(ctx, id)?;
            body.insert("base_version".into(), json!(current));
            format!("/v1/artifacts/{id}/versions")
        }
        None => "/v1/artifacts".to_string(),
    };

    let done = match (entries, inline_files) {
        (Some(entries), _) => {
            body.insert("manifest".into(), manifest(&entries));
            let (_, started) = call(ctx, "POST", &versions_path, Some(&Value::Object(body)))?;
            let upload = started
                .get("upload_id")
                .and_then(Value::as_str)
                .ok_or_else(|| Fail::new(1, "error: server returned no upload_id"))?;
            upload_missing(ctx, upload, &started, &entries)?;
            let commit = format!("/v1/artifacts/uploads/{upload}/commit");
            call(ctx, "POST", &commit, Some(&json!({})))?.1
        }
        (None, Some(files)) => {
            body.insert("files".into(), files);
            call(ctx, "POST", &versions_path, Some(&Value::Object(body)))?.1
        }
        (None, None) => unreachable!("a path is either a directory or a file"),
    };
    print_published(ctx, &done)
}

/// `{ "<name>": { "content": ..., "encoding": "utf8" } }` for one file.
fn inline_file(src: &Path, size: u64) -> Res<Value> {
    if size > MAX_INLINE_BYTES {
        return Err(usage(format!(
            "{} is {} bytes; the limit is {MAX_INLINE_BYTES} for a single file.",
            src.display(),
            size
        )));
    }
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| usage(format!("{} has no usable file name.", src.display())))?;
    let content = fs::read_to_string(src).map_err(|e| {
        usage(format!(
            "Cannot read {} as UTF-8 text ({e}). Put binary files in a folder and publish the folder.",
            src.display()
        ))
    })?;
    Ok(json!({ name: { "content": content, "encoding": "utf8" } }))
}

fn manifest(entries: &[Entry]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|e| json!({ "path": e.rel, "size": e.size, "sha256": e.sha256 }))
            .collect(),
    )
}

fn upload_missing(ctx: &Ctx, upload: &str, started: &Value, entries: &[Entry]) -> Res<()> {
    let missing: Vec<&str> = started
        .get("missing")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|m| m.get("sha256").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    let key = ctx.key().map_err(|e| Fail::new(4, e))?;
    let mut pending: Vec<&Entry> = Vec::new();
    for entry in entries {
        // Identical files share one blob; upload each hash once.
        if missing.contains(&entry.sha256.as_str())
            && !pending.iter().any(|p| p.sha256 == entry.sha256)
        {
            pending.push(entry);
        }
    }
    for (i, entry) in pending.iter().enumerate() {
        eprintln!(
            "{}",
            term::dim(&format!(
                "uploading {}/{} {}",
                i + 1,
                pending.len(),
                entry.rel
            ))
        );
        // The manifest already promised this hash and size; a file edited
        // since would be rejected by the server, so stop before sending it.
        verify_unchanged(entry)?;
        let path = format!("/v1/artifacts/uploads/{upload}/files/{}", entry.sha256);
        let url = crate::http::join_api(&ctx.base, &path);
        let (status, text) =
            crate::http::http_put_file(&url, Some(key), &entry.abs).map_err(|e| Fail::new(1, e))?;
        if let Err(f) = check(ctx.key.as_deref(), status, &text) {
            // A rejected blob may mean the file changed mid-upload; say so.
            verify_unchanged(entry)?;
            return Err(f);
        }
    }
    Ok(())
}

/// Re-hash a file against its manifest entry. A mismatch means it changed
/// after the manifest was built, so the publish fails and nothing is
/// committed.
fn verify_unchanged(entry: &Entry) -> Res<()> {
    let size = fs::metadata(&entry.abs).map(|m| m.len()).ok();
    if size == Some(entry.size) && hash_file(&entry.abs)? == entry.sha256 {
        return Ok(());
    }
    Err(Fail::new(
        1,
        format!(
            "{} {} changed while publishing; nothing was committed. Run the command again.",
            term::err("error:"),
            entry.rel
        ),
    ))
}

fn print_published(ctx: &Ctx, done: &Value) -> Res<()> {
    if ctx.json {
        println!("{}", serde_json::to_string_pretty(done).unwrap_or_default());
        return Ok(());
    }
    let url = done
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| Fail::new(1, "error: server returned no url"))?;
    println!("{url}");
    if let Some(v) = done.get("version").and_then(Value::as_u64) {
        eprintln!("{} published version {v}", term::ok("✓"));
    }
    if let Some(hint) = private_hint(done) {
        eprintln!("{hint}");
    }
    Ok(())
}

/// A private link opens a sign-in page for everyone but the owner, so say so
/// and give the one command that makes it shareable.
fn private_hint(done: &Value) -> Option<String> {
    if done.get("visibility").and_then(Value::as_str) != Some("private") {
        return None;
    }
    let id = done.get("id").and_then(Value::as_str)?;
    let bin = crate::help::invoked_bin();
    Some(format!(
        "Private: only you can open this link. To share it, run: {bin} artifact visibility {id} unlisted\n(or publish with --visibility unlisted)"
    ))
}

/// Hard stop for `ls --all`, so a server that keeps handing back cursors
/// cannot loop the CLI forever.
const MAX_PAGES: usize = 1000;

fn list(ctx: &Ctx, parsed: &ParsedArgs) -> Res<()> {
    let limit = match get_string_flag(&parsed.flags, "limit") {
        Some(n) => Some(
            n.trim()
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=100).contains(n))
                .ok_or_else(|| usage("--limit must be a whole number from 1 to 100."))?,
        ),
        None => None,
    };
    let cursor = get_string_flag(&parsed.flags, "cursor");
    let all = parsed.flag_true("all");
    if all && cursor.is_some() {
        return Err(usage("--all and --cursor cannot be used together."));
    }
    let page_path = |cursor: Option<&str>| {
        let mut q = Vec::new();
        if let Some(n) = limit {
            q.push(format!("limit={n}"));
        }
        if let Some(c) = cursor {
            q.push(format!("cursor={}", crate::api::encode(c)));
        }
        if q.is_empty() {
            "/v1/artifacts".to_string()
        } else {
            format!("/v1/artifacts?{}", q.join("&"))
        }
    };
    let (_, mut v) = call(ctx, "GET", &page_path(cursor.as_deref()), None)?;
    if all {
        let mut rows = crate::api::rows(&v);
        let mut seen: Vec<String> = Vec::new();
        let mut pages = 1;
        while let Some(next) = next_cursor(&v) {
            if seen.contains(&next) || pages >= MAX_PAGES {
                return Err(Fail::new(
                    1,
                    format!(
                        "{} the server kept returning cursors (stopped after {pages} pages); use --cursor to continue by hand.",
                        term::err("error:")
                    ),
                ));
            }
            seen.push(next.clone());
            let (_, page) = call(ctx, "GET", &page_path(Some(&next)), None)?;
            rows.extend(crate::api::rows(&page));
            v = page;
            pages += 1;
        }
        v = json!({ "data": rows, "next_cursor": null });
    } else if let Some(next) = next_cursor(&v) {
        eprintln!(
            "{}",
            term::dim(&format!(
                "more artifacts exist; run with --cursor {next}, or use --all."
            ))
        );
    }
    crate::api::list(
        ctx,
        v,
        &["id", "title", "visibility", "current_version", "url"],
    )
    .map(|_| ())
    .map_err(|e| Fail::new(1, e))
}

fn next_cursor(page: &Value) -> Option<String> {
    page.get("next_cursor")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
}

fn remove(ctx: &Ctx, parsed: &ParsedArgs, id: &str) -> Res<()> {
    let id = checked_id(id)?;
    ctx.key().map_err(|e| Fail::new(4, e))?;
    confirm(
        parsed,
        &format!("Delete artifact {id}? Its URL stops working."),
    )
    .map_err(|e| Fail::new(2, strip_usage(&e)))?;
    call(ctx, "DELETE", &format!("/v1/artifacts/{id}"), None)?;
    if ctx.json {
        println!("{}", json!({ "id": id, "deleted": true }));
    } else {
        eprintln!("{} deleted {id}", term::ok("✓"));
    }
    Ok(())
}

fn set_visibility(ctx: &Ctx, parsed: &ParsedArgs, id: &str, v: Option<&str>) -> Res<()> {
    let id = checked_id(id)?;
    let show_author = show_author_flag(parsed)?;
    let v = match v {
        Some(v) => Some(canonical_visibility(v).ok_or_else(|| {
            usage(format!(
                "unknown visibility \"{v}\"; use private, unlisted or public."
            ))
        })?),
        None if show_author.is_some() => None,
        None => {
            let bin = crate::help::invoked_bin();
            return Err(usage(format!(
                "Usage: {bin} artifact visibility <id> <v> [--show-author|--no-show-author]"
            )));
        }
    };
    ctx.key().map_err(|e| Fail::new(4, e))?;
    if v == Some("public") {
        confirm_public(parsed, "Make this artifact public?")?;
    }
    let mut body = Map::new();
    if let Some(v) = v {
        body.insert("visibility".into(), json!(v));
    }
    if let Some(s) = show_author {
        body.insert("show_author".into(), json!(s));
    }
    let (_, out) = call(
        ctx,
        "PATCH",
        &format!("/v1/artifacts/{id}"),
        Some(&Value::Object(body)),
    )?;
    if ctx.json {
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    } else {
        if let Some(v) = v {
            eprintln!("{} {id} is now {v}", term::ok("✓"));
        }
        match show_author {
            Some(true) => eprintln!("{} {id} shows your name", term::ok("✓")),
            Some(false) => eprintln!("{} {id} hides your name", term::ok("✓")),
            None => {}
        }
    }
    Ok(())
}

fn confirm_public(parsed: &ParsedArgs, question: &str) -> Res<()> {
    confirm(parsed, question).map_err(|e| Fail::new(2, strip_usage(&e)))
}

fn current_version(ctx: &Ctx, id: &str) -> Res<u64> {
    let (_, v) = call(ctx, "GET", &format!("/v1/artifacts/{id}"), None)?;
    v.get("current_version")
        .or_else(|| v.pointer("/artifact/current_version"))
        .and_then(Value::as_u64)
        .ok_or_else(|| Fail::new(1, format!("error: artifact {id} has no current_version")))
}

/// One authenticated call. Non-2xx becomes a `Fail` with the mapped exit
/// code; 409 reports the current version and exits 2.
fn call(ctx: &Ctx, method: &str, path: &str, body: Option<&Value>) -> Res<(u16, Value)> {
    let key = ctx.key().map_err(|e| Fail::new(4, e))?;
    let body = body.map(Value::to_string);
    let (status, text) =
        request(ctx, method, path, body.as_deref(), Some(key)).map_err(|e| Fail::new(1, e))?;
    check(ctx.key.as_deref(), status, &text)?;
    let value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok((status, value))
}

/// Server error text must never echo the caller's key back to a terminal or log.
fn redact_key(text: &str, key: Option<&str>) -> String {
    match key {
        Some(k) if k.len() >= 4 => text.replace(k, "[redacted]"),
        _ => text.to_string(),
    }
}

fn check(key: Option<&str>, status: u16, text: &str) -> Res<()> {
    let redacted = redact_key(text, key);
    let text = redacted.as_str();
    if (200..300).contains(&status) {
        return Ok(());
    }
    let value: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let message = value
        .pointer("/error/message")
        .or_else(|| value.get("error"))
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            let peek: String = text
                .split_whitespace()
                .take(30)
                .collect::<Vec<_>>()
                .join(" ");
            if peek.is_empty() {
                "no response body".into()
            } else {
                peek
            }
        });
    if status == 409 {
        let current = value
            .pointer("/error/current_version")
            .or_else(|| value.get("current_version"))
            .and_then(Value::as_u64);
        let mut out = format!("{} version conflict: {message}", term::err("error:"));
        if let Some(n) = current {
            let _ = write!(
                out,
                "\ncurrent version is {n}; publish again with --id to build on it."
            );
        }
        return Err(Fail::new(2, out));
    }
    let mut out = format!("{} HTTP {status}: {message}", term::err("error:"));
    if status == 401 || status == 403 {
        let _ = write!(
            out,
            "\n{} sign in again: {} login",
            term::dim("hint:"),
            crate::help::invoked_bin()
        );
    }
    Err(Fail::new(exit_for(status), out))
}

fn visibility_flag(parsed: &ParsedArgs) -> Res<Option<String>> {
    match get_string_flag(&parsed.flags, "visibility") {
        None => Ok(None),
        Some(v) => match canonical_visibility(&v) {
            Some(known) => Ok(Some(known.to_string())),
            None => Err(usage(format!(
                "unknown visibility \"{v}\"; use private, unlisted or public."
            ))),
        },
    }
}

/// `--show-author` / `--no-show-author`; None when neither is given.
fn show_author_flag(parsed: &ParsedArgs) -> Res<Option<bool>> {
    match (
        parsed.flag_true("show-author"),
        parsed.flag_true("no-show-author"),
    ) {
        (true, true) => Err(usage(
            "--show-author and --no-show-author cannot be used together.",
        )),
        (true, false) => Ok(Some(true)),
        (false, true) => Ok(Some(false)),
        (false, false) => Ok(None),
    }
}

/// Ids go into URL paths, so only the characters the server mints are allowed.
fn checked_id(id: &str) -> Res<String> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(id.to_string())
    } else {
        Err(usage(format!("\"{id}\" is not a valid artifact id.")))
    }
}

fn usage(message: impl Into<String>) -> Fail {
    Fail::new(2, format!("{} {}", term::err("error:"), message.into()))
}

fn strip_usage(e: &str) -> String {
    e.strip_prefix(USAGE).unwrap_or(e).to_string()
}

/// Walk a folder in sorted order, skipping dotfiles, `node_modules` and
/// symlinks, and enforce the publish caps before any request is made.
fn scan_dir(root: &Path) -> Res<Vec<Entry>> {
    let mut found = Vec::new();
    walk(root, root, &mut found)?;
    found.sort_by(|a, b| a.rel.cmp(&b.rel));
    if found.is_empty() {
        return Err(usage(format!(
            "{} has no files to publish.",
            root.display()
        )));
    }
    if found.len() > MAX_FILES {
        return Err(usage(format!(
            "{} files found; the limit is {MAX_FILES} per publish.",
            found.len()
        )));
    }
    let total: u64 = found.iter().map(|e| e.size).sum();
    if total > MAX_TOTAL_BYTES {
        return Err(usage(format!(
            "{total} bytes found; the limit is {MAX_TOTAL_BYTES} per publish."
        )));
    }
    // Hash only once the count and size caps pass.
    for entry in &mut found {
        entry.sha256 = hash_file(&entry.abs)?;
    }
    Ok(found)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<Entry>) -> Res<()> {
    let read =
        fs::read_dir(dir).map_err(|e| usage(format!("Cannot read {}: {e}", dir.display())))?;
    for item in read {
        let item = item.map_err(|e| usage(format!("Cannot read {}: {e}", dir.display())))?;
        let Some(name) = item.file_name().to_str().map(str::to_string) else {
            return Err(usage(format!(
                "{} has a file name that is not valid UTF-8; rename it before publishing.",
                item.path().display()
            )));
        };
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let kind = item
            .file_type()
            .map_err(|e| usage(format!("Cannot stat {name}: {e}")))?;
        let abs = item.path();
        if kind.is_dir() {
            walk(root, &abs, out)?;
        } else if kind.is_file() {
            let size = item
                .metadata()
                .map_err(|e| usage(format!("Cannot stat {name}: {e}")))?
                .len();
            if size > MAX_FILE_BYTES {
                return Err(usage(format!(
                    "{} is {size} bytes; the limit is {MAX_FILE_BYTES} per file.",
                    abs.display()
                )));
            }
            let rel = abs
                .strip_prefix(root)
                .unwrap_or(&abs)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            out.push(Entry {
                rel,
                abs,
                size,
                sha256: String::new(),
            });
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Res<String> {
    let mut file =
        fs::File::open(path).map_err(|e| usage(format!("Cannot read {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| usage(format!("Cannot read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_cannot_escape_the_url_path() {
        // WHY: the id is spliced into /v1/artifacts/<id>; "../" must never reach it.
        assert!(checked_id("k3Jd9sQw1xYz0AbC").is_ok());
        assert!(checked_id("../keys").is_err());
        assert!(checked_id("a/b").is_err());
        assert!(checked_id("").is_err());
    }

    #[test]
    fn conflict_reports_current_version_and_exits_2() {
        let f = check(
            None,
            409,
            r#"{"error":{"code":"version_conflict","message":"stale","current_version":4}}"#,
        )
        .err()
        .unwrap();
        assert_eq!(f.code, 2);
        assert!(f.message.contains("current version is 4"), "{}", f.message);
    }

    #[test]
    fn statuses_map_to_exit_codes() {
        assert_eq!(check(None, 401, "{}").err().unwrap().code, 4);
        assert_eq!(check(None, 404, "{}").err().unwrap().code, 3);
        assert_eq!(check(None, 500, "{}").err().unwrap().code, 1);
        assert!(check(None, 201, "{}").is_ok());
    }
}
