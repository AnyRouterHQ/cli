//! Color TUI helpers. Truecolor GrokNight palette; silent when not a TTY or
//! when `NO_COLOR` is set. No extra crates — keep startup cheap.

use std::io::{self, IsTerminal, Write};

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";

pub const MAGENTA: &str = "\x1b[38;2;187;154;247m";
pub const BLUE: &str = "\x1b[38;2;122;162;247m";
pub const TEAL: &str = "\x1b[38;2;58;149;171m";
pub const SUCCESS: &str = "\x1b[38;2;158;206;106m";
/// Site orange #F6821F — brand accent (help, links, TUI).
pub const BRAND: &str = "\x1b[38;2;246;130;31m";
pub const ORANGE: &str = "\x1b[38;2;246;130;31m";
pub const YELLOW: &str = "\x1b[38;2;224;175;104m";
pub const DANGER: &str = "\x1b[38;2;247;118;142m";
pub const WHITE: &str = "\x1b[38;2;225;225;225m";
pub const MUTED: &str = "\x1b[38;2;108;108;108m";
pub const LABEL: &str = "\x1b[38;2;200;200;200m";

pub fn color_enabled() -> bool {
    if std::env::var_os("NO_COLOR").is_some() || dumb_terminal() {
        return false;
    }
    io::stdout().is_terminal()
}

/// `TERM=dumb`: the terminal cannot take colors or cursor moves.
pub fn dumb_terminal() -> bool {
    std::env::var("TERM").is_ok_and(|t| t.trim() == "dumb")
}

/// Columns of the terminal on stdout, else `COLUMNS`. `None` for a pipe,
/// where nothing wraps.
pub fn width() -> Option<usize> {
    #[cfg(feature = "native")]
    if io::stdout().is_terminal() {
        if let Ok((w, _)) = crossterm::terminal::size() {
            if w > 0 {
                return Some(w as usize);
            }
        }
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|n: &usize| *n > 0)
}

/// Display columns of plain text (wide characters count two).
pub fn text_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

