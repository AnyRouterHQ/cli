//! `anyr report [--last] [--title T] [--json] [--file]`: print the redacted
//! bug report bundle (`report::bundle`), and with `--file` file it on
//! GitHub: through `gh` when it is on PATH (comment on an open issue with the
//! same fingerprint, else a new issue labelled `captured`), or as a
//! prefilled issue URL. Nothing is filed without `--file`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::parse::{get_string_flag, ParsedArgs};

const REPO: &str = "AnyRouterHQ/cli";
/// Longest prefilled issue URL; browsers and GitHub cut longer ones.
const MAX_URL: usize = 8000;

pub(crate) fn run(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let bundle = crate::report::bundle(env, parsed.flag_true("last"));
    let body = markdown(&bundle);
    if !parsed.flag_true("file") {
        if parsed.flag_true("json") {
            println!("{bundle}");
        } else {
            print!("{body}");
        }
        return Ok(0);
    }
    let title = title(parsed, &bundle);
    if !parsed.flag_true("json") {
        print!("{body}");
    }
    crate::api::confirm(parsed, "File this report as a GitHub issue?")?;
    let fp = bundle["fingerprint"].as_str().unwrap_or_default();
    let (how, url) = match find_gh(env) {
        Some(gh) => file_with_gh(&gh, fp, &title, &body)?,
        None => ("url", issue_url(&title, &body)),
    };
    if parsed.flag_true("json") {
        println!(
            "{}",
            json!({ "filed": how, "url": url, "fingerprint": bundle["fingerprint"] })
        );
    } else if how == "url" {
        println!("Open this to file it (gh is not installed):\n{url}");
    } else {
        println!("{url}");
    }
    Ok(0)
}

/// `--title`, redacted, or one built from closed-set fields.
fn title(parsed: &ParsedArgs, b: &Value) -> String {
    if let Some(t) = get_string_flag(&parsed.flags, "title").filter(|t| !t.trim().is_empty()) {
        return crate::report::redact(&t);
    }
    let fp = b["fingerprint"].as_str().unwrap_or("no fingerprint");
    if let Some(c) = b["crash"].as_object() {
        return format!(
            "crash in anyr {} ({fp})",
            c["command"].as_str().unwrap_or("other")
        );
    }
    if let Some(e) = b["last_error"].as_object() {
        return format!(
            "anyr {} failed: {} ({fp})",
            e["command"].as_str().unwrap_or("other"),
            e["error"].as_str().unwrap_or("error")
        );
    }
    "anyr report".into()
}

fn markdown(b: &Value) -> String {
    let s = |v: &Value| match v {
        Value::Null => "-".to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let mut out = format!(
        "## anyr report\n\n- version: {} ({})\n- os: {} {}\n- install: {}\n- fingerprint: {}\n",
        s(&b["version"]),
        s(&b["channel"]),
        s(&b["os"]),
        s(&b["arch"]),
        s(&b["install_method"]),
        s(&b["fingerprint"]),
    );
    if let Some(e) = b["last_error"].as_object() {
        out.push_str(&format!(
            "\n### Last error\n\n- command: {}\n- exit: {} ({}), error: {}\n- message: {}\n- version: {}\n",
            s(&e["command"]),
            s(&e["exit"]),
            s(&e["code"]),
            s(&e["error"]),
            s(&e["message"]),
            s(&e["version"]),
        ));
    }
    if let Some(c) = b["crash"].as_object() {
        let crumbs: Vec<String> = c["breadcrumbs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(s)
            .collect();
        out.push_str(&format!(
            "\n### Crash\n\n- command: {}\n- location: {}\n- message: {}\n- count: {}, version: {}\n- breadcrumbs: {}\n",
            s(&c["command"]),
            s(&c["location"]),
            s(&c["message"]),
            s(&c["count"]),
            s(&c["version"]),
            crumbs.join(" > "),
        ));
    }
    let u = &b["update"];
    out.push_str(&format!(
        "\n### Update\n\n- channel: {}, latest seen: {}, pending: {}, skipped: {}\n- last error: {}\n",
        s(&u["channel"]),
        s(&u["latest_seen"]),
        s(&u["pending_version"]),
        s(&u["skip_version"]),
        s(&u["last_error"]),
    ));
    let agents: Vec<String> = b["agents"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| format!("{k} {v}"))
        .collect();
    if !agents.is_empty() {
        out.push_str(&format!("\n### Agents\n\n- {}\n", agents.join(", ")));
    }
    out
}

/// `gh` (or `gh.exe`) on the run's PATH (`Path` on Windows). Relative
/// entries such as `.` are skipped, so a `gh` in a cloned repo never runs.
fn find_gh(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let path = env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
        .map(|(_, v)| v)?;
    let name = if cfg!(windows) { "gh.exe" } else { "gh" };
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Run `gh args…` with `stdin`, returning its trimmed stdout on success.
fn gh(bin: &PathBuf, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run gh: {e}"))?;
    if let Some(mut pipe) = child.stdin.take() {
        let _ = pipe.write_all(stdin.unwrap_or_default().as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("gh did not finish: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "gh {} failed: {}",
            args.first().copied().unwrap_or_default(),
            crate::report::redact(String::from_utf8_lossy(&out.stderr).trim())
        ))
    }
}

/// Comment on an open issue that names this fingerprint, or open one.
fn file_with_gh(
    bin: &PathBuf,
    fp: &str,
    title: &str,
    body: &str,
) -> Result<(&'static str, String), String> {
    if !fp.is_empty() {
        let found = gh(
            bin,
            &[
                "issue",
                "list",
                "-R",
                REPO,
                "--search",
                fp,
                "--state",
                "open",
                "--json",
                "number",
                "--jq",
                ".[0].number",
            ],
            None,
        )
        .unwrap_or_default();
        if let Ok(n) = found.parse::<u64>() {
            let n = n.to_string();
            let url = gh(
                bin,
                &["issue", "comment", &n, "-R", REPO, "--body-file", "-"],
                Some(body),
            )?;
            return Ok(("comment", url));
        }
    }
    // `--title=<t>`: a title can never be read as another flag.
    let title = format!("--title={title}");
    let create = |label: bool| {
        let mut args = vec![
            "issue",
            "create",
            "-R",
            REPO,
            title.as_str(),
            "--body-file",
            "-",
        ];
        if label {
            args.extend(["--label", "captured"]);
        }
        gh(bin, &args, Some(body))
    };
    // A repo without the label still gets the issue.
    let url = create(true).or_else(|_| create(false))?;
    Ok(("issue", url))
}

/// A prefilled new-issue URL, the body cut so the whole URL fits.
fn issue_url(title: &str, body: &str) -> String {
    let head = format!(
        "https://github.com/{REPO}/issues/new?labels=captured&title={}&body=",
        encode(title)
    );
    let mut body_enc = String::new();
    for c in body.chars() {
        let mut buf = [0u8; 4];
        let piece = encode(c.encode_utf8(&mut buf));
        if head.len() + body_enc.len() + piece.len() > MAX_URL {
            body_enc.push_str(&encode("\n…"));
            break;
        }
        body_enc.push_str(&piece);
    }
    head + &body_enc
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_issue_url_is_encoded_and_bounded() {
        let url = issue_url("a b&c", &"x ".repeat(10_000));
        assert!(url.starts_with("https://github.com/AnyRouterHQ/cli/issues/new?"));
        assert!(url.contains("title=a%20b%26c"));
        assert!(url.len() <= MAX_URL + 16);
        assert!(url.ends_with("%0A%E2%80%A6"));
    }
}
