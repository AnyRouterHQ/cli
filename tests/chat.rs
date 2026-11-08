//! `anyr chat` for scripts: `--json`, `--no-stream`, stdin, exit codes.

mod common;
use common::{run, serve};
use serde_json::Value;

const COMPLETION: &str = r#"{"id":"chatcmpl-1","model":"z-ai/glm-4.7-flash",
 "anyrouter_metadata":{"requestId":"req_abc"},
 "choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],
 "usage":{"prompt_tokens":5,"completion_tokens":1}}"#;

#[test]
fn json_sends_one_non_stream_request_and_prints_one_object() {
    let (base, rx) = serve(vec![(200, COMPLETION.into())]);
    let (code, out, err) = run(
        &base,
        &[
            "chat",
            "Reply with ok",
            "--model",
            "z-ai/glm-4.7-flash",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{err}");
    let seen = rx.recv().unwrap();
    assert_eq!(seen.path, "/api/v1/chat/completions");
    let body: Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(body["stream"], false);
    assert_eq!(body["messages"][0]["content"], "Reply with ok");
    let v: Value = serde_json::from_str(&out).expect("stdout is one JSON object");
    // WHY: `id` is what `anyr generation <id>` takes, so a script can chain them.
    assert_eq!(v["id"], "req_abc");
    assert_eq!(v["content"], "ok");
    assert_eq!(v["finish_reason"], "stop");
    assert_eq!(v["usage"]["completion_tokens"], 1);
    assert_eq!(v["model"], "z-ai/glm-4.7-flash");
    // WHY: scripts read latency from `timing`; a non-stream reply has no
    // first-token time to report, so the key must be absent, not zero.
    assert!(v["timing"]["total_ms"].is_u64(), "{v}");
    assert!(v["timing"].get("ttft_ms").is_none(), "{v}");
}

#[test]
fn no_stream_prints_only_the_answer() {
    let (base, rx) = serve(vec![(200, COMPLETION.into())]);
    let (code, out, _) = run(&base, &["chat", "hi", "--no-stream"], None);
    assert_eq!(code, 0);
    assert_eq!(out, "ok\n");
    let body: Value = serde_json::from_str(&rx.recv().unwrap().body).unwrap();
    assert_eq!(body["stream"], false);
}

#[test]
fn a_dash_appends_piped_stdin_to_the_prompt() {
    let (base, rx) = serve(vec![(200, COMPLETION.into())]);
    let (code, _, err) = run(
        &base,
        &["chat", "Review this", "-", "--json"],
        Some("+ added line\n"),
    );
    assert_eq!(code, 0, "{err}");
    let body: Value = serde_json::from_str(&rx.recv().unwrap().body).unwrap();
    assert_eq!(body["messages"][0]["content"], "Review this + added line");
}

#[test]
fn piped_stdin_alone_is_the_prompt() {
    let (base, rx) = serve(vec![(200, COMPLETION.into())]);
    let (code, _, err) = run(&base, &["chat", "--json"], Some("from stdin\n"));
    assert_eq!(code, 0, "{err}");
    let body: Value = serde_json::from_str(&rx.recv().unwrap().body).unwrap();
    assert_eq!(body["messages"][0]["content"], "from stdin");
}

#[test]
fn failures_map_to_exit_codes_and_json_errors() {
    for (status, code, name) in [
        (401, 4, "auth"),
        (404, 3, "not_found"),
        (429, 5, "rate_limited"),
    ] {
        let (base, _rx) = serve(vec![(status, r#"{"error":{"message":"nope"}}"#.into())]);
        let (got, out, err) = run(&base, &["chat", "hi", "--json"], None);
        assert_eq!(got, code, "HTTP {status}: {err}");
        assert!(out.is_empty(), "{out}");
        let v: Value = serde_json::from_str(err.trim()).unwrap();
        assert_eq!(v["error"]["code"], name);
        assert_eq!(v["error"]["exit"], code);
        assert!(v["error"]["message"].as_str().unwrap().contains("nope"));
    }
}

#[test]
fn empty_input_is_a_usage_error_and_sends_nothing() {
    let (base, rx) = serve(vec![(200, COMPLETION.into())]);
    let (code, _, err) = run(&base, &["chat", "--json"], None);
    assert_eq!(code, 2, "{err}");
    assert!(rx
        .recv_timeout(std::time::Duration::from_millis(300))
        .is_err());
}

#[test]
fn a_server_error_that_echoes_the_key_is_masked() {
    // WHY: gateways echo the rejected Authorization value back; stderr lands in
    // CI logs and screenshots, so the raw key must never reach it.
    let key = "sk-ar-v1-abcdefghijklmnop1234567890";
    let body = format!(r#"{{"error":{{"message":"Invalid key {key} for request"}}}}"#);
    let (base, _rx) = serve(vec![(401, body)]);
    let (_, out, err) = run(&base, &["chat", "hi", "--no-stream"], None);
    assert!(!err.contains("abcdefghijklmnop1234567890"), "{err}");
    assert!(!out.contains("abcdefghijklmnop1234567890"), "{out}");
}

mod stream {
    use super::common::KEY;
    use serde_json::Value;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    const CHUNK_A: &str =
        r#"data: {"id":"gen-1","model":"cohere/x","choices":[{"delta":{"content":"AAA"}}]}"#;
    const CHUNK_B: &str =
        r#"data: {"id":"gen-1","model":"cohere/x","choices":[{"delta":{"content":"BBB"}}]}"#;
    const USAGE: &str = r#"data: {"id":"gen-1","model":"cohere/x","choices":[{"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":11,"cost":0.0005}}"#;
    const META: &str = r#"data: {"id":"req_abcdef0123456789","model":"cohere/x","choices":[],"anyrouter_metadata":{"requestId":"req_abcdef0123456789"}}"#;

    /// Serves one SSE reply: sends the first chunk, then waits for `release`
    /// before sending the rest. The request body goes out on `seen`.
    fn serve_sse() -> (String, mpsc::Sender<()>, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/api", listener.local_addr().unwrap());
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (seen_tx, seen_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).unwrap();
            seen_tx.send(String::from_utf8(body).unwrap()).unwrap();
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nx-request-id: req_hdr\r\nConnection: close\r\n\r\n";
            s.write_all(head.as_bytes()).unwrap();
            s.write_all(format!("{CHUNK_A}\n\n").as_bytes()).unwrap();
            s.flush().unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
            let rest = format!("{CHUNK_B}\n\n{USAGE}\n\n{META}\n\ndata: [DONE]\n\n");
            let _ = s.write_all(rest.as_bytes());
        });
        (base, release_tx, seen_rx)
    }

    fn spawn(base: &str, stdout: Stdio, stderr: Stdio, extra: &[&str]) -> Child {
        Command::new(env!("CARGO_BIN_EXE_anyr"))
            .env("ANYR_NO_UPDATE", "1")
            .env("NO_COLOR", "1")
            .env("ANYROUTER_API_KEY", KEY)
            .env(
                "ANYROUTER_HOME",
                std::env::temp_dir().join("anyr-common-test-empty"),
            )
            .args(["chat", "hi", "--model", "z-ai/glm-4.7-flash"])
            .args(extra)
            .args(["--base-url", base])
            .stdout(stdout)
            .stderr(stderr)
            .stdin(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn deltas_reach_a_pipe_before_the_stream_ends_and_no_footer_is_added() {
        let (base, release, seen) = serve_sse();
        let mut child = spawn(&base, Stdio::piped(), Stdio::piped(), &[]);
        let body: Value = serde_json::from_str(&seen.recv().unwrap()).unwrap();
        // WHY: usage only arrives on a stream when the request asks for it.
        assert_eq!(body["stream_options"]["include_usage"], true);
        let mut out = child.stdout.take().unwrap();
        let mut first = [0u8; 3];
        // The server is still holding the rest back, so this only returns if
        // the CLI flushed the first delta on its own.
        out.read_exact(&mut first).unwrap();
        assert_eq!(&first, b"AAA");
        release.send(()).unwrap();
        let mut rest = String::new();
        out.read_to_string(&mut rest).unwrap();
        // WHY: a pipe is a script; extra lines would corrupt what it parses.
        assert_eq!(rest, "BBB\n");
        let mut err = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        assert!(child.wait().unwrap().success(), "{err}");
        assert!(!err.contains("tokens"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_terminal_gets_one_footer_from_the_final_usage_chunk() {
        let (base, release, _seen) = serve_sse();
        release.send(()).unwrap();
        let (master, slave) = pty();
        // The footer needs both stdout and stderr on the terminal.
        let mut child = spawn(
            &base,
            Stdio::from(slave.try_clone().unwrap()),
            Stdio::from(slave),
            &[],
        );
        let mut text = String::new();
        let mut f = std::fs::File::from(master);
        let _ = f.read_to_string(&mut text);
        assert!(child.wait().unwrap().success());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "AAABBB", "{text:?}");
        let footer = lines[1];
        for want in [
            "cohere/x",
            "7→11 tokens",
            "first token",
            "$0.0005",
            "req_abcdef01",
        ] {
            assert!(footer.contains(want), "{want} missing in {footer:?}");
        }
        assert_eq!(lines.len(), 2, "{text:?}");
    }

    #[cfg(unix)]
    #[test]
    fn quiet_on_a_terminal_prints_no_footer() {
        let (base, release, _seen) = serve_sse();
        release.send(()).unwrap();
        let (master, slave) = pty();
        let mut child = spawn(&base, Stdio::from(slave), Stdio::piped(), &["--quiet"]);
        let mut text = String::new();
        let _ = std::fs::File::from(master).read_to_string(&mut text);
        assert!(child.wait().unwrap().success());
        assert_eq!(text.trim_end(), "AAABBB");
    }

    #[cfg(unix)]
    #[test]
    fn a_narrow_terminal_gets_a_footer_that_fits_one_row() {
        let (base, release, _seen) = serve_sse();
        release.send(()).unwrap();
        let (master, slave) = pty();
        resize(&master, 30);
        let mut child = spawn(
            &base,
            Stdio::from(slave.try_clone().unwrap()),
            Stdio::from(slave),
            &[],
        );
        let mut text = String::new();
        let _ = std::fs::File::from(master).read_to_string(&mut text);
        assert!(child.wait().unwrap().success());
        let lines: Vec<&str> = text.lines().collect();
        // WHY: a wrapped footer would take two rows, and the erase removes one.
        assert_eq!(lines.len(), 2, "{text:?}");
        assert!(lines[1].chars().count() <= 29, "{:?}", lines[1]);
        assert!(lines[1].starts_with("cohere/x"), "{:?}", lines[1]);
    }

    #[cfg(unix)]
    #[test]
    fn stdout_on_a_terminal_with_stderr_redirected_gets_no_footer() {
        let (base, release, _seen) = serve_sse();
        release.send(()).unwrap();
        let (master, slave) = pty();
        let mut child = spawn(&base, Stdio::from(slave), Stdio::piped(), &[]);
        let mut text = String::new();
        let _ = std::fs::File::from(master).read_to_string(&mut text);
        assert!(child.wait().unwrap().success());
        assert_eq!(text.trim_end(), "AAABBB");
    }

    /// An interactive session on one pty: stdin, stdout and stderr are the
    /// terminal. Everything the child writes is collected for inspection.
    #[cfg(unix)]
    struct Repl {
        child: Child,
        input: std::fs::File,
        output: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        drained: mpsc::Receiver<()>,
    }

    #[cfg(unix)]
    impl Repl {
        fn start(base: &str) -> Repl {
            let (master, slave) = pty();
            resize(&master, 120);
            let child = Command::new(env!("CARGO_BIN_EXE_anyr"))
                .env("ANYR_NO_UPDATE", "1")
                .env("NO_COLOR", "1")
                .env("ANYROUTER_API_KEY", KEY)
                .env(
                    "ANYROUTER_HOME",
                    std::env::temp_dir().join("anyr-common-test-empty"),
                )
                .args(["chat", "--model", "z-ai/glm-4.7-flash"])
                .args(["--base-url", base])
                .stdin(Stdio::from(slave.try_clone().unwrap()))
                .stdout(Stdio::from(slave.try_clone().unwrap()))
                .stderr(Stdio::from(slave))
                .spawn()
                .unwrap();
            let input = std::fs::File::from(master);
            let mut reader = input.try_clone().unwrap();
            let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let sink = output.clone();
            let (drained_tx, drained) = mpsc::channel();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n @ 1..) = reader.read(&mut buf) {
                    sink.lock().unwrap().extend_from_slice(&buf[..n]);
                }
                let _ = drained_tx.send(());
            });
            Repl {
                child,
                input,
                output,
                drained,
            }
        }

        /// Wait for the child to exit, then for the reader to drain the pty.
        /// The child's last writes can still sit in the pty buffer after it
        /// exits, so reading the text before EOF can miss the footer.
        fn finish(&mut self) -> std::process::ExitStatus {
            let status = self.child.wait().unwrap();
            if self.drained.recv_timeout(Duration::from_secs(10)).is_err() {
                panic!("pty never reached EOF: {:?}", self.text());
            }
            status
        }

        fn text(&self) -> String {
            String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
        }

        fn send(&mut self, s: &str) {
            self.input.write_all(s.as_bytes()).unwrap();
        }

        /// Wait until `needle` shows up after byte `from`; returns the index
        /// just past it.
        fn wait_for(&self, needle: &str, from: usize) -> usize {
            for _ in 0..200 {
                let t = self.text();
                if let Some(i) = t.get(from..).and_then(|rest| rest.find(needle)) {
                    return from + i + needle.len();
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            panic!("never saw {needle:?} in {:?}", self.text());
        }
    }

    #[cfg(unix)]
    const ERASE_LINE: &str = "\x1b[M";

    #[cfg(unix)]
    #[test]
    fn typing_ahead_during_a_reply_keeps_the_footer_instead_of_erasing() {
        let (base, release, _seen) = serve_sse();
        let mut repl = Repl::start(&base);
        let at = repl.wait_for("> ", 0);
        repl.send("hi\n");
        let at = repl.wait_for("AAA", at);
        // Typed while the reply is still streaming; its echo is on screen.
        repl.send("next\n/exit\n");
        repl.wait_for("next\r\n/exit\r\n", at);
        release.send(()).unwrap();
        assert!(repl.finish().success());
        let text = repl.text();
        // The echo of the typed lines lands inside the reply, untouched.
        assert!(text.contains("AAAnext\r\n/exit\r\nBBB"), "{text:?}");
        assert!(text.contains("req_abcdef01"), "{text:?}");
        // WHY: erasing now would delete a line of the reply or the typed text.
        assert!(!text.contains(ERASE_LINE), "{text:?}");
    }

    #[cfg(unix)]
    #[test]
    fn the_footer_is_erased_when_the_next_prompt_is_typed_after_the_reply() {
        let (base, release, _seen) = serve_sse();
        release.send(()).unwrap();
        let mut repl = Repl::start(&base);
        let at = repl.wait_for("> ", 0);
        repl.send("hi\n");
        let at = repl.wait_for("req_abcdef01", at);
        // Only type once the next prompt is up, so nothing is queued.
        let at = repl.wait_for("> ", at);
        let _ = at;
        repl.send("/exit\n");
        assert!(repl.finish().success());
        assert!(repl.text().contains(ERASE_LINE), "{:?}", repl.text());
    }

    #[cfg(unix)]
    fn resize(master: &std::os::fd::OwnedFd, cols: u16) {
        use std::os::fd::AsRawFd;
        let ws = libc::winsize {
            ws_row: 24,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let rc = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
        assert_eq!(rc, 0);
    }

    #[cfg(unix)]
    fn pty() -> (std::os::fd::OwnedFd, std::os::fd::OwnedFd) {
        use std::os::fd::FromRawFd;
        // Open both ends close-on-exec at open time (#223). openpty has no
        // such flag, and a parallel test could spawn a child in the window
        // between openpty and the fcntl fixup, leaking the pty past EOF.
        let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
        assert_ne!(master, -1, "posix_openpt");
        assert_eq!(unsafe { libc::grantpt(master) }, 0, "grantpt");
        assert_eq!(unsafe { libc::unlockpt(master) }, 0, "unlockpt");
        let name = unsafe { libc::ptsname(master) };
        assert!(!name.is_null(), "ptsname");
        let slave_name = unsafe { std::ffi::CStr::from_ptr(name) }.to_owned();
        let slave = unsafe {
            libc::open(
                slave_name.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            )
        };
        assert_ne!(slave, -1, "open slave");
        let (m, s) = (master, slave);
        unsafe {
            (
                std::os::fd::OwnedFd::from_raw_fd(m),
                std::os::fd::OwnedFd::from_raw_fd(s),
            )
        }
    }
}
