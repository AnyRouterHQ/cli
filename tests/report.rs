//! `last-error.json` and `anyr report`, through the built binary. Telemetry
//! is switched on toward a closed loopback port (nothing listens, nothing
//! leaves the machine); `gh` is a fake script on PATH, never the real one.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const SECRET: &str = "sk-ar-v1-reportsecret0123456789";

fn anyr(home: &Path, args: &[&str], extra: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.args(args)
        .env("NO_COLOR", "1")
        .env("HOME", home)
        .env("ANYROUTER_HOME", home.join("anyr"))
        .env("ANYR_TELEMETRY_URL", "http://127.0.0.1:9/i/v0/e/")
        .env("ANYR_NO_CATALOG", "1")
        .env_remove("CI")
        .env_remove("ANYR_NO_UPDATE")
        .env_remove("DO_NOT_TRACK")
        .env_remove("ANYR_NO_TELEMETRY")
        .env_remove("ANYR_TELEMETRY")
        .env_remove("ANYROUTER_API_KEY")
        .stdin(Stdio::null());
    for (k, v) in extra {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

fn last_error(home: &Path) -> PathBuf {
    home.join("anyr").join("last-error.json")
}

fn fail_once(home: &Path) {
    let out = anyr(
        home,
        &["models", "--no-such-flag", SECRET, "/home/alice/notes.txt"],
        &[],
    );
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

#[test]
fn a_failed_run_leaves_a_redacted_last_error_that_report_shows() {
    let home = common::unique_temp_dir("anyr-report-last");
    fail_once(&home);
    let raw = std::fs::read_to_string(last_error(&home)).expect("last-error.json");
    assert_eq!(anyr_cli::report::leaks(&raw), None, "{raw}");
    assert!(
        !raw.contains("reportsecret") && !raw.contains("alice"),
        "{raw}"
    );
    let rec: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(rec["command"], "models");
    assert_eq!(rec["exit"], 2);
    assert_eq!(rec["code"], "usage");

    let out = anyr(&home, &["report", "--last", "--json"], &[]);
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["last_error"]["command"], "models");
    assert_eq!(v["fingerprint"], rec["fingerprint"]);
    assert_eq!(anyr_cli::report::leaks(&v.to_string()), None);
    // `report` itself never overwrites the error it reports.
    assert_eq!(std::fs::read_to_string(last_error(&home)).unwrap(), raw);

    let out = anyr(&home, &["report", "--last"], &[]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("### Last error") && text.contains("fingerprint: "),
        "{text}"
    );
    let plain = anyr(&home, &["report", "--json"], &[]);
    let v: serde_json::Value = serde_json::from_slice(&plain.stdout).unwrap();
    assert!(
        v["last_error"].is_null(),
        "without --last there is no error"
    );
}

#[test]
fn nothing_is_recorded_while_telemetry_is_off() {
    let home = common::unique_temp_dir("anyr-report-off");
    let out = anyr(
        &home,
        &["models", "--no-such-flag"],
        &[("DO_NOT_TRACK", "1")],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(!last_error(&home).exists());
}

#[cfg(unix)]
#[test]
fn an_agents_own_exit_code_is_not_an_anyr_error() {
    use std::os::unix::fs::PermissionsExt;
    let home = common::unique_temp_dir("anyr-report-agent");
    let agent = home.join("fake-claude");
    std::fs::write(&agent, "#!/bin/sh\nexit 3\n").unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = anyr(
        &home,
        &[
            "claude",
            "--yes",
            "--no-mcp",
            "--key",
            "sk-ar-v1-fixture-key-0001",
            "--command-path",
            agent.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(!last_error(&home).exists());
}

/// A fake `gh` that logs its argv and stdin, answers `issue list` with
/// `$FAKE_GH_LIST` and prints a URL for create and comment.
#[cfg(unix)]
fn fake_gh(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        r#"#!/bin/sh
echo "ARGS $*" >> "$FAKE_GH_LOG"
case "$1 $2" in
  "issue list") printf '%s\n' "$FAKE_GH_LIST" ;;
  *) echo "STDIN" >> "$FAKE_GH_LOG"; cat >> "$FAKE_GH_LOG"; echo "https://github.com/AnyRouterHQ/cli/issues/7" ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

#[cfg(unix)]
#[test]
fn file_asks_first_off_a_terminal_and_never_runs_gh_without_yes() {
    let home = common::unique_temp_dir("anyr-report-yes");
    let bin = fake_gh(&home);
    let log = home.join("gh.log");
    fail_once(&home);
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let out = anyr(
        &home,
        &["report", "--last", "--file"],
        &[("PATH", &path), ("FAKE_GH_LOG", log.to_str().unwrap())],
    );
    assert_eq!(out.status.code(), Some(6), "{out:?}");
    assert!(!log.exists(), "gh ran without --yes");
}

#[cfg(unix)]
#[test]
fn file_opens_a_labelled_issue_then_comments_on_the_same_fingerprint() {
    let home = common::unique_temp_dir("anyr-report-gh");
    let bin = fake_gh(&home);
    let log = home.join("gh.log");
    fail_once(&home);
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let env = |list: &'static str| {
        vec![
            ("PATH", path.clone()),
            ("FAKE_GH_LOG", log.to_str().unwrap().to_string()),
            ("FAKE_GH_LIST", list.to_string()),
        ]
    };
    let run = |list: &'static str, title: &str| {
        let e = env(list);
        let pairs: Vec<(&str, &str)> = e.iter().map(|(k, v)| (*k, v.as_str())).collect();
        anyr(
            &home,
            &[
                "report", "--last", "--file", "--yes", "--json", "--title", title,
            ],
            &pairs,
        )
    };
    let out = run("", &format!("models broke {SECRET}"));
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["filed"], "issue");
    let fp = v["fingerprint"].as_str().unwrap().to_string();
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(
        logged.contains(&format!(
            "issue list -R AnyRouterHQ/cli --search {fp} --state open"
        )),
        "{logged}"
    );
    assert!(
        logged.contains("issue create -R AnyRouterHQ/cli --title=models broke <key>"),
        "{logged}"
    );
    assert!(logged.contains("--label captured"), "{logged}");
    assert!(
        logged.contains(&format!("fingerprint: {fp}")),
        "body names the fingerprint: {logged}"
    );
    assert!(
        !logged.contains("reportsecret") && !logged.contains("alice"),
        "{logged}"
    );
    assert_eq!(anyr_cli::report::leaks(&logged), None, "{logged}");

    std::fs::remove_file(&log).unwrap();
    let out = run("42", "again");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["filed"], "comment");
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(
        logged.contains("issue comment 42 -R AnyRouterHQ/cli --body-file -"),
        "{logged}"
    );
    assert!(!logged.contains("issue create"), "{logged}");
}