/// Cut plain text to `max` display columns, ending in `…` when cut.
pub fn truncate(s: &str, max: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if text_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > max - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// Cut a line that may hold color codes to `max` visible columns. Escape
/// sequences pass through untouched and do not count; a cut line ends in
/// `…` plus a reset so the color does not bleed into the next line.
pub fn truncate_ansi(s: &str, max: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if text_width(&strip_ansi(s)) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            out.push(c);
            if chars.peek() == Some(&'[') {
                for n in chars.by_ref() {
                    out.push(n);
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        let w = c.width().unwrap_or(0);
        if used + w > max.saturating_sub(1) {
            break;
        }
        out.push(c);
        used += w;
    }
    if max > 0 {
        out.push('…');
    }
    if out.contains('\x1b') {
        out.push_str(RESET);
    }
    out
}

/// Drop `ESC [ … letter` sequences.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

pub fn paint(ansi: &str, text: &str) -> String {
    if !color_enabled() {
        return text.to_string();
    }
    format!("{ansi}{text}{RESET}")
}

pub fn bold(text: &str) -> String {
    paint(BOLD, text)
}

pub fn dim(text: &str) -> String {
    paint(MUTED, text)
}

pub fn accent(text: &str) -> String {
    paint(BRAND, text)
}

pub fn ok(text: &str) -> String {
    paint(SUCCESS, text)
}

pub fn warn(text: &str) -> String {
    paint(YELLOW, text)
}

pub fn err(text: &str) -> String {
    paint(DANGER, &crate::key::mask_secrets(text))
}

pub fn model_id(text: &str) -> String {
    paint(TEAL, text)
}

/// Official AR ligature as half-blocks. Source is `anyrouter-logo-white.png`,
/// not the 32px favicon. Half-blocks fill the cell so the mark stays readable
/// in fonts where braille dots collapse into noise.
pub const MARK_LINES: [&str; 5] = [
    "        ▄█▄▀█████████▄",
    "      ▄█████▄       ███",
    "    ▄███▀  ▀██▄ ▄▄▄███▀",
    "  ▄███▀      ▀██▄▀███▄",
    "▄███▀          ▀██▄▀███▄",
];

/// Compact 3-row AR mark for TUI chrome. Recovered from the original
/// half-block logo (`b7a313b`); not the later 5-line ligature.
pub const TUI_MARK_LINES: [&str; 3] = ["    ▄▄ ▄▄▄ ", "  ▄█▀▀█▄▄█▀", " ▀▀    ▀▀▀▀"];

const MARK_PNG: &[u8] = include_bytes!("../assets/mark.png");

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (a << 16) | (b << 8) | c;
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Graphics {
    Kitty,
    Iterm,
    Sixel,
}

/// Inline image protocol for this terminal.
///
/// - Kitty PNG: kitty, Ghostty
/// - iTerm OSC 1337 PNG: iTerm2, WezTerm, Warp, mintty, rio, Tabby, Blink, Prompt
/// - Sixel: Windows Terminal, Konsole, foot, mlterm, contour, Alacritty, eat, yaft
///
/// `ANYR_GRAPHICS=kitty|iterm|sixel|off` overrides detection.
/// Generic `xterm-256color` is not treated as Sixel (VTE/Apple/conhost share it).
/// tmux: Kitty/iTerm are wrapped in DCS passthrough; Sixel is left raw for tmux 3.4+.
fn graphics_kind() -> Option<Graphics> {
    if !io::stdout().is_terminal() {
        return None;
    }
    graphics_kind_from(|k| std::env::var(k).ok())
}

fn graphics_kind_from(get: impl Fn(&str) -> Option<String>) -> Option<Graphics> {
    let get = &get;
    let has = |k: &str| get(k).is_some();
    let val = |k: &str| get(k).unwrap_or_default();

    match val("ANYR_GRAPHICS").trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "0" | "false" => return None,
        "kitty" => return Some(Graphics::Kitty),
        "iterm" | "iterm2" | "osc1337" => return Some(Graphics::Iterm),
        "sixel" => return Some(Graphics::Sixel),
        _ => {}
    }

    let term = val("TERM").to_ascii_lowercase();
    let program = val("TERM_PROGRAM").to_ascii_lowercase();
    let emulator = val("TERMINAL_EMULATOR").to_ascii_lowercase();
    let lc_terminal = val("LC_TERMINAL").to_ascii_lowercase();

    // Multiplexers that swallow sequences (tmux is handled at emit time).
    if has("ZELLIJ") || has("STY") {
        return None;
    }
    if term.starts_with("screen") && !has("TMUX") {
        return None;
    }

    if matches!(
        program.as_str(),
        "apple_terminal"
            | "vscode"
            | "vscode-insiders"
            | "cursor"
            | "zed"
            | "hyper"
            | "terminus"
            | "jetbrains"
    ) || emulator.contains("jetbrains")
        || has("NVIM")
        || has("TERMUX_VERSION")
        || matches!(
            term.as_str(),
            "dumb" | "linux" | "vt100" | "vt102" | "vt220" | "ansi" | "cygwin"
        )
    {
        return None;
    }

    if has("KITTY_WINDOW_ID")
        || has("KITTY_PID")
        || has("GHOSTTY_RESOURCES_DIR")
        || has("GHOSTTY_BIN_DIR")
        || term.contains("kitty")
        || term.contains("ghostty")
        || program == "kitty"
        || program == "ghostty"
    {
        return Some(Graphics::Kitty);
    }

    if has("ITERM_SESSION_ID")
        || has("WEZTERM_EXECUTABLE")
        || has("WEZTERM_PANE")
        || lc_terminal == "iterm2"
        || matches!(
            program.as_str(),
            "iterm.app"
                | "wezterm"
                | "warpterminal"
                | "mintty"
                | "rio"
                | "tabby"
                | "blink"
                | "prompt"
        )
        || term.contains("wezterm")
        || term.contains("mintty")
        || term.contains("warp")
        || term == "rio"
        || term.starts_with("rio-")
    {
        return Some(Graphics::Iterm);
    }

    if has("WT_SESSION")
        || has("WT_PROFILE_ID")
        || has("KONSOLE_VERSION")
        || has("KONSOLE_DBUS_SERVICE")
        || has("ALACRITTY_WINDOW_ID")
        || has("ALACRITTY_SOCKET")
        || term.starts_with("foot")
        || term.contains("mlterm")
        || term.contains("contour")
        || term.contains("sixel")
        || term.contains("alacritty")
        || term.starts_with("yaft")
        || term.starts_with("eat-")
        || term.contains("darktile")
        || term.contains("domterm")
    {
        return Some(Graphics::Sixel);
    }

    None
}

fn tmux_passthrough(seq: &str) -> String {
    let escaped = seq.replace('\x1b', "\x1b\x1b");
    format!("\x1bPtmux;{escaped}\x1b\\")
}

/// Best-effort inline PNG/Sixel of the official AR mark.
fn emit_graphics_mark() -> bool {
    let Some(kind) = graphics_kind() else {
        return false;
    };
    let seq = match kind {
        Graphics::Kitty => {
            let b64 = base64_encode(MARK_PNG);
            format!("\x1b_Ga=T,f=100,c=12,r=4,q=2;{b64}\x1b\\")
        }
        Graphics::Iterm => {
            let b64 = base64_encode(MARK_PNG);
            format!("\x1b]1337;File=inline=1;width=12;height=4;preserveAspectRatio=1:{b64}\x07")
        }
        Graphics::Sixel => include_str!("../assets/mark.sixel").to_string(),
    };
    // Kitty/iTerm need tmux DCS passthrough (`allow-passthrough on`, tmux 3.3+).
    // Sixel is understood by tmux 3.4+ itself.
    let payload = if std::env::var_os("TMUX").is_some() && kind != Graphics::Sixel {
        tmux_passthrough(&seq)
    } else {
        seq
    };
    let mut out = io::stdout();
    if out.write_all(payload.as_bytes()).is_err() {
        return false;
    }
    let _ = out.write_all(b"\n");
    let _ = out.flush();
    true
}

/// One-line chip: ▲ anyr + official half-block mark + captions.
pub fn brand_chip(captions: &[&str]) -> String {
    let mut out = String::new();
    out.push_str(&accent("▲ anyr"));
    out.push_str("  ");
    out.push_str(&paint(WHITE, MARK_LINES[0].trim()));
    let first = captions.first().copied().unwrap_or("").trim();
    if !first.is_empty() {
        out.push_str("  ");
        out.push_str(first);
    }
    out.push('\n');
    for cap in captions.iter().skip(1) {
        if cap.is_empty() {
            continue;
        }
        out.push_str(cap);
        out.push('\n');
    }
    out
}

/// AR mark next to matching caption lines (5-row hero).
pub fn brand_header(captions: &[&str]) -> String {
    let mut out = String::new();
    for (i, mark_line) in MARK_LINES.iter().enumerate() {
        let mark = paint(WHITE, mark_line);
        let caption = captions.get(i).copied().unwrap_or("");
        out.push_str(&mark);
        if !caption.is_empty() {
            out.push_str("  ");
            out.push_str(caption);
        }
        out.push('\n');
    }
    out
}

/// Print the official AR mark: inline PNG/Sixel when the terminal can, else half-blocks.
pub fn print_brand_header(captions: &[&str]) {
    if emit_graphics_mark() {
        for caption in captions {
            if !caption.is_empty() {
                println!("{caption}");
            }
        }
        return;
    }
    print!("{}", brand_header(captions));
}

/// OSC 8 hyperlink when color/TTY is on; plain URL otherwise.
pub fn link(url: &str) -> String {
    if !color_enabled() {
        return url.to_string();
    }
    format!("\x1b]8;;{url}\x1b\\{BRAND}{url}{RESET}\x1b]8;;\x1b\\")
}

pub fn tool_color(tool: &str) -> &'static str {
    match tool {
        "claude" | "cc" => ORANGE,
        "codex" => SUCCESS,
        "grok" => WHITE,
        "opencode" => BLUE,
        "pi" => TEAL,
        "pool" | "poolside" => MAGENTA,
        "qwen" | "kilo" => BLUE,
        "goose" | "aider" => SUCCESS,
        "crush" => MAGENTA,
        "fx" => WHITE,
        _ => BRAND,
    }
}

