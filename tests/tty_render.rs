//! Drive the real `anyr` binary inside a pseudo-terminal, feed what it writes
//! into a VT100 emulator, and assert on the rendered screen. Raw bytes can look
//! fine while the screen is broken: a bare `\n` in raw mode moves the cursor
//! down without returning it to column 0, so a menu turns into a staircase.
//!
//! Each case runs under every shell found on the host (bash, zsh, fish, sh).
//! A missing shell is skipped with a message on stderr; set
//! `ANYR_PTY_REQUIRE_SHELLS=1` (CI does) to make a missing shell fail instead.
#![cfg(all(unix, feature = "native"))]

mod common;

use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{serve_routes, unique_temp_dir, KEY};

const SHELLS: [&str; 4] = ["bash", "zsh", "fish", "sh"];
const WIDTHS: [u16; 3] = [40, 80, 200];
const ROWS: u16 = 50;
const DEADLINE: Duration = Duration::from_secs(20);

const STATUS_ACTIONS: [&str; 5] = [
    "Switch account",
    "Change default model",
    "Log out",
    "Open dashboard",
    "Quit",
];

const ME: &str = r#"{"id":"user_1","email":"me@example.com","name":"Duyet","username":"duyet","balance":133.83}"#;

/// A key whose scope list is far wider than any terminal, so the row must be
/// cut to the width instead of wrapping onto the next line.
fn key_info() -> String {
    let endpoints: Vec<String> = (0..40)
        .map(|i| format!("\"/api/v1/endpoint-number-{i}\""))
        .collect();
    format!(
        r#"{{"data":{{"name":"laptop","is_free_tier":false,"last_used_at":"2026-10-27T15:31:41Z","allowed_endpoints":[{}],"allowed_models":null}}}}"#,
        endpoints.join(",")
    )
}

