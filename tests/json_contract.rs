//! Agent contract: every command and subcommand, run with `--json` and no TTY,
//! either prints JSON on stdout or fails with exactly one JSON error object on
//! stderr whose `exit` is the process exit code. The command list comes from
//! `anyr commands --json`, so a new command is covered without editing this
//! file.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

mod common;

const KEY: &str = "sk-ar-v1-testkey0123456789";

/// Runs that would start a long-lived process, open a browser or replace the
/// binary. Each still gets its error path checked by a usage case below.
const SKIP: &[&str] = &[
    "claude",
    "codex",
    "grok",
    "opencode",
    "pi",
    "pool",
    "qwen",
    "goose",
    "aider",
    "crush",
    "kilo",
    "fx",
    "menu",
    "upgrade",
    "login",
    "auth login",
    "account add",
    "relay start",
    "relay pair",
    "relay logs",
    "delegate",
    "task",
];

/// Answer every request with 200 `{}` until the test ends.
fn serve_ok() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/api", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut len = 0usize;
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            );
        }
    });
    base
}

fn anyr(args: &[&str], base: &str, home: &std::path::Path) -> (i32, String, String) {
    std::fs::write(
        home.join("config.yaml"),
        format!(
            "active_profile: default\nprofiles:\n  default:\n    api_key: {KEY}\n    base_url: {base}\n"
        ),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .env_clear()
        .env("PATH", home) // no agents, no `open`, no network tools
        .env("HOME", home)
        .env("ANYROUTER_HOME", home)
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .args(args)
        .arg("--json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Drain both pipes while waiting: a large output (`schema`) would fill
    // the pipe and block the child forever.
    let drain = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = r.read_to_string(&mut s);
            s
        })
    };
    let out = drain(Box::new(child.stdout.take().unwrap()));
    let err = drain(Box::new(child.stderr.take().unwrap()));
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            panic!("`anyr {} --json` did not finish in 20s", args.join(" "));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = child.wait().unwrap();
    (
        status.code().unwrap_or(-1),
        out.join().unwrap(),
        err.join().unwrap(),
    )
}

fn check(args: &[&str], base: &str, json_output: bool) -> Result<(), String> {
    let home = tempdir();
    let (code, out, err) = anyr(args, base, &home);
    let what = format!("`anyr {} --json` (exit {code})", args.join(" "));
    if out.contains('\x1b') || err.contains('\x1b') {
        return Err(format!("{what}: ANSI escape in output"));
    }
    if code == 0 {
        // `json: false` in the tree: the command has no JSON form (a shell
        // script, a launcher); only its errors must be JSON.
        if out.trim().is_empty() || !json_output {
            return Ok(()); // nothing to print (e.g. a quiet write)
        }
        return serde_json::from_str::<Value>(&out)
            .map(|_| ())
            .map_err(|e| format!("{what}: stdout is not JSON ({e}):\n{out}"));
    }
    // A failure: either a JSON result on stdout (e.g. `relay doctor` with a
    // failed check), or one JSON error on stderr carrying this exit code.
    if !out.trim().is_empty() {
        return serde_json::from_str::<Value>(&out)
            .map(|_| ())
            .map_err(|e| format!("{what}: failed with non-JSON stdout ({e}):\n{out}"));
    }
    let lines: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
    let [line] = lines.as_slice() else {
        return Err(format!(
            "{what}: want one JSON error line on stderr, got:\n{err}"
        ));
    };
    let v: Value = serde_json::from_str(line)
        .map_err(|e| format!("{what}: stderr is not JSON ({e}):\n{err}"))?;
    if v["error"]["exit"].as_i64() != Some(code as i64) {
        return Err(format!("{what}: error.exit does not match:\n{err}"));
    }
    if v["error"]["message"].as_str().is_none_or(str::is_empty) {
        return Err(format!("{what}: error.message is empty:\n{err}"));
    }
    Ok(())
}

fn tempdir() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "anyr-json-contract-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tree() -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .args(["commands", "--json"])
        .env("ANYR_NO_UPDATE", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    serde_json::from_slice(&out.stdout).expect("commands --json parses")
}

#[test]
fn every_command_and_subcommand_honours_the_json_contract() {
    let base = serve_ok();
    let tree = tree();
    let mut runs: Vec<(Vec<String>, bool)> = Vec::new();
    for c in tree["commands"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap().to_string();
        let json = c["json"].as_bool().unwrap();
        runs.push((vec![name.clone()], json));
        for sub in c["subcommands"].as_array().unwrap() {
            runs.push((vec![name.clone(), sub.as_str().unwrap().to_string()], json));
        }
    }
    assert!(runs.len() > 60, "tree looks too small: {}", runs.len());
    let mut failures = Vec::new();
    for (run, json) in &runs {
        let joined = run.join(" ");
        if SKIP.contains(&joined.as_str()) {
            continue;
        }
        let args: Vec<&str> = run.iter().map(String::as_str).collect();
        if let Err(e) = check(&args, &base, *json) {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn skipped_commands_still_return_json_usage_errors() {
    // The skipped runs above would launch things; a bad flag on each must
    // still fail before that, as JSON with exit 2. Launchers forward unknown
    // flags to the agent, so they get anyr's `--model` without a value.
    let base = serve_ok();
    let launchers = anyr_cli::registry::LAUNCHERS;
    for name in SKIP {
        let mut args: Vec<&str> = name.split(' ').collect();
        args.push(if launchers.contains(name) {
            "--model"
        } else {
            "--no-such-flag"
        });
        let home = tempdir();
        let (code, out, err) = anyr(&args, &base, &home);
        assert_eq!(code, 2, "anyr {name}: {out}{err}");
        let v: Value =
            serde_json::from_str(err.trim()).unwrap_or_else(|e| panic!("{name}: {e}: {err}"));
        assert_eq!(v["error"]["code"], "usage", "{name}");
    }
}

#[test]
fn anyr_output_env_turns_on_json_without_the_flag() {
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .args(["no-such-command"])
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_OUTPUT", "json")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(v["error"]["code"], "usage");
    assert!(v["error"]["hint"].as_str().unwrap().contains("--help"));
}

#[test]
fn missing_key_is_exit_4_auth_on_every_account_command() {
    for args in [["whoami"], ["usage"], ["logs"], ["transactions"]] {
        let home = tempdir();
        let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
            .env_clear()
            .env("HOME", &home)
            .env("ANYROUTER_HOME", &home)
            .env("ANYR_NO_UPDATE", "1")
            .args(args)
            .arg("--json")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(4), "{args:?}: {err}");
        let v: Value = serde_json::from_str(err.trim()).unwrap();
        assert_eq!(v["error"]["code"], "auth");
    }
}

#[test]
fn error_output_masks_a_key_echoed_by_the_server() {
    // WHY: a server error body can echo the caller's key; logs and agent
    // transcripts must never capture the full secret, in text or JSON mode.
    for json in [false, true] {
        let body = format!(r#"{{"error":{{"message":"bad key {}"}}}}"#, common::KEY);
        let (base, _rx) = common::serve(vec![(401, body)]);
        let mut args = vec!["api", "credits"];
        if json {
            args.push("--json");
        }
        let (code, out, err) = common::run(&base, &args, None);
        assert_eq!(code, 4, "json={json}: {err}");
        for stream in [&out, &err] {
            assert!(
                !stream.contains(common::KEY),
                "json={json}: full key leaked: {stream}"
            );
        }
        assert!(
            err.contains("bad key sk-ar-v1-testk…6789"),
            "json={json}: {err}"
        );
    }
}
