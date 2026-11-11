//! In-place CLI spinner. Frames actually advance on a timer so a TTY never
//! shows a frozen loading glyph. Non-TTY prints a static status line instead.

use std::cell::RefCell;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

thread_local! {
    static ACTIVE_PAUSE: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

use crate::term::{self, BLUE, SUCCESS};

/// Braille spinner frames. Consecutive indices are visually distinct.
pub const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A running agent keeps the build it started with; new runs use the new one.
pub const START_USING: &str = "Run anyr to start using the new version.";

const DEFAULT_INTERVAL_MS: u64 = 80;
const DEFAULT_MIN_TICKS: usize = 4;

/// Reports the current terminal width in columns; re-read on every paint.
pub type WidthFn = Arc<dyn Fn() -> usize + Send + Sync>;

fn default_width() -> WidthFn {
    Arc::new(|| term::width().unwrap_or(80))
}

/// Clip a painted line to one column short of `width` so it never wraps.
fn clip(line: &str, width: usize) -> String {
    term::truncate_ansi(line, width.saturating_sub(1).max(1))
}

pub fn frame(index: usize) -> &'static str {
    FRAMES[index % FRAMES.len()]
}

/// One spinner line (`glyph message`). Tests use this to prove ticks change.
pub fn render(index: usize, message: &str) -> String {
    format!("{} {message}", frame(index))
}

/// Glyphs from [`FRAMES`] that appear in a captured transcript (including `\r` history).
pub fn frames_in(output: &str) -> Vec<&'static str> {
    FRAMES
        .iter()
        .copied()
        .filter(|g| output.contains(g))
        .collect()
}