fn server() -> String {
    serve_routes(vec![
        ("/api/v1/me", 200, ME.into()),
        ("/api/v1/key", 200, key_info()),
        ("/api/v1/credits", 200, r#"{"balance":133.83}"#.into()),
    ])
}

fn find_shell(name: &str) -> Option<PathBuf> {
    ["/bin", "/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"]
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
}

/// Shells present on this host. A missing one is reported, never dropped
/// silently, and fails the run when CI asks for every shell.
fn shells() -> Vec<(&'static str, PathBuf)> {
    let require = std::env::var("ANYR_PTY_REQUIRE_SHELLS").is_ok_and(|v| v == "1");
    let mut found = Vec::new();
    for name in SHELLS {
        match find_shell(name) {
            Some(path) => found.push((name, path)),
            None if require => panic!("{name} is not installed; ANYR_PTY_REQUIRE_SHELLS=1"),
            None => eprintln!("SKIP {name}: not installed on this host"),
        }
    }
    assert!(!found.is_empty(), "no shell found at all");
    found
}

/// A throwaway HOME with fake agents on PATH, so the launcher lists the same
/// agents on every machine and no rc file of the developer leaks in.
struct Sandbox {
    home: PathBuf,
    bin: PathBuf,
}

impl Sandbox {
    /// Signed in against `base` through a saved profile, as after a login.
    fn new(base: &str) -> Self {
        let home = unique_temp_dir("anyr-tty");
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let anyrouter = home.join(".anyrouter");
        std::fs::create_dir_all(&anyrouter).unwrap();
        std::fs::write(
            anyrouter.join("config.yaml"),
            format!("active_profile: default\nprofiles:\n  default:\n    api_key: {KEY}\n    base_url: {base}\n"),
        )
        .unwrap();
        for agent in ["claude", "codex"] {
            let p = bin.join(agent);
            std::fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Sandbox { home, bin }
    }

    fn command(&self, shell: &Path, script: &str, env: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new(shell);
        cmd.arg("-c").arg(script).env_clear();
        cmd.env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("ANYROUTER_HOME", self.home.join(".anyrouter"))
            .env("ANYROUTER_API_KEY", KEY)
            .env("ANYR_NO_UPDATE", "1")
            .env("LANG", "C.UTF-8")
            .env("TERM", "xterm-256color");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// `exec '<anyr>' <args>`: single quotes parse the same in all four shells.
/// `ANYR_TTY_BIN` points the cases at another build, such as a release.
fn script(args: &str) -> String {
    let bin = std::env::var("ANYR_TTY_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_anyr").into());
    format!("exec '{bin}' {args}")
}

struct Pty {
    child: Child,
    writer: std::fs::File,
    parser: Arc<Mutex<vt100::Parser>>,
    raw: Arc<Mutex<Vec<u8>>>,
}

impl Pty {
    fn spawn(mut cmd: Command, cols: u16, rows: u16) -> Pty {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let mut ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: out-pointers are valid locals; name and termios may be null.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut::<libc::termios>(),
                // A raw pointer: Linux takes `*const`, macOS `*mut`.
                std::ptr::addr_of_mut!(ws),
            )
        };
        assert_eq!(rc, 0, "openpty failed");
        // SAFETY: openpty returned two fresh descriptors we now own.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        cmd.stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()));
        // SAFETY: setsid and ioctl are async-signal-safe; this makes the pty
        // the child's controlling terminal, as a real terminal emulator does.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().expect("spawn in pty");
        drop(slave);
        let reader = std::fs::File::from(master);
        let writer = reader.try_clone().unwrap();
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let raw = Arc::new(Mutex::new(Vec::new()));
        let (p, r) = (Arc::clone(&parser), Arc::clone(&raw));
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            // Linux answers EIO once the child side closes; that ends the read.
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                p.lock().unwrap().process(&buf[..n]);
                r.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        Pty {
            child,
            writer,
            parser,
            raw,
        }
    }

    fn screen(&self) -> vt100::Screen {
        self.parser.lock().unwrap().screen().clone()
    }

    fn raw(&self) -> Vec<u8> {
        self.raw.lock().unwrap().clone()
    }

    /// Wait until every `needle` is on screen; panic with the screen if not.
    fn wait_for(&mut self, needles: &[&str]) -> vt100::Screen {
        let start = Instant::now();
        loop {
            let screen = self.screen();
            let text = screen.contents();
            if needles.iter().all(|n| text.contains(n)) {
                return screen;
            }
            if start.elapsed() > DEADLINE {
                let _ = self.child.kill();
                panic!(
                    "timed out waiting for {needles:?}; screen:\n{}",
                    dump(&screen)
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).unwrap();
        self.writer.flush().unwrap();
    }

    fn finish(mut self) {
        let start = Instant::now();
        loop {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            if start.elapsed() > DEADLINE {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!("anyr did not exit; screen:\n{}", dump(&self.screen()));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// The screen as text, one numbered row per line, for failure messages and
/// the before/after captures.
fn dump(screen: &vt100::Screen) -> String {
    let (_, cols) = screen.size();
    let rows: Vec<String> = screen.rows(0, cols).collect();
    let last = rows.iter().rposition(|r| !r.trim().is_empty()).unwrap_or(0);
    rows[..=last]
        .iter()
        .map(|r| format!("|{}", r.trim_end()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn indent(row: &str) -> usize {
    row.chars().take_while(|c| *c == ' ').count()
}

/// The checks every screen must pass, at every width.
fn assert_plain_top_left(screen: &vt100::Screen, what: &str) {
    let (rows, cols) = screen.size();
    let text: Vec<String> = screen.rows(0, cols).collect();
    let shown = dump(screen);
    for (i, row) in text.iter().enumerate() {
        if row.trim().is_empty() {
            continue;
        }
        // WHY: the owner rule is plain text from the top-left. A staircase or
        // a centered card both push content right.
        assert!(
            indent(row) <= 4,
            "{what}: row {i} starts at column {} (centered or staircase):\n{shown}",
            indent(row)
        );
        // WHY: a wrapped row breaks label alignment and the picker's redraw.
        assert!(
            !screen.row_wrapped(i as u16),
            "{what}: row {i} wrapped past {cols} columns:\n{shown}"
        );
        for frame in ['╭', '╮', '╰', '╯', '│', '┌', '┐', '└', '┘'] {
            assert!(
                !row.contains(frame),
                "{what}: row {i} draws a frame ({frame}):\n{shown}"
            );
        }
    }
    let first = text.iter().position(|r| !r.trim().is_empty());
    assert!(first.is_some(), "{what}: blank screen");
    assert!(rows > 0);
}

/// `anyr auth status` on a TTY: summary rows, then the inline action picker.
fn status_in_pty(shell: &Path, cols: u16, env: &[(&str, &str)]) -> (vt100::Screen, Vec<u8>) {
    let sandbox = Sandbox::new(&server());
    let cmd = sandbox.command(shell, &script("auth status"), env);
    let mut pty = Pty::spawn(cmd, cols, ROWS);
    // The hint is the last line of the picker frame.
    let screen = pty.wait_for(&["Open dashboard", "Quit", "esc quit"]);
    pty.send(b"q");
    let raw = pty.raw();
    pty.finish();
    (screen, raw)
}

#[test]
fn status_menu_rows_start_at_one_column_in_every_shell_and_width() {
    for (name, shell) in shells() {
        for cols in WIDTHS {
            let (screen, _) = status_in_pty(&shell, cols, &[]);
            let what = format!("auth status, {name}, {cols} cols");
            assert_plain_top_left(&screen, &what);
            let text: Vec<String> = screen.rows(0, cols).collect();
            // WHY: the staircase bug. Every action must begin in the same
            // column; a bare LF in raw mode shifts each row right.
            let starts: Vec<usize> = STATUS_ACTIONS
                .iter()
                .map(|label| {
                    let row = text
                        .iter()
                        .find(|r| r.contains(label))
                        .unwrap_or_else(|| panic!("{what}: {label} missing:\n{}", dump(&screen)));
                    row[..row.find(label).unwrap()].chars().count()
                })
                .collect();
            assert!(
                starts.windows(2).all(|w| w[0] == w[1]),
                "{what}: menu rows start at columns {starts:?}:\n{}",
                dump(&screen)
            );
            // WHY: a value wider than the terminal is cut with an ellipsis on
            // its own row, not wrapped into the next label.
            let scopes = text
                .iter()
                .find(|r| r.starts_with("scopes"))
                .unwrap_or_else(|| panic!("{what}: scopes row missing:\n{}", dump(&screen)));
            assert!(
                scopes.contains('…'),
                "{what}: scopes not truncated:\n{}",
                dump(&screen)
            );
            // WHY: a paid key must never show a bare dash for its plan.
            let plan = text.iter().find(|r| r.starts_with("plan"));
            assert!(
                plan.is_some_and(|r| r.contains("paid")),
                "{what}: plan row should say paid:\n{}",
                dump(&screen)
            );
            if cols == 80 && name == "bash" {
                eprintln!("--- auth status, 80 cols ---\n{}", dump(&screen));
            }
        }
    }
}

/// Escape sequences that set colors or attributes (SGR, `ESC [ … m`).
fn has_sgr(bytes: &[u8]) -> bool {
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == 0x1b && bytes[i + 1] == b'[' {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'm' {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Output from the first byte anyr itself wrote, so a shell's own startup
/// bytes cannot pass or fail the check.
fn from_first<'a>(raw: &'a [u8], marker: &str) -> &'a [u8] {
    let m = marker.as_bytes();
    let at = raw
        .windows(m.len())
        .position(|w| w == m)
        .unwrap_or_else(|| panic!("{marker:?} not in output: {}", String::from_utf8_lossy(raw)));
    &raw[at..]
}

#[test]
fn no_color_drops_every_color_code_in_every_shell() {
    for (name, shell) in shells() {
        let (screen, raw) = status_in_pty(&shell, 80, &[("NO_COLOR", "1")]);
        assert_plain_top_left(&screen, &format!("NO_COLOR, {name}"));
        let out = from_first(&raw, "account");
        assert!(
            !has_sgr(out),
            "{name}: NO_COLOR output still sets colors:\n{:?}",
            String::from_utf8_lossy(out)
        );
    }
}

#[test]
fn dumb_terminal_gets_plain_text_and_a_typed_prompt_in_every_shell() {
    for (name, shell) in shells() {
        let sandbox = Sandbox::new(&server());
        let cmd = sandbox.command(&shell, &script("auth status"), &[("TERM", "dumb")]);
        let mut pty = Pty::spawn(cmd, 80, ROWS);
        let screen = pty.wait_for(&["Quit", "q quit"]);
        assert_plain_top_left(&screen, &format!("TERM=dumb, {name}"));
        pty.send(b"q\n");
        let raw = pty.raw();
        pty.finish();
        // WHY: a dumb terminal cannot move the cursor or paint; any escape
        // byte shows up as garbage.
        let out = from_first(&raw, "account");
        assert!(
            !out.contains(&0x1b),
            "{name}: TERM=dumb output has escape codes:\n{:?}",
            String::from_utf8_lossy(out)
        );
    }
}

#[test]
fn piped_output_has_no_escape_codes_in_every_shell() {
    for (name, shell) in shells() {
        let sandbox = Sandbox::new(&server());
        for args in ["auth status", "menu"] {
            let out = sandbox
                .command(&shell, &script(args), &[])
                .stdin(Stdio::null())
                .stderr(Stdio::piped())
                .stdout(Stdio::piped())
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(!text.trim().is_empty(), "{name} {args}: no output");
            assert!(
                !out.stdout.contains(&0x1b) && !out.stderr.contains(&0x1b),
                "{name} {args}: piped output has escape codes:\n{text:?}"
            );
        }
    }
}

#[test]
fn launcher_renders_plain_from_the_top_left_in_every_shell_and_width() {
    for (name, shell) in shells() {
        for cols in WIDTHS {
            let sandbox = Sandbox::new(&server());
            let cmd = sandbox.command(&shell, &script(""), &[]);
            let mut pty = Pty::spawn(cmd, cols, ROWS);
            // Snapshot while the launcher is still up: leaving the alternate
            // screen restores the shell's screen.
            // Credits fill in from a background lookup after the first frame.
            let screen = pty.wait_for(&["claude", "quit", "esc", "$133.83"]);
            let what = format!("launcher, {name}, {cols} cols");
            assert_plain_top_left(&screen, &what);
            let text: Vec<String> = screen.rows(0, cols).collect();
            // WHY: plain text starts on the first row, not in a vertically
            // centered card.
            let first = text.iter().position(|r| !r.trim().is_empty()).unwrap();
            assert!(
                first <= 1,
                "{what}: content starts on row {first}:\n{}",
                dump(&screen)
            );
            // The capabilities stay: account, credits, agents, configure, quit.
            for want in ["account", "credits", "claude", "codex", "model…", "quit"] {
                assert!(
                    screen.contents().contains(want),
                    "{what}: {want} missing:\n{}",
                    dump(&screen)
                );
            }
            if cols == 80 && name == "bash" {
                eprintln!("--- launcher, 80 cols ---\n{}", dump(&screen));
            }
            pty.send(b"\x1b");
            pty.finish();
        }
    }
}

#[test]
fn a_launch_from_the_launcher_prints_its_command_on_exit() {
    // WHY: every HUD action that launches or changes a setting ends with
    // the command that does the same without the HUD, so a user learns it.
    let sandbox = Sandbox::new(&server());
    let shell = PathBuf::from("/bin/sh");
    let cmd = sandbox.command(&shell, &script(""), &[("ANYR_NO_CATALOG", "1")]);
    let mut pty = Pty::spawn(cmd, 80, ROWS);
    pty.wait_for(&["claude", "quit"]);
    // The first row launches claude; the fake claude exits at once.
    pty.send(b"\r");
    let screen = pty.wait_for(&["anyr claude"]);
    assert!(
        screen.contents().lines().any(|l| l.trim() == "anyr claude"),
        "{}",
        dump(&screen)
    );
    pty.finish();
}