/// One line of a label/value block: a row, or the thin line between groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelLine {
    /// `label` is padded to the block's label column; `value` is already cut
    /// to the width.
    Row {
        label: String,
        value: String,
    },
    Divider(usize),
}

/// Lay out `label  value` rows from column 0: labels padded to one column,
/// groups split by one thin line, every value cut with `…` so no row is wider
/// than `width`. Empty groups add nothing.
pub fn label_lines(groups: &[&[(&str, String)]], width: Option<usize>) -> Vec<LabelLine> {
    let rows = || groups.iter().flat_map(|g| g.iter());
    let label_w = rows().map(|(l, _)| text_width(l)).max().unwrap_or(0);
    let value_max = width.map(|w| w.saturating_sub(label_w + 2));
    let line_w = rows()
        .map(|(_, v)| label_w + 2 + text_width(v))
        .max()
        .unwrap_or(0)
        .min(60)
        .min(width.unwrap_or(usize::MAX));
    let mut out = Vec::new();
    for (i, group) in groups.iter().filter(|g| !g.is_empty()).enumerate() {
        if i > 0 {
            out.push(LabelLine::Divider(line_w));
        }
        for (label, value) in group.iter() {
            let pad = label_w - text_width(label);
            out.push(LabelLine::Row {
                label: format!("{label}{}", " ".repeat(pad)),
                value: match value_max {
                    Some(max) => truncate(value, max),
                    None => value.clone(),
                },
            });
        }
    }
    out
}

/// `label_lines` for the current terminal, painted: labels dimmed, a thin
/// line between groups. No frame. NO_COLOR drops the dim.
pub fn label_rows(groups: &[&[(&str, String)]]) -> String {
    let mut out = String::new();
    for line in label_lines(groups, width()) {
        match line {
            LabelLine::Row { label, value } => {
                out.push_str(&dim(&label));
                out.push_str("  ");
                out.push_str(&value);
            }
            LabelLine::Divider(w) => out.push_str(&divider(w)),
        }
        out.push('\n');
    }
    out
}

pub fn divider(width: usize) -> String {
    dim(&"─".repeat(width))
}

pub fn is_interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// True when an agent or script drives this run: `--json`, `ANYR_OUTPUT=json`,
/// `--quiet`, `ANYR_QUIET=1`, or no TTY on stdin/stdout. Such runs get no
/// update notices, no background update check and no prompts.
pub fn agent_mode(
    parsed: &crate::parse::ParsedArgs,
    env: &std::collections::BTreeMap<String, String>,
) -> bool {
    machine_output(parsed, env) || !is_interactive()
}

/// `agent_mode` without the TTY check: the run asked for machine output
/// (`--json`, `--quiet`, `ANYR_OUTPUT=json`, `ANYR_QUIET=1`).
pub fn machine_output(
    parsed: &crate::parse::ParsedArgs,
    env: &std::collections::BTreeMap<String, String>,
) -> bool {
    machine_output_flags(&parsed.flags, env)
}

/// `machine_output` for callers that hold only the parsed flags.
pub fn machine_output_flags(
    flags: &std::collections::HashMap<String, crate::parse::FlagValue>,
    env: &std::collections::BTreeMap<String, String>,
) -> bool {
    let on = |k: &str| {
        env.get(k)
            .is_some_and(|v| !matches!(v.trim(), "" | "0" | "false"))
    };
    let flag = |k: &str| matches!(flags.get(k), Some(crate::parse::FlagValue::Bool(true)));
    flag("json")
        || flag("quiet")
        || env
            .get("ANYR_OUTPUT")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("json"))
        || on("ANYR_QUIET")
}

pub fn prompt(label: &str) -> Result<String, String> {
    prompt_from(label, &mut io::stdin().lock())
}

/// `prompt` over an explicit reader, so tests never block on real stdin.
fn prompt_from(label: &str, input: &mut dyn io::BufRead) -> Result<String, String> {
    eprint!("{label}");
    let _ = io::stderr().flush();
    let mut line = String::new();
    input
        .read_line(&mut line)
        .map_err(|e| format!("Could not read input: {e}"))?;
    Ok(line.trim().to_string())
}