fn interval_from_env() -> Duration {
    let ms = std::env::var("ANYR_SPINNER_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_INTERVAL_MS)
        .min(1_000);
    Duration::from_millis(ms)
}

fn min_ticks_from_env() -> usize {
    std::env::var("ANYR_SPINNER_MIN_TICKS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_MIN_TICKS)
}

fn lock_write(out: &Mutex<Box<dyn Write + Send>>, bytes: &[u8]) {
    let mut guard = out.lock().unwrap_or_else(|e| e.into_inner());
    let _ = guard.write_all(bytes);
    let _ = guard.flush();
}

fn paint_glyph(glyph: &str) -> String {
    term::paint(BLUE, glyph)
}

fn paint_ok_mark() -> String {
    term::paint(SUCCESS, "✔")
}

/// Live spinner on a TTY; a single status line otherwise.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    ticks: Arc<AtomicUsize>,
    handle: Option<JoinHandle<()>>,
    out: Arc<Mutex<Box<dyn Write + Send>>>,
    tty: bool,
    interval: Duration,
    min_ticks: usize,
    finished: bool,
    width: WidthFn,
}

impl Spinner {
    pub fn start(message: impl Into<String>) -> Self {
        Self::start_on(
            io::stdout(),
            io::stdout().is_terminal(),
            message,
            interval_from_env(),
            min_ticks_from_env(),
        )
    }

    pub fn start_on<W: Write + Send + 'static>(
        writer: W,
        tty: bool,
        message: impl Into<String>,
        interval: Duration,
        min_ticks: usize,
    ) -> Self {
        Self::start_on_width(writer, tty, message, interval, min_ticks, default_width())
    }

    pub fn start_on_width<W: Write + Send + 'static>(
        writer: W,
        tty: bool,
        message: impl Into<String>,
        interval: Duration,
        min_ticks: usize,
        width: WidthFn,
    ) -> Self {
        let message = message.into();
        let out: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(writer)));
        let stop = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        let ticks = Arc::new(AtomicUsize::new(0));
        let handle = if tty {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let out_t = Arc::clone(&out);
                let stop_t = Arc::clone(&stop);
                let pause_t = Arc::clone(&pause);
                let ticks_t = Arc::clone(&ticks);
                let msg = message.clone();
                let width_t = Arc::clone(&width);
                let interval = if interval.is_zero() {
                    Duration::from_millis(DEFAULT_INTERVAL_MS)
                } else {
                    interval
                };
                Some(thread::spawn(move || {
                    tick_loop(out_t, stop_t, pause_t, ticks_t, msg, interval, width_t);
                }))
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = (
                    Arc::clone(&out),
                    Arc::clone(&stop),
                    Arc::clone(&ticks),
                    interval,
                );
                lock_write(&out, format!("{message}\n").as_bytes());
                None
            }
        } else {
            lock_write(&out, format!("{message}\n").as_bytes());
            None
        };
        let spinner = Self {
            stop,
            pause: Arc::clone(&pause),
            ticks,
            handle,
            out,
            tty,
            interval,
            min_ticks,
            finished: false,
            width,
        };
        ACTIVE_PAUSE.with(|slot| {
            *slot.borrow_mut() = Some(Arc::clone(&spinner.pause));
        });
        spinner
    }

    /// Pause the live glyph, finish the current line, then print `msg`.
    /// Stops `(stable channel)warning:` glue when stderr shares the TTY.
    pub fn warn(&self, msg: &str) {
        warn_beside_spinner_on(&self.out, self.tty, &self.pause, self.interval, msg);
    }

    pub fn tick_count(&self) -> usize {
        self.ticks.load(Ordering::Relaxed)
    }

    fn wait_min_ticks(&self) {
        if !self.tty || self.min_ticks == 0 {
            return;
        }
        let cap = self.interval.saturating_mul(self.min_ticks as u32) + Duration::from_millis(250);
        let deadline = Instant::now() + cap;
        while self.ticks.load(Ordering::Relaxed) < self.min_ticks {
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn stop_thread(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn write_line(&self, line: &str) {
        lock_write(&self.out, line.as_bytes());
    }

    /// Replace the spinner with a checkmark success line and the restart hint.
    pub fn succeed(mut self, message: &str) {
        self.wait_min_ticks();
        self.stop_thread();
        self.finished = true;
        let mark = paint_ok_mark();
        let body = if self.tty {
            let line = clip(&format!("{mark} {message}"), (self.width)());
            format!("\r\x1b[2K{line}\n")
        } else {
            format!("{mark} {message}\n")
        };
        self.write_line(&body);
        self.write_line("\n");
        self.write_line(&format!("{}\n", term::dim(START_USING)));
    }

    /// Clear the spinner line so a following error can print cleanly.
    pub fn fail(mut self) {
        self.stop_thread();
        self.finished = true;
        if self.tty {
            self.write_line("\r\x1b[K");
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        ACTIVE_PAUSE.with(|slot| {
            *slot.borrow_mut() = None;
        });
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        if self.tty && !self.finished {
            lock_write(&self.out, b"\r\x1b[K");
        }
    }
}

/// Print an install warning on its own line. Pauses a live spinner first so
/// the glyph line is not glued to `warning:`.
pub fn warn_beside_spinner(msg: &str) {
    let pause = ACTIVE_PAUSE.with(|slot| slot.borrow().clone());
    let tty = io::stdout().is_terminal();
    if let Some(pause) = pause.as_ref() {
        pause.store(true, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(40));
    }
    if tty {
        let mut out = io::stdout();
        let _ = out.write_all(format!("\r\x1b[K\n{msg}\n\n").as_bytes());
        let _ = out.flush();
    } else {
        eprintln!("{msg}");
    }
    if let Some(pause) = pause.as_ref() {
        pause.store(false, Ordering::SeqCst);
    }
}

fn warn_beside_spinner_on(
    out: &Mutex<Box<dyn Write + Send>>,
    tty: bool,
    pause: &AtomicBool,
    interval: Duration,
    msg: &str,
) {
    pause.store(true, Ordering::SeqCst);
    thread::sleep(interval.max(Duration::from_millis(15)));
    if tty {
        lock_write(out, format!("\r\x1b[K\n{msg}\n\n").as_bytes());
    } else {
        lock_write(out, format!("{msg}\n").as_bytes());
    }
    pause.store(false, Ordering::SeqCst);
}

#[cfg(not(target_arch = "wasm32"))]
fn tick_loop(
    out: Arc<Mutex<Box<dyn Write + Send>>>,
    stop: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    ticks: Arc<AtomicUsize>,
    message: String,
    interval: Duration,
    width: WidthFn,
) {
    let mut index = 0usize;
    while !stop.load(Ordering::Relaxed) {
        if pause.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(5).min(interval));
            continue;
        }
        let glyph = paint_glyph(frame(index));
        let line = clip(&format!("{glyph} {message}"), width());
        lock_write(&out, format!("\r\x1b[2K{line}").as_bytes());
        ticks.fetch_add(1, Ordering::Relaxed);
        index = index.wrapping_add(1);
        let until = Instant::now() + interval;
        while Instant::now() < until
            && !stop.load(Ordering::Relaxed)
            && !pause.load(Ordering::Relaxed)
        {
            thread::sleep(Duration::from_millis(5).min(interval));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn long_message_is_clipped_so_frames_never_wrap() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let cols = 30;
        let spinner = Spinner::start_on_width(
            SharedBuf(Arc::clone(&buf)),
            true,
            "Updating v0.1.44-beta.833 -> v0.1.44-beta.837 (beta channel)",
            Duration::from_millis(10),
            5,
            Arc::new(move || cols),
        );
        spinner.succeed("Updated to v0.1.44-beta.837 from the beta channel today");
        let text = String::from_utf8_lossy(&buf.lock().unwrap()).into_owned();
        let body = text.split(START_USING).next().unwrap();
        let frames: Vec<&str> = body.split('\r').filter(|f| !f.is_empty()).collect();
        assert!(frames.len() >= 5, "expected several frames: {text:?}");
        for f in &frames {
            let visible = term::strip_ansi(f.trim_end_matches('\n'));
            let visible = visible.trim_end_matches('\n');
            assert!(
                term::text_width(visible) < cols,
                "frame wider than terminal: {f:?}"
            );
        }
        assert!(frames.iter().all(|f| f.starts_with("\x1b[2K")), "{text:?}");
        assert!(
            !body.trim_end().contains('\n'),
            "no newline before the final line: {text:?}"
        );
    }

    #[test]
    fn consecutive_frames_are_distinct() {
        for i in 0..FRAMES.len() {
            assert_ne!(
                frame(i),
                frame(i + 1),
                "frame {i} must differ from the next tick"
            );
            assert_ne!(render(i, "Updating"), render(i + 1, "Updating"));
        }
        assert_eq!(FRAMES.len(), 10);
    }

    #[test]
    fn live_spinner_writes_multiple_changing_frames() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let spinner = Spinner::start_on(
            SharedBuf(Arc::clone(&buf)),
            true,
            "Updating v0.1.11 -> v0.1.99 (stable channel)",
            Duration::from_millis(15),
            5,
        );
        spinner.succeed("Updated to v0.1.99");
        let bytes = buf.lock().unwrap().clone();
        let text = String::from_utf8_lossy(&bytes);
        let seen = frames_in(&text);
        assert!(
            seen.len() >= 2,
            "spinner must tick distinct frames, got {seen:?} in:\n{text:?}"
        );
        assert!(
            text.contains('\r'),
            "in-place animation uses CR, got {text:?}"
        );
        assert!(text.contains("✔"), "{text}");
        assert!(text.contains("Updated to v0.1.99"), "{text}");
        assert!(text.contains(START_USING), "{text}");
    }

    #[test]
    fn non_tty_prints_status_without_frozen_glyph() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let spinner = Spinner::start_on(
            SharedBuf(Arc::clone(&buf)),
            false,
            "Updating v0.1.11 -> v0.1.99 (beta channel)",
            Duration::from_millis(15),
            8,
        );
        assert_eq!(spinner.tick_count(), 0);
        spinner.succeed("Would update to v0.1.99");
        let text = String::from_utf8_lossy(&buf.lock().unwrap()).into_owned();
        assert!(
            text.contains("Updating v0.1.11 -> v0.1.99 (beta channel)"),
            "{text}"
        );
        assert!(
            frames_in(&text).is_empty(),
            "non-TTY must not print a frozen spinner glyph, got {text:?}"
        );
        assert!(text.contains("✔ Would update to v0.1.99"), "{text}");
        assert!(text.contains(START_USING), "{text}");
    }

    #[test]
    fn warn_starts_on_new_line_not_glued_to_channel() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let spinner = Spinner::start_on(
            SharedBuf(Arc::clone(&buf)),
            true,
            "Updating v0.1.14 -> v0.1.15 (stable channel)",
            Duration::from_millis(15),
            2,
        );
        spinner.warn("warning: release has no checksums.txt — skipping verification");
        spinner.succeed("Updated to v0.1.15");
        let text = String::from_utf8_lossy(&buf.lock().unwrap()).into_owned();
        assert!(
            !text.contains("(stable channel)warning:"),
            "warning must not glue to the spinner line:\n{text:?}"
        );
        let n = text
            .matches("warning: release has no checksums.txt")
            .count();
        assert_eq!(n, 1, "warning must print once, got {n} in:\n{text:?}");
        let stripped = text.replace("\x1b[K", "");
        assert!(
            stripped.contains("\nwarning:"),
            "expected a newline before warning, got {text:?}"
        );
    }
}