#[test]
fn without_gh_file_prints_a_prefilled_issue_url() {
    let home = common::unique_temp_dir("anyr-report-url");
    fail_once(&home);
    let empty = home.join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let out = anyr(
        &home,
        &["report", "--last", "--file", "--yes", "--json"],
        &[("PATH", empty.to_str().unwrap())],
    );
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["filed"], "url");
    let url = v["url"].as_str().unwrap();
    assert!(
        url.starts_with("https://github.com/AnyRouterHQ/cli/issues/new?labels=captured&title="),
        "{url}"
    );
    assert!(url.len() <= 8100);
    // Encoding turns `/` into %2F; check the decoded text, not the URL.
    let decoded = decode(url);
    assert_eq!(anyr_cli::report::leaks(&decoded), None, "{decoded}");
    assert!(!decoded.contains("reportsecret") && !decoded.contains("alice"));
    assert!(decoded.contains("### Last error"), "{decoded}");
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn a_hand_edited_last_error_cannot_smuggle_text_into_the_report() {
    let home = common::unique_temp_dir("anyr-report-edited");
    fail_once(&home);
    let planted = format!("1.0.0-/home/alice/x {SECRET}");
    let mut rec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(last_error(&home)).unwrap()).unwrap();
    rec["version"] = planted.as_str().into();
    std::fs::write(last_error(&home), rec.to_string()).unwrap();
    let state = home.join("anyr").join("update-state.json");
    std::fs::write(
        &state,
        format!(r#"{{"latest_seen":"{planted}","pending_version":"0.1.0-beta.1 sk-ar-v1-x","channel":"/home/alice"}}"#),
    )
    .unwrap();
    let out = anyr(&home, &["report", "--last", "--json"], &[]);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(anyr_cli::report::leaks(&text), None, "{text}");
    assert!(
        !text.contains("alice") && !text.contains("reportsecret"),
        "{text}"
    );
}