/// Read a line with terminal echo disabled (for API keys). Falls back to
/// normal reading when stdin is not a TTY or termios control fails.
///
/// Windows keeps echo on (pre-existing limitation; no new deps).
pub fn prompt_secret(label: &str) -> Result<String, String> {
    #[cfg(unix)]
    {
        if !io::stdin().is_terminal() {
            return prompt(label);
        }

        let fd = libc::STDIN_FILENO;
        // SAFETY: zeroed termios is only a write buffer for tcgetattr.
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: fd 0 is stdin; `original` is a valid termios buffer we own.
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return prompt(label);
        }

        let mut hidden = original;
        hidden.c_lflag &= !libc::ECHO;
        // SAFETY: `hidden` is a copy of the termios returned by tcgetattr.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) } != 0 {
            return prompt(label);
        }

        let result = prompt(label);
        // Restore echo even if read_line failed — restore before return.
        // SAFETY: `original` came from tcgetattr on this same fd.
        let _ = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &original) };
        let _ = io::stderr().write_all(b"\n");
        let _ = io::stderr().flush();
        result
    }
    #[cfg(not(unix))]
    {
        // Windows: documented limitation, same as before.
        prompt(label)
    }
}

pub fn confirm(question: &str) -> bool {
    match prompt(&format!("{question} [y/N] ")) {
        Ok(ans) => matches!(ans.to_ascii_lowercase().as_str(), "y" | "yes"),
        Err(_) => false,
    }
}

/// Interactive picker (Ratatui when `native`). `current` is pre-selected.
#[cfg(feature = "native")]
pub fn pick(title: &str, items: &[String], current: Option<usize>) -> Result<usize, String> {
    crate::tui::pick(title, items, current)
}

/// Render a compact on-screen menu (no alt-screen). Used by the HUD dump
/// and the arrow-key picker.
pub fn render_inline_menu(
    header: &[String],
    question: &str,
    items: &[String],
    cursor: usize,
    footer: &str,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    for h in header {
        if !h.is_empty() {
            lines.push(h.clone());
        }
    }
    if !header.is_empty() {
        lines.push(String::new());
    }
    if !question.is_empty() {
        lines.push(dim(question));
    }
    for (i, item) in items.iter().enumerate() {
        let n = i + 1;
        // The `›` marks the row without color, so NO_COLOR still shows it.
        if i == cursor {
            lines.push(format!("{} {}", accent(&format!("› {n}.")), item));
        } else {
            lines.push(format!("  {n}. {item}"));
        }
    }
    if !footer.is_empty() {
        lines.push(String::new());
        lines.push(dim(footer));
    }
    lines.join("\n")
}

/// Arrow-key picker on the current screen. No alt-screen, no "type a number".
pub fn pick_inline(
    header: &[String],
    question: &str,
    items: &[String],
    footer: &str,
    current: Option<usize>,
) -> Result<usize, String> {
    if items.is_empty() {
        return Err("Nothing to pick.".into());
    }
    #[cfg(feature = "native")]
    if is_interactive() && !dumb_terminal() {
        return pick_inline_raw(header, question, items, footer, current);
    }
    pick_numbered("anyr", items, current)
}

/// Write a frame while raw mode is on. Raw mode turns off the terminal's
/// newline translation, so a bare `\n` moves down without going back to
/// column 0 and every line starts where the last one ended. Each line ends in
/// `\r\n` and is cut to the terminal width: a wrapped line would also throw
/// off the line count the next redraw moves up by. Returns that count.
#[cfg(feature = "native")]
fn write_raw_frame(out: &mut impl Write, frame: &str) -> Result<u16, String> {
    let width = crossterm::terminal::size()
        .ok()
        .map(|(w, _)| w as usize)
        .filter(|w| *w > 0);
    let bytes = raw_frame_bytes(frame, width);
    out.write_all(bytes.as_bytes()).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    Ok(frame.lines().count().max(1) as u16)
}

/// The bytes `write_raw_frame` sends: each line cut to `width`, then `\r\n`.
pub fn raw_frame_bytes(frame: &str, width: Option<usize>) -> String {
    let mut out = String::with_capacity(frame.len() + 16);
    for line in frame.lines() {
        match width {
            // One column spare: writing the last column parks the cursor in
            // a pending-wrap state some terminals resolve as a new line.
            Some(w) => out.push_str(&truncate_ansi(line, w.saturating_sub(1))),
            None => out.push_str(line),
        }
        out.push_str("\r\n");
    }
    out
}

#[cfg(feature = "native")]
fn pick_inline_raw(
    header: &[String],
    question: &str,
    items: &[String],
    footer: &str,
    current: Option<usize>,
) -> Result<usize, String> {
    use std::io::stderr;

    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{self, Clear, ClearType};
    use crossterm::{cursor, execute, queue};

    terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let mut out = stderr();
            let _ = execute!(out, cursor::Show);
            let _ = terminal::disable_raw_mode();
        }
    }
    let _restore = Restore;
    let mut out = stderr();
    let _ = execute!(out, cursor::Hide);

    let mut cursor_i = current.unwrap_or(0).min(items.len() - 1);
    let mut last_lines: u16 = 0;
    loop {
        let frame = render_inline_menu(header, question, items, cursor_i, footer);
        if last_lines > 0 {
            queue!(
                out,
                cursor::MoveToColumn(0),
                cursor::MoveUp(last_lines),
                Clear(ClearType::FromCursorDown),
            )
            .map_err(|e| e.to_string())?;
        }
        last_lines = write_raw_frame(&mut out, &frame)?;

        match event::read().map_err(|e| e.to_string())? {
            Event::Key(ev) if ev.kind == KeyEventKind::Press => match ev.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    cursor_i = if cursor_i == 0 {
                        items.len() - 1
                    } else {
                        cursor_i - 1
                    };
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    cursor_i = (cursor_i + 1) % items.len();
                }
                KeyCode::Enter => return Ok(cursor_i),
                KeyCode::Esc => return Err("Cancelled.".into()),
                KeyCode::Char('q') if !ev.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Err("Cancelled.".into());
                }
                KeyCode::Char('c') if ev.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Err("Cancelled.".into());
                }
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

