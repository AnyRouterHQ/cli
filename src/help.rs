use std::cell::RefCell;
use std::path::Path;

use crate::VERSION;

thread_local! {
    static INVOKED_BIN: RefCell<String> = const { RefCell::new(String::new()) };
    static INVOKED_ARGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The words after the binary name, for `rerun_with_yes`.
pub fn set_invoked_args(args: &[String]) {
    INVOKED_ARGS.with(|slot| *slot.borrow_mut() = args.to_vec());
}

/// This run's command line with `--yes` added (before a `--`), quoted for a
/// POSIX shell.
/// A secret on the command line (`--key <v>`, `--key=<v>`, an
/// `Authorization` header) becomes `"$ANYROUTER_API_KEY"`, so the hint stays
/// runnable and never shows the secret.
pub fn rerun_with_yes() -> String {
    const KEY_REF: &str = "\"$ANYROUTER_API_KEY\"";
    let args = INVOKED_ARGS.with(|slot| slot.borrow().clone());
    let sep = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let needs_yes = !args[..sep].iter().any(|w| w == "--yes");
    let mut words: Vec<String> = Vec::new();
    let mut secret_next = None;
    for (i, a) in args.iter().enumerate() {
        if i == sep && needs_yes {
            words.push("--yes".into());
        }
        if let Some(replacement) = secret_next.take().filter(|_| a != "-") {
            words.push(replacement);
            continue;
        }
        if i < sep {
            if a.starts_with("--key=") && a != "--key=-" {
                words.push(format!("--key={KEY_REF}"));
                continue;
            }
            if a == "--key" {
                secret_next = Some(KEY_REF.to_string());
            } else if a == "-H"
                && args
                    .get(i + 1)
                    .is_some_and(|h| h.to_ascii_lowercase().starts_with("authorization"))
            {
                secret_next = Some("\"Authorization: Bearer $ANYROUTER_API_KEY\"".into());
            }
        }
        words.push(shell_quote(a));
    }
    if sep == args.len() && needs_yes {
        words.push("--yes".into());
    }
    let mut out = invoked_bin();
    for w in words {
        out.push(' ');
        out.push_str(&w);
    }
    out
}

fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && !word.starts_with('=')
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:=@,+%".contains(&b));
    if plain {
        return word.to_string();
    }
    if word.chars().any(char::is_control) {
        // `$'…'` keeps a newline inside one line, so the hint stays one line.
        let mut out = String::from("$'");
        for c in word.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '\'' => out.push_str("\\'"),
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('\'');
        return out;
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// Map argv0 / npm shim name to the command users should type in help.
pub fn display_bin(argv0: &str) -> String {
    let name = Path::new(argv0)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(argv0);
    let name = name.strip_suffix(".exe").unwrap_or(name);
    match name {
        "ar" => "ar".into(),
        "anyrouter" => "anyrouter".into(),
        "anyr" => "anyr".into(),
        "cli" | "npx-anyr" | "npx-anyr.js" => "npx @anyr/cli".into(),
        other if other.starts_with("npx @anyr/cli") => "npx @anyr/cli".into(),
        other if other.starts_with("anyr-") => "anyr".into(),
        _ => "anyr".into(),
    }
}

pub fn set_invoked_bin(name: impl Into<String>) {
    let name = name.into();
    INVOKED_BIN.with(|slot| *slot.borrow_mut() = name);
}

/// `template` with `{bin}` replaced by the name this binary was run as.
pub fn with_bin(template: &str) -> String {
    template.replace("{bin}", &invoked_bin())
}

pub fn invoked_bin() -> String {
    INVOKED_BIN.with(|slot| {
        let current = slot.borrow();
        if current.is_empty() {
            "anyr".into()
        } else {
            current.clone()
        }
    })
}

/// Resolve the help command from env (`ANYR_DISPLAY_BIN`) or argv0.
pub fn resolve_bin(argv0: Option<&str>, display_env: Option<&str>) -> String {
    if let Some(v) = display_env.map(str::trim).filter(|s| !s.is_empty()) {
        return display_bin(v);
    }
    display_bin(argv0.unwrap_or("anyr"))
}

/// Short `--help`: the everyday commands and three examples. The full map is
/// `help --all`.
pub fn root_help() -> String {
    let bin = invoked_bin();
    format!(
        "\
{} – point any coding agent at AnyRouter

Everyday
  {bin} login                     Sign in (browser; --device on a server)
  {bin} chat \"<prompt>\"           Ask a model; --json for scripts
  {bin} models                    Browse models; `models use <id>` sets the default
  {bin} claude                    Launch Claude Code (also codex, grok, opencode, pi, pool)
  {bin} relay                     Serve this machine's local models
  {bin} artifact publish <file>   Publish a page or folder to a URL
  {bin} usage                     Credits remaining

Examples
  $ {bin} claude --model anyrouter/free[1m]
  $ git diff | {bin} chat \"Review this\" -
  $ {bin} models --free

More
  {bin} help --all        every command, grouped
  {bin} help agents       for scripts and agents: --json, exit codes
  {bin} <command> --help
",
        crate::term::accent(&bin),
        bin = bin
    )
}

/// Full command map (`help --all`, `help commands`, `commands`), grouped,
/// generated from `crate::registry` so it lists exactly what dispatch runs.
/// Headings are ALL CAPS and entries `  name: summary` — the cli-regression
/// skill parses this layout.
pub fn commands_help() -> String {
    use crate::registry;
    let bin = invoked_bin();
    let header = crate::term::brand_chip(&[
        &format!(
            "{}  {}",
            crate::term::bold(&format!("AnyRouter CLI v{VERSION}")),
            crate::term::link("https://anyrouter.dev")
        ),
        &crate::term::dim("One key. Every coding agent."),
    ]);
    let names = registry::canonical_names();
    let mut out = format!(
        "{header}\nUSAGE\n  {bin} <command> [subcommand] [flags]\n  {bin} <command> --help\n  {bin} commands --json      this map as JSON\n"
    );
    for (id, title) in registry::GROUPS {
        let rows: Vec<(String, String)> = names
            .iter()
            .filter(|n| registry::group(n) == *id && !registry::is_shortcut(n))
            .flat_map(|n| {
                let aliases = registry::aliases(n);
                let mut summary = crate::registry::summary(n).to_string();
                let subs = registry::subcommands(n);
                if !subs.is_empty() && *id != "launch" {
                    summary = format!("{summary} [{}]", subs.join(" "));
                }
                let mut rows = vec![(n.to_string(), summary)];
                if *id == "launch" {
                    rows.extend(
                        aliases
                            .iter()
                            .map(|a| (a.to_string(), format!("Alias of {n}"))),
                    );
                } else if !aliases.is_empty() {
                    rows[0].1 = format!("{} (alias: {})", rows[0].1, aliases.join(", "));
                }
                rows
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{}\n", title.to_uppercase()));
        for (name, summary) in rows {
            out.push_str(&format!("  {:<14}{summary}\n", format!("{name}:")));
        }
    }
    out.push_str(&format!(
        "\nMORE\n  {bin} help agents      using {bin} from scripts and coding agents\n  {bin}                  on a terminal: confirm account / model / agent, then launch\n"
    ));
    out
}

/// Lines tagged `@claude ` in the body only apply to Claude Code; other agents
/// do not get them.
fn launch_help(bin: &str, id: &str, label: &str) -> String {
    let body: String = doc("launch_help_body")
        .lines()
        .filter_map(|line| match line.strip_prefix("@claude ") {
            Some(rest) => (id == "claude").then_some(rest),
            None => Some(line),
        })
        .map(|line| format!("{line}\n"))
        .collect();
    let example = match id {
        "pi" => "--tools read,grep",
        "codex" => "--search",
        "grok" => "--no-plan",
        "opencode" | "kilo" => "--print-logs",
        "pool" => "--worktree",
        "aider" => "--architect",
        "goose" => "session --name work",
        "qwen" | "crush" => "--debug",
        _ => "--verbose",
    };
    let body = body
        .replace("{portable}", &crate::cmd::portable::help_lines(id, label))
        .replace("{bin}", bin)
        .replace("{id}", id)
        .replace("{example}", example);
    format!(
        "{bin} {id} — launch {label} through AnyRouter\n\nUsage:\n  {bin} {id} [options] [{id} flags and args] [-- <raw {id} args>]\n\n{body}"
    )
}

fn fill(bin: &str, template: &str) -> String {
    template.replace("{bin}", bin)
}

/// A long text page from `assets/help/<name>.txt` (help pages, onboard
/// prompts) or the agents guide. `build.rs` packs them deflated
/// (`name\0text\0` pairs), which keeps about 33 KB out of the binary;
/// inflating costs well under a millisecond and only the command that prints
/// the page pays it.
pub(crate) fn doc(name: &str) -> String {
    static PACKED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/help.bin"));
    let raw = miniz_oxide::inflate::decompress_to_vec(PACKED).unwrap_or_default();
    let raw = String::from_utf8(raw).unwrap_or_default();
    let mut parts = raw.split('\0');
    while let (Some(key), Some(text)) = (parts.next(), parts.next()) {
        if key == name {
            return text.to_string();
        }
    }
    String::new()
}

fn page(bin: &str, name: &str) -> String {
    fill(bin, &doc(name))
}

pub fn command_help(command: &str) -> Option<String> {
    let bin = invoked_bin();
    let canonical = match command {
        "cc" => "claude",
        "poolside" => "pool",
        "status" => "whoami",
        "update" => "upgrade",
        "setup" => "login",
        "implement" => "impl",
        "decisions" | "systemone" => "decision",
        "gen" => "generation",
        other => other,
    };
    Some(match canonical {
        "auth" => page(&bin, "auth"),
        "login" => page(&bin, "login"),
        "token" => page(&bin, "token"),
        "switch" => page(&bin, "switch"),
        "usage" => page(&bin, "usage"),
        "decision" => page(&bin, "decision"),
        "whoami" => page(&bin, "whoami"),
        "account" => page(&bin, "account"),
        "logs" => page(&bin, "logs"),
        "generation" => page(&bin, "generation"),
        "presets" => page(&bin, "presets"),
        "mcp" => page(&bin, "mcp"),
        "models" => page(&bin, "models"),
        "config" => page(&bin, "config"),
        "chat" => page(&bin, "chat"),
        "skills" => fill(
            &bin,
            "{bin} skills — not yet in the native CLI (coming later).\n",
        ),
        "relay" => page(&bin, "relay"),
        "byok" => page(&bin, "byok"),
        "artifact" => page(&bin, "artifact"),
        "task" => page(&bin, "task"),
        "delegate" => page(&bin, "delegate"),
        "keys" => page(&bin, "keys"),
        "audit" => fill(
            &bin,
            "{bin} audit — not yet in the native CLI (coming later).\n",
        ),
        "logout" => page(&bin, "logout"),
        "upgrade" => page(&bin, "upgrade"),
        "transactions" => page(&bin, "transactions"),
        "embed" => page(&bin, "embed"),
        "eval" => page(&bin, "eval"),
        "menu" => fill(
            &bin,
            "{bin} menu — fullscreen launcher (default on a TTY)\n\nUsage:\n  {bin}                 Same as `{bin} menu` on a TTY\n  {bin} menu [--dump-tui]\n\nAccount, model, credits in a header. Launch agents, configure, quit.\n↑↓ / j k move, ↵ select (signs in and installs if needed), q/esc quit.\nSet ANYR_TUI=0 for inline HUD.\n\n`--dump-tui` / ANYR_TUI_DUMP=1 prints one plain frame and exits.\n",
        ),
        "commands" => commands_help(),
        "schema" => page(&bin, "schema"),
        "telemetry" => page(&bin, "telemetry"),
        "report" => page(&bin, "report"),
        "api" => crate::api::help(),
        "publish" => crate::publish::publish_help(),
        "apps" => crate::publish::apps_help(),
        "completion" => page(&bin, "completion"),
        "prompt" => fill(
            &bin,
            "{bin} prompt — hub prompts not yet in the native CLI. Use {bin} onboard for agent paste prompts.\n",
        ),
        "onboard" | "impl" | "plan" | "fix" | "deploy" | "cp" => {
            crate::onboard::usage_hint(&bin)
        }
        "claude" => launch_help(&bin, "claude", "Claude Code"),
        "codex" => launch_help(&bin, "codex", "Codex"),
        "grok" => launch_help(&bin, "grok", "Grok Build"),
        "opencode" => launch_help(&bin, "opencode", "opencode"),
        "pi" => launch_help(&bin, "pi", "Pi"),
        "qwen" => launch_help(&bin, "qwen", "Qwen Code"),
        "goose" => launch_help(&bin, "goose", "Goose"),
        "aider" => launch_help(&bin, "aider", "Aider"),
        "crush" => launch_help(&bin, "crush", "Crush"),
        "kilo" => launch_help(&bin, "kilo", "Kilo Code"),
        "fx" => launch_help(&bin, "fx", "fx"),
        "pool" => format!(
            "{}\n{}",
            page(&bin, "pool"),
            launch_help(&bin, "pool", "Poolside")
        ),
        "cursor" | "cline" | "windsurf" => format!(
            "{bin} {canonical} — not a launch target yet.\n\
Print a key with `{bin} auth token` and the base URL with `{bin} onboard impl`.\n"
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_bin_from_symlink_and_npx() {
        assert_eq!(display_bin("/home/duyet/.local/bin/ar"), "ar");
        assert_eq!(display_bin("anyrouter"), "anyrouter");
        assert_eq!(display_bin("anyr"), "anyr");
        assert_eq!(display_bin("anyr.exe"), "anyr");
        assert_eq!(display_bin("cli"), "npx @anyr/cli");
        assert_eq!(display_bin("npx-anyr.js"), "npx @anyr/cli");
        assert_eq!(display_bin("anyr-linux-x86_64"), "anyr");
        assert_eq!(display_bin("npx @anyr/cli"), "npx @anyr/cli");
    }

    #[test]
    fn root_help_uses_invoked_name() {
        set_invoked_bin("ar");
        let out = root_help();
        assert!(out.contains("ar claude"), "{out}");
        assert!(out.contains("ar login"), "{out}");
        assert!(out.contains("point any coding agent"), "{out}");
        assert!(out.contains("ar help --all"), "{out}");
        assert!(out.contains("ar help agents"), "{out}");
        assert!(!out.contains("npx @anyr/cli"), "{out}");
        assert!(!out.contains("Install:"), "{out}");
        assert!(!out.contains("anyrouter.dev/docs/cli"), "{out}");
        assert!(
            !out.contains("CORE COMMANDS"),
            "examples-first help must not dump the catalog:\n{out}"
        );

        let map = commands_help();
        assert!(map.contains("\nLAUNCH\n"), "{map}");
        assert!(map.contains("▀█████████▄"), "{map}");
        assert!(map.contains("ar help agents"), "{map}");

        set_invoked_bin("npx @anyr/cli");
        let npx = root_help();
        assert!(npx.contains("npx @anyr/cli claude"), "{npx}");
        assert!(npx.contains("npx @anyr/cli login"), "{npx}");
        set_invoked_bin("anyr");
    }

    #[test]
    fn short_help_shows_each_everyday_command_and_stays_short() {
        set_invoked_bin("anyr");
        let out = root_help();
        for c in crate::registry::EVERYDAY {
            assert!(out.contains(&format!("anyr {c}")), "{c} missing:\n{out}");
        }
        // WHY: the first screen is for the common path; the rest is one
        // command away (`help --all`).
        assert!(
            out.lines().count() <= 25,
            "{} lines:\n{out}",
            out.lines().count()
        );
    }

    #[test]
    fn full_map_lists_every_command_under_an_all_caps_heading() {
        set_invoked_bin("anyr");
        let map = commands_help();
        for name in crate::registry::canonical_names() {
            if crate::registry::is_shortcut(name) {
                continue; // listed under `onboard`
            }
            assert!(
                map.contains(&format!("\n  {name}:")),
                "{name} missing:\n{map}"
            );
        }
        for alias in ["cc", "poolside"] {
            assert!(map.contains(&format!("  {alias}:")), "{alias}:\n{map}");
        }
        for (_, title) in crate::registry::GROUPS {
            let heading = title.to_uppercase();
            assert!(
                heading.chars().all(|c| c.is_ascii_uppercase() || c == ' '),
                "cli-regression parses ^[A-Z ]+$ headings: {heading}"
            );
        }
    }

    #[test]
    fn rerun_adds_yes_before_the_separator_and_quotes_words() {
        // WHY: the exit-6 hint must run as is; `--yes` after `--` would go to
        // the agent, and an unquoted space would split an argument.
        super::set_invoked_args(&[
            "claude".into(),
            "-p".into(),
            "fix it".into(),
            "--".into(),
            "x".into(),
        ]);
        assert!(super::rerun_with_yes().ends_with("claude -p 'fix it' --yes -- x"));
        super::set_invoked_args(&["artifact".into(), "rm".into(), "it's".into()]);
        assert!(super::rerun_with_yes().ends_with("artifact rm 'it'\\''s' --yes"));
        super::set_invoked_args(&[
            "keys".into(),
            "revoke".into(),
            "ab12".into(),
            "--yes".into(),
        ]);
        assert!(super::rerun_with_yes().ends_with("keys revoke ab12 --yes"));
        // `--yes` after `--` belongs to the agent; anyr still needs its own.
        super::set_invoked_args(&["claude", "-p", "a\nb", "--", "--yes"].map(String::from));
        let r = super::rerun_with_yes();
        assert!(r.ends_with("claude -p $'a\\nb' --yes -- --yes"), "{r}");
        super::set_invoked_args(&["x", "=ls"].map(String::from));
        assert!(super::rerun_with_yes().ends_with("x '=ls' --yes"));
        super::set_invoked_args(&[]);
    }

    #[test]
    fn rerun_never_shows_a_key_and_stays_runnable() {
        // WHY: the hint lands in agent transcripts; a masked key would fail
        // the rerun and a plain one would leak.
        let words = [
            "api",
            "GET",
            "/x",
            "--key",
            "sk-secret-123",
            "-H",
            "Authorization: Bearer abc",
        ];
        super::set_invoked_args(&words.map(String::from));
        let r = super::rerun_with_yes();
        assert!(!r.contains("sk-secret") && !r.contains("abc"), "{r}");
        assert!(r.contains("--key \"$ANYROUTER_API_KEY\""), "{r}");
        super::set_invoked_args(&["x", "--key=sk-secret-123"].map(String::from));
        assert!(!super::rerun_with_yes().contains("sk-secret"));
        super::set_invoked_args(&["x", "--key", "-"].map(String::from));
        assert!(super::rerun_with_yes().ends_with("x --key - --yes"));
        super::set_invoked_args(&[]);
    }

    #[test]
    fn command_help_uses_invoked_name() {
        set_invoked_bin("ar");
        let login = command_help("login").unwrap();
        assert!(login.contains("ar auth login"), "{login}");
        assert!(login.contains("ar claude"), "{login}");
        assert!(
            !login.to_ascii_lowercase().contains("wizard"),
            "login must not mention a post-login wizard:\n{login}"
        );
        assert!(!login.contains("npx @anyr/cli"), "{login}");
        let auth = command_help("auth").unwrap();
        assert!(auth.contains("ar auth <command>"), "{auth}");
        let claude = command_help("claude").unwrap();
        assert!(claude.contains("ar claude"), "{claude}");
        assert!(!claude.contains("--no-check"), "{claude}");
        assert!(!claude.contains("opens the launcher"), "{claude}");
        let cursor = command_help("cursor").unwrap();
        assert!(
            cursor.contains("not a launch target") || cursor.contains("auth token"),
            "{cursor}"
        );
        set_invoked_bin("anyr");
    }

    #[test]
    fn resolve_bin_prefers_env() {
        assert_eq!(resolve_bin(Some("/usr/bin/anyr"), Some("ar")), "ar");
        assert_eq!(resolve_bin(Some("/usr/bin/anyr"), None), "anyr");
    }

    /// The packed pages must come back byte for byte; a page lost in packing
    /// would print an empty `--help`.
    #[test]
    fn packed_pages_round_trip_exactly() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/help");
        let mut pages = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_stem().unwrap().to_str().unwrap().to_string();
            let want = std::fs::read_to_string(&path)
                .unwrap()
                .replace("\r\n", "\n");
            assert!(!want.is_empty(), "{name}");
            assert_eq!(doc(&name), want, "{name}");
            pages += 1;
        }
        assert!(pages >= 32, "only {pages} pages");
        let agents = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/agents.md");
        assert_eq!(doc("agents"), std::fs::read_to_string(agents).unwrap());
        assert_eq!(doc("no-such-page"), "");
    }
}