/// Render a checkbox list: `[x]`/`[ ]` per item, cursor row highlighted.
pub fn render_checkboxes(
    question: &str,
    items: &[String],
    checked: &[bool],
    cursor: usize,
) -> String {
    let mut lines = vec![dim(question)];
    for (i, item) in items.iter().enumerate() {
        let mark = if checked.get(i).copied().unwrap_or(false) {
            "[x]"
        } else {
            "[ ]"
        };
        if i == cursor {
            lines.push(format!("{} {} {item}", accent(">"), accent(mark)));
        } else {
            lines.push(format!("  {mark} {item}"));
        }
    }
    lines.join("\n")
}

/// Multi-select on the current screen: arrows move, space toggles, `a`
/// toggles all, enter confirms. Returns the final checked state. Without a
/// TTY the initial state is returned unchanged.
pub fn pick_checkboxes(
    question: &str,
    items: &[String],
    checked: &[bool],
) -> Result<Vec<bool>, String> {
    let state: Vec<bool> = (0..items.len())
        .map(|i| checked.get(i).copied().unwrap_or(false))
        .collect();
    #[cfg(feature = "native")]
    if is_interactive() && !dumb_terminal() && !items.is_empty() {
        return pick_checkboxes_raw(question, items, state);
    }
    Ok(state)
}

#[cfg(feature = "native")]
fn pick_checkboxes_raw(
    question: &str,
    items: &[String],
    mut state: Vec<bool>,
) -> Result<Vec<bool>, String> {
    use std::io::stderr;

    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{self, Clear, ClearType};
    use crossterm::{cursor, execute, queue};

    terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let mut out = stderr();
            let _ = execute!(out, cursor::Show);
            let _ = terminal::disable_raw_mode();
        }
    }
    let _restore = Restore;
    let mut out = stderr();
    let _ = execute!(out, cursor::Hide);

    let mut cursor_i = 0usize;
    let mut last_lines: u16 = 0;
    loop {
        let frame = render_checkboxes(question, items, &state, cursor_i);
        if last_lines > 0 {
            queue!(
                out,
                cursor::MoveToColumn(0),
                cursor::MoveUp(last_lines),
                Clear(ClearType::FromCursorDown),
            )
            .map_err(|e| e.to_string())?;
        }
        last_lines = write_raw_frame(&mut out, &frame)?;

        if let Event::Key(ev) = event::read().map_err(|e| e.to_string())? {
            if ev.kind != KeyEventKind::Press {
                continue;
            }
            match ev.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    cursor_i = cursor_i.checked_sub(1).unwrap_or(items.len() - 1);
                }
                KeyCode::Down | KeyCode::Char('j') => cursor_i = (cursor_i + 1) % items.len(),
                KeyCode::Char(' ') => state[cursor_i] = !state[cursor_i],
                KeyCode::Char('a') => {
                    let all = state.iter().all(|b| *b);
                    state.iter_mut().for_each(|b| *b = !all);
                }
                KeyCode::Enter => return Ok(state),
                KeyCode::Char('c') if ev.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Err("Cancelled.".into());
                }
                KeyCode::Esc | KeyCode::Char('q') => return Err("Cancelled.".into()),
                _ => {}
            }
        }
    }
}

/// Compact HUD prompt on the current screen — no alt-screen.
pub fn pick_numbered(
    title: &str,
    items: &[String],
    current: Option<usize>,
) -> Result<usize, String> {
    const PAGE: usize = 12;
    if items.is_empty() {
        return Err("Nothing to pick.".into());
    }
    let mut page = current
        .map(|i| i / PAGE)
        .unwrap_or(0)
        .min(items.len().saturating_sub(1) / PAGE);

    loop {
        let start = page * PAGE;
        let end = (start + PAGE).min(items.len());
        let pages = items.len().div_ceil(PAGE);
        eprintln!("{}", accent(&format!("▲ {title}")));
        if pages > 1 {
            eprintln!(
                "{}",
                dim(&format!("page {}/{}  ·  n next · p prev", page + 1, pages))
            );
        }
        for (i, item) in items.iter().enumerate().take(end).skip(start) {
            let marker = if current == Some(i) { "◆" } else { " " };
            eprintln!("{marker} {item}");
        }
        let hint = current
            .map(|i| format!("Enter = {} · ", i + 1))
            .unwrap_or_default();
        let ans = prompt(&format!(
            "{} {hint}1-{}  ·  q quit: ",
            accent("❯"),
            items.len()
        ))?;
        let t = ans.trim();
        if t.is_empty() {
            if let Some(i) = current {
                return Ok(i);
            }
            continue;
        }
        if t.eq_ignore_ascii_case("q") || t.eq_ignore_ascii_case("quit") {
            return Err("Cancelled.".into());
        }
        if t.eq_ignore_ascii_case("n") && page + 1 < pages {
            page += 1;
            continue;
        }
        if t.eq_ignore_ascii_case("p") && page > 0 {
            page -= 1;
            continue;
        }
        if let Ok(n) = t.parse::<usize>() {
            if n >= 1 && n <= items.len() {
                return Ok(n - 1);
            }
        }
        eprintln!("{}", dim("  type a number in range, or q"));
    }
}

/// Fallback readline picker when the native TUI feature is off.
#[cfg(not(feature = "native"))]
pub fn pick(title: &str, items: &[String], current: Option<usize>) -> Result<usize, String> {
    const PAGE: usize = 12;
    if items.is_empty() {
        return Err("Nothing to pick.".into());
    }
    let mut page = current
        .map(|i| i / PAGE)
        .unwrap_or(0)
        .min(items.len().saturating_sub(1) / PAGE);

    loop {
        let start = page * PAGE;
        let end = (start + PAGE).min(items.len());
        let pages = items.len().div_ceil(PAGE);
        eprintln!("{}", bold(title));
        if pages > 1 {
            eprintln!(
                "{}",
                dim(&format!(
                    "  page {}/{}  ·  n next · p prev · q cancel",
                    page + 1,
                    pages
                ))
            );
        } else {
            eprintln!("{}", dim("  q cancel"));
        }
        for (i, item) in items.iter().enumerate().take(end).skip(start) {
            let marker = if current == Some(i) { "*" } else { " " };
            eprintln!("  {marker} {:>2}. {item}", i + 1);
        }
        let hint = current
            .map(|i| format!("Enter = {} · ", i + 1))
            .unwrap_or_default();
        let ans = prompt(&format!("{hint}Pick 1-{}: ", items.len()))?;
        let lower = ans.to_ascii_lowercase();
        if ans.is_empty() {
            return current.ok_or_else(|| "Cancelled.".into());
        }
        if lower == "q" || lower == "quit" || lower == "cancel" {
            return Err("Cancelled.".into());
        }
        if pages > 1 && (lower == "n" || lower == "next") {
            page = (page + 1) % pages;
            continue;
        }
        if pages > 1 && (lower == "p" || lower == "prev" || lower == "previous") {
            page = if page == 0 { pages - 1 } else { page - 1 };
            continue;
        }
        if let Ok(n) = ans.parse::<usize>() {
            if n >= 1 && n <= items.len() {
                return Ok(n - 1);
            }
        }
        let hits: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.to_ascii_lowercase() == lower || item.to_ascii_lowercase().contains(&lower)
            })
            .map(|(i, _)| i)
            .collect();
        if hits.len() == 1 {
            return Ok(hits[0]);
        }
        if let Some(i) = items
            .iter()
            .position(|item| item.to_ascii_lowercase() == lower)
        {
            return Ok(i);
        }
        eprintln!("{}", dim(&format!("Not a valid pick: {ans}")));
    }
}

/// Port of the JS fuzzy matcher (contiguous substring + subsequence bonuses).
pub fn fuzzy_score(query: &str, target: &str) -> i32 {
    let q = query.to_ascii_lowercase();
    let t = target.to_ascii_lowercase();
    if q.is_empty() {
        return 0;
    }
    let q_chars: Vec<char> = q.chars().collect();
    let t_chars: Vec<char> = t.chars().collect();
    let mut score = 0i32;
    if let Some(sub_idx) = t.find(&q) {
        score += 50;
        if sub_idx == 0 {
            score += 25;
        } else if is_sep(
            t_chars
                .get(sub_idx.saturating_sub(1))
                .copied()
                .unwrap_or('\0'),
        ) {
            score += 20;
        }
    }
    let mut qi = 0usize;
    let mut run = 0i32;
    for (ti, ch) in t_chars.iter().enumerate() {
        if qi >= q_chars.len() {
            break;
        }
        if *ch == q_chars[qi] {
            if run > 0 {
                score += 8 + run;
            } else {
                score += 1;
            }
            run += 1;
            if ti == 0 {
                score += 30;
            }
            if ti > 0 && is_sep(t_chars[ti - 1]) {
                score += 15;
            }
            qi += 1;
        } else {
            run = 0;
        }
    }
    if qi < q_chars.len() && t.find(&q).is_none() {
        return -1;
    }
    if qi < q_chars.len() {
        return -1;
    }
    score
}

fn is_sep(c: char) -> bool {
    matches!(c, '/' | '-' | '_' | '.' | ':' | ' ' | '\t' | '\n')
}

pub fn rank_ids(query: &str, ids: &[String]) -> Vec<String> {
    if query.trim().is_empty() {
        return ids.to_vec();
    }
    let mut scored: Vec<(i32, usize, String)> = ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let s = fuzzy_score(query, id);
            if s < 0 {
                None
            } else {
                Some((s, i, id.clone()))
            }
        })
        .collect();
    crate::sort::sort_by(&mut scored, |a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, id)| id).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn raw_frames_return_to_column_zero_on_every_line() {
        // WHY: raw mode turns off the terminal's newline translation; a bare
        // `\n` left each menu row starting where the last one ended.
        let out = super::raw_frame_bytes("  1. Switch account\n  2. Log out", Some(80));
        assert_eq!(out, "  1. Switch account\r\n  2. Log out\r\n");
        assert!(!out.replace("\r\n", "").contains('\n'), "{out:?}");
    }

    #[test]
    fn raw_frames_cut_lines_to_the_width_and_keep_color_codes_whole() {
        let line = format!("{}{}", "\x1b[2m", "x".repeat(100));
        let out = super::raw_frame_bytes(&line, Some(40));
        let row = out.trim_end_matches("\r\n");
        assert_eq!(super::text_width(&super::strip_ansi(row)), 39, "{row:?}");
        assert!(
            row.starts_with("\x1b[2m") && row.ends_with(super::RESET),
            "{row:?}"
        );
    }

    #[test]
    fn label_lines_cut_values_to_the_width_with_an_ellipsis() {
        let long = "y".repeat(300);
        let rows = [("scopes", long), ("key", "sk-ar-v1-ab…wxyz".to_string())];
        for width in [40, 80, 200] {
            for line in super::label_lines(&[&rows], Some(width)) {
                if let super::LabelLine::Row { label, value } = line {
                    let w = super::text_width(&label) + 2 + super::text_width(&value);
                    assert!(w <= width, "{width}: {label}{value}");
                }
            }
        }
        let lines = super::label_lines(&[&rows], Some(40));
        assert!(matches!(&lines[0], super::LabelLine::Row { value, .. } if value.ends_with('…')));
    }

    #[test]
    fn label_rows_align_one_column_and_split_groups_with_one_line() {
        let a = [("account", "duyet".to_string()), ("plan", "free".into())];
        let b = [("base url", "https://x".to_string())];
        let out = super::label_rows(&[&a, &[], &b]);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "account   duyet");
        assert_eq!(lines[1], "plan      free");
        assert!(lines[2].chars().all(|c| c == '─'), "{out}");
        assert_eq!(lines[3], "base url  https://x");
        assert_eq!(lines.len(), 4, "empty groups add no separator:\n{out}");
    }

    use super::*;

    #[test]
    fn prompt_reads_one_trimmed_line_from_the_reader() {
        // The non-TTY fallback of prompt_secret is this path. It takes an
        // injected reader so the test cannot hang on inherited stdin.
        let got = prompt_from("x: ", &mut "  sk-ar-v1-abc \nsecond\n".as_bytes()).unwrap();
        assert_eq!(got, "sk-ar-v1-abc");
        assert_eq!(prompt_from("x: ", &mut "".as_bytes()).unwrap(), "");
    }

    #[test]
    fn paint_is_plain_when_no_tty() {
        let out = paint(MAGENTA, "hello");
        assert_eq!(out, "hello");
        assert!(!out.contains('\u{1b}'));
    }

    #[test]
    fn fuzzy_contiguous_outranks_scattered() {
        let glm = fuzzy_score("glm", "z-ai/glm-4.7");
        let gem = fuzzy_score("glm", "google/gemini-2.5");
        assert!(glm > gem, "glm={glm} gem={gem}");
        assert!(glm >= 0);
    }

    #[test]
    fn fuzzy_miss_is_negative() {
        assert_eq!(fuzzy_score("zzzz", "anthropic/claude-sonnet-4.6"), -1);
    }

    #[test]
    fn rank_ids_keeps_order_on_empty_query() {
        let ids = vec!["b".into(), "a".into()];
        assert_eq!(rank_ids("", &ids), ids);
    }

    #[test]
    fn inline_menu_highlights_cursor_and_numbers_rows() {
        let frame = render_inline_menu(
            &["anyr  ● duyet  ·  claude".into()],
            "What do you want to do?",
            &["Launch claude".into(), "Config".into(), "Quit".into()],
            0,
            "# no fullscreen",
        );
        assert!(frame.contains("1."), "{frame}");
        assert!(frame.contains("Launch claude"), "{frame}");
        assert!(frame.contains("What do you want to do?"), "{frame}");
        assert!(frame.contains("# no fullscreen"), "{frame}");
        assert!(!frame.contains("type a number"), "{frame}");
    }

    #[test]
    fn brand_chip_is_one_line_mark_plus_caption() {
        let out = brand_chip(&["AnyRouter CLI v0.1.x", "One key."]);
        assert!(out.contains("▲ anyr"), "{out}");
        assert!(out.contains("▀█████████▄"), "{out}");
        assert!(out.contains("AnyRouter CLI v0.1.x"), "{out}");
        assert!(out.contains("One key."), "{out}");
        assert!(out.lines().count() >= 2, "{out}");
        assert!(
            !out.contains(MARK_LINES[1].trim()),
            "chip must not print the 5-row hero:\n{out}"
        );
    }

    #[test]
    fn brand_header_places_captions_beside_the_mark() {
        let out = brand_header(&["AnyRouter", "account  default", ""]);
        for line in MARK_LINES {
            assert!(out.contains(line), "missing {line:?} in:\n{out}");
        }
        assert!(
            out.contains("▀█████████▄"),
            "must be the official AR half-block mark, not dithered dots:\n{out}"
        );
        assert!(out.contains("AnyRouter"), "{out}");
        assert!(out.contains("account  default"), "{out}");
        assert_eq!(out.lines().count(), MARK_LINES.len());
    }

    #[test]
    fn mark_png_is_the_official_logo_not_the_32px_icon() {
        assert!(
            MARK_PNG.starts_with(b"\x89PNG"),
            "assets/mark.png must be PNG"
        );
        assert!(
            MARK_PNG.len() > 4000,
            "hi-res official AR is larger than the 96px crop, got {}",
            MARK_PNG.len()
        );
        let b64 = base64_encode(MARK_PNG);
        assert!(b64.starts_with("iVBOR"));
        assert!(!b64.contains(' '));
    }

    #[test]
    fn mark_sixel_is_a_sixel_dcs() {
        let s = include_str!("../assets/mark.sixel");
        assert!(
            s.starts_with("\x1bPq"),
            "assets/mark.sixel must start with DCS Pq, got {:?}",
            s.chars().take(8).collect::<String>()
        );
        assert!(s.contains("\x1b\\"), "sixel must end with ST");
    }

    fn probe(pairs: &[(&str, &str)]) -> Option<Graphics> {
        graphics_kind_from(|k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_string())
        })
    }

    #[test]
    fn graphics_kitty_family() {
        assert_eq!(probe(&[("KITTY_WINDOW_ID", "1")]), Some(Graphics::Kitty));
        assert_eq!(probe(&[("TERM", "xterm-kitty")]), Some(Graphics::Kitty));
        assert_eq!(
            probe(&[("GHOSTTY_RESOURCES_DIR", "/opt/ghostty")]),
            Some(Graphics::Kitty)
        );
        assert_eq!(
            probe(&[("TERM", "xterm-ghostty"), ("TERM_PROGRAM", "ghostty")]),
            Some(Graphics::Kitty)
        );
    }

    #[test]
    fn graphics_iterm_family() {
        assert_eq!(
            probe(&[("ITERM_SESSION_ID", "w0t0p0")]),
            Some(Graphics::Iterm)
        );
        assert_eq!(
            probe(&[("LC_TERMINAL", "iTerm2"), ("TERM", "xterm-256color")]),
            Some(Graphics::Iterm)
        );
        assert_eq!(probe(&[("TERM_PROGRAM", "WezTerm")]), Some(Graphics::Iterm));
        assert_eq!(
            probe(&[("WEZTERM_EXECUTABLE", "/usr/bin/wezterm")]),
            Some(Graphics::Iterm)
        );
        assert_eq!(
            probe(&[("TERM_PROGRAM", "WarpTerminal")]),
            Some(Graphics::Iterm)
        );
        assert_eq!(probe(&[("TERM_PROGRAM", "mintty")]), Some(Graphics::Iterm));
        assert_eq!(probe(&[("TERM_PROGRAM", "rio")]), Some(Graphics::Iterm));
        assert_eq!(probe(&[("TERM_PROGRAM", "Tabby")]), Some(Graphics::Iterm));
        assert_eq!(probe(&[("TERM_PROGRAM", "Blink")]), Some(Graphics::Iterm));
    }

    #[test]
    fn graphics_sixel_family() {
        assert_eq!(probe(&[("WT_SESSION", "abc")]), Some(Graphics::Sixel));
        assert_eq!(
            probe(&[("KONSOLE_VERSION", "240400")]),
            Some(Graphics::Sixel)
        );
        assert_eq!(probe(&[("TERM", "foot")]), Some(Graphics::Sixel));
        assert_eq!(probe(&[("TERM", "mlterm")]), Some(Graphics::Sixel));
        assert_eq!(probe(&[("TERM", "contour")]), Some(Graphics::Sixel));
        assert_eq!(
            probe(&[("ALACRITTY_WINDOW_ID", "1")]),
            Some(Graphics::Sixel)
        );
        assert_eq!(probe(&[("TERM", "eat-truecolor")]), Some(Graphics::Sixel));
        assert_eq!(probe(&[("TERM", "yaft")]), Some(Graphics::Sixel));
    }

    #[test]
    fn graphics_skips_hosts_that_would_print_garbage() {
        assert_eq!(
            probe(&[("TERM", "xterm-256color"), ("TERM_PROGRAM", "vscode")]),
            None
        );
        assert_eq!(
            probe(&[
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "Apple_Terminal")
            ]),
            None
        );
        assert_eq!(
            probe(&[("TERM", "xterm-256color"), ("VTE_VERSION", "7600")]),
            None
        );
        assert_eq!(probe(&[("TERM", "xterm-256color")]), None);
        assert_eq!(probe(&[("TERM", "linux")]), None);
        assert_eq!(probe(&[("TERM", "dumb")]), None);
        assert_eq!(probe(&[("ZELLIJ", "0"), ("KITTY_WINDOW_ID", "1")]), None);
        assert_eq!(
            probe(&[("STY", "123.pts"), ("TERM", "screen-256color")]),
            None
        );
        assert_eq!(probe(&[("NVIM", "1"), ("TERM", "xterm-kitty")]), None);
        assert_eq!(
            probe(&[
                ("TERMINAL_EMULATOR", "JetBrains-JediTerm"),
                ("TERM", "xterm-256color")
            ]),
            None
        );
        assert_eq!(probe(&[("MSYSTEM", "MINGW64"), ("TERM", "xterm")]), None);
    }

    #[test]
    fn graphics_tmux_still_detects_outer_terminal() {
        assert_eq!(
            probe(&[
                ("TMUX", "/tmp/tmux-1/default,1,0"),
                ("KITTY_WINDOW_ID", "1")
            ]),
            Some(Graphics::Kitty)
        );
        assert_eq!(
            probe(&[("TMUX", "1"), ("ITERM_SESSION_ID", "w0t0p0")]),
            Some(Graphics::Iterm)
        );
        assert_eq!(
            probe(&[("TMUX", "1"), ("WT_SESSION", "abc")]),
            Some(Graphics::Sixel)
        );
    }

    #[test]
    fn graphics_override_env() {
        assert_eq!(
            probe(&[("ANYR_GRAPHICS", "off"), ("KITTY_WINDOW_ID", "1")]),
            None
        );
        assert_eq!(
            probe(&[("ANYR_GRAPHICS", "sixel"), ("TERM_PROGRAM", "vscode")]),
            Some(Graphics::Sixel)
        );
        assert_eq!(
            probe(&[("ANYR_GRAPHICS", "kitty"), ("TERM", "dumb")]),
            Some(Graphics::Kitty)
        );
        assert_eq!(probe(&[("ANYR_GRAPHICS", "iterm")]), Some(Graphics::Iterm));
    }

    #[test]
    fn tmux_passthrough_doubles_esc() {
        let wrapped = tmux_passthrough("\x1b_Gok\x1b\\");
        assert!(wrapped.starts_with("\x1bPtmux;"), "{wrapped:?}");
        assert!(wrapped.contains("\x1b\x1b_Gok\x1b\x1b\\"), "{wrapped:?}");
        assert!(wrapped.ends_with("\x1b\\"), "{wrapped:?}");
    }
}
