//! Portable launcher flags: one spelling for an idea most coding agents
//! share, mapped to each agent's own argv. Launch argv, `anyr <agent> --help`
//! and the README table all read `ALIASES`.

use crate::cmd::dispatch::launch_flags;

/// Launchers, in README column order.
#[cfg(test)]
pub(crate) const AGENTS: &[&str] = crate::registry::LAUNCHERS;

/// What each `Check::Help` / `Check::Probe` cell was checked against.
/// Poolside is not installed where this was written; its cells cite
/// docs.poolside.ai/cli/cli-reference (v1.0.16).
#[cfg(test)]
pub(crate) const CHECKED: [(&str, &str); 12] = [
    ("claude", "2.1.310"),
    ("codex", "0.133.0"),
    ("grok", "1.0.52"),
    ("opencode", "2.0.24"),
    ("pi", "0.73.1"),
    ("pool", "docs v1.0.16"),
    ("qwen", "0.26.1"),
    ("goose", "1.56.0"),
    ("aider", "0.86.2"),
    ("crush", "0.98.2"),
    ("kilo", "7.8.17"),
    ("fx", "0.0.14"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Check {
    /// Listed in the agent's `--help`.
    Help,
    /// Hidden from `--help`; the agent's parser says "argument missing".
    Probe,
    /// From the vendor docs only.
    Docs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Takes {
    Nothing,
    /// Placeholder name for help.
    Value(&'static str),
    /// Value taken only when the next token is not a flag.
    Optional(&'static str),
}

/// An agent's own argv for an alias. `{}` in `argv` is the alias value.
#[derive(Debug)]
pub(crate) struct Argv {
    pub(crate) argv: &'static [&'static str],
    /// For `Takes::Optional` given no value. None: the agent needs one.
    pub(crate) bare: Option<&'static [&'static str]>,
    /// Subcommand the agent needs; inserted once, `argv` right after it.
    pub(crate) sub: Option<&'static str>,
    /// When the user runs this subcommand, `argv` must follow it.
    pub(crate) after: Option<&'static str>,
    /// Global flag: placed right before this subcommand, not after it.
    pub(crate) before: Option<&'static str>,
    /// With a value-less alias under this subcommand: use these tokens
    /// instead (empty: drop, and say so when `note` is set).
    pub(crate) under: Option<Under>,
    /// The agent's own spellings: any of them already present wins.
    pub(crate) same: &'static [&'static str],
    /// Values the agent accepts; others clamp to the nearest lower one.
    pub(crate) levels: Option<&'static [&'static str]>,
    pub(crate) check: Check,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Under {
    pub(crate) sub: &'static str,
    pub(crate) tokens: &'static [&'static str],
    /// Completes "<agent> <sub> has ...": the reason for dropping.
    pub(crate) note: Option<&'static str>,
}

#[derive(Debug)]
pub(crate) enum Native {
    Argv(Argv),
    /// anyr sets this env var on the child instead of an argv flag.
    Env(&'static str),
    /// anyr sets `NAME=value` on the child; the agent has no flag for it.
    EnvSet(&'static str, &'static str),
    /// Completes "<agent> has ...": dropped with a one-line note.
    Unsupported(&'static str),
}

const fn argv(argv: &'static [&'static str], same: &'static [&'static str]) -> Argv {
    Argv {
        argv,
        bare: None,
        sub: None,
        after: None,
        before: None,
        under: None,
        same,
        levels: None,
        check: Check::Help,
    }
}

impl Argv {
    const fn bare(self, bare: &'static [&'static str]) -> Self {
        Self {
            bare: Some(bare),
            ..self
        }
    }
    const fn sub(self, sub: &'static str) -> Self {
        Self {
            sub: Some(sub),
            ..self
        }
    }
    const fn after(self, after: &'static str) -> Self {
        Self {
            after: Some(after),
            ..self
        }
    }
    const fn before(self, before: &'static str) -> Self {
        Self {
            before: Some(before),
            ..self
        }
    }
    const fn under(
        self,
        sub: &'static str,
        tokens: &'static [&'static str],
        note: Option<&'static str>,
    ) -> Self {
        Self {
            under: Some(Under { sub, tokens, note }),
            ..self
        }
    }
    const fn levels(self, levels: &'static [&'static str]) -> Self {
        Self {
            levels: Some(levels),
            ..self
        }
    }
    const fn check(self, check: Check) -> Self {
        Self { check, ..self }
    }
    const fn native(self) -> Native {
        Native::Argv(self)
    }
}

pub(crate) struct Alias {
    pub(crate) name: &'static str,
    /// Other spellings of the same flag.
    pub(crate) also: &'static [&'static str],
    pub(crate) takes: Takes,
    /// Every use is forwarded, even next to the agent's own spelling.
    pub(crate) repeatable: bool,
    pub(crate) about: &'static str,
    pub(crate) agents: [(&'static str, Native); 12],
}

impl Alias {
    pub(crate) fn native(&self, agent: &str) -> Option<&Native> {
        self.agents
            .iter()
            .find(|(a, _)| *a == agent)
            .map(|(_, n)| n)
    }
}

const CODEX_LEVELS: &[&str] = &["minimal", "low", "medium", "high"];
const PI_LEVELS: &[&str] = &["minimal", "low", "medium", "high", "xhigh"];
/// From grok 1.0.52's own error for `--reasoning-effort bogus`.
const GROK_LEVELS: &[&str] = &["low", "medium", "high", "xhigh"];

/// Registry order is also subcommand order: codex `--print --continue` is
/// `exec resume --last`.
pub(crate) const ALIASES: &[Alias] = &[
    Alias {
        name: "yolo",
        also: &[],
        takes: Takes::Nothing,
        repeatable: false,
        about: "Skip permission prompts",
        agents: [
            (
                "claude",
                argv(
                    &["--dangerously-skip-permissions"],
                    &["--dangerously-skip-permissions"],
                )
                .native(),
            ),
            (
                "codex",
                argv(
                    &["--dangerously-bypass-approvals-and-sandbox"],
                    &["--dangerously-bypass-approvals-and-sandbox"],
                )
                .native(),
            ),
            (
                "grok",
                argv(&["--always-approve"], &["--always-approve"]).native(),
            ),
            // `--auto` before `run` did not reach the run (#181).
            (
                "opencode",
                argv(&["--auto"], &["--auto"]).after("run").native(),
            ),
            (
                "pi",
                Native::Unsupported("no flag to skip permission prompts"),
            ),
            (
                "pool",
                argv(&["--unsafe-auto-allow"], &["--unsafe-auto-allow"])
                    .check(Check::Docs)
                    .native(),
            ),
            (
                "qwen",
                argv(&["--approval-mode", "yolo"], &["--approval-mode"]).native(),
            ),
            // Goose has no flag; its mode comes from the environment.
            ("goose", Native::EnvSet("GOOSE_MODE", "auto")),
            ("aider", argv(&["--yes-always"], &["--yes-always"]).native()),
            // `crush run` rejects `-y`; it is a root flag only.
            (
                "crush",
                argv(&["-y"], &["-y"])
                    .under("run", &[], Some("no flag to skip permission prompts"))
                    .native(),
            ),
            ("kilo", argv(&["--auto"], &["--auto"]).after("run").native()),
            // `--yolo` is a flag of `ask` only; fx reads its mode from the environment.
            ("fx", Native::EnvSet("FX_PERMISSION_MODE", "full-access")),
        ],
    },
    Alias {
        name: "effort",
        also: &[],
        takes: Takes::Value("level"),
        repeatable: false,
        about: "Reasoning effort: minimal | low | medium | high | xhigh | max",
        agents: [
            ("claude", Native::Env("MAX_THINKING_TOKENS")),
            (
                "codex",
                argv(
                    &["-c", "model_reasoning_effort=\"{}\""],
                    &["model_reasoning_effort"],
                )
                .levels(CODEX_LEVELS)
                .native(),
            ),
            (
                "grok",
                argv(&["--reasoning-effort", "{}"], &["--reasoning-effort"])
                    .levels(GROK_LEVELS)
                    .native(),
            ),
            ("opencode", Native::Unsupported("no reasoning-effort flag")),
            (
                "pi",
                argv(&["--thinking", "{}"], &["--thinking"])
                    .levels(PI_LEVELS)
                    .native(),
            ),
            ("pool", Native::Unsupported("no reasoning-effort flag")),
            ("qwen", Native::Unsupported("no reasoning-effort flag")),
            ("goose", Native::Unsupported("no reasoning-effort flag")),
            (
                "aider",
                argv(&["--reasoning-effort", "{}"], &["--reasoning-effort"]).native(),
            ),
            // `--reasoning-effort` exists on `crush run` only.
            (
                "crush",
                Native::Unsupported("a reasoning-effort flag only on `crush run`"),
            ),
            // `--variant` exists on `kilo run` only.
            (
                "kilo",
                Native::Unsupported("a reasoning-effort flag only on `kilo run`"),
            ),
            ("fx", argv(&["--effort", "{}"], &["--effort"]).native()),
        ],
    },
    Alias {
        name: "print",
        also: &[],
        takes: Takes::Nothing,
        repeatable: false,
        about: "Answer the prompt that follows it without the UI, then exit",
        agents: [
            ("claude", argv(&["-p"], &["-p", "--print"]).native()),
            ("codex", argv(&[], &[]).sub("exec").native()),
            // `--single <PROMPT>`: the prompt must come right after --print.
            ("grok", argv(&["--single"], &["-p", "--single"]).native()),
            ("opencode", argv(&[], &[]).sub("run").native()),
            ("pi", argv(&["-p"], &["-p", "--print"]).native()),
            (
                "pool",
                Native::Unsupported("no one-shot flag that takes the prompt as an argument"),
            ),
            // Qwen's own `--prompt`; its positional prompt is one-shot already.
            ("qwen", argv(&["-p"], &["-p", "--prompt"]).native()),
            (
                "goose",
                Native::Unsupported("no flag that takes the prompt as an argument"),
            ),
            (
                "aider",
                argv(&["--message"], &["-m", "--message", "--msg"]).native(),
            ),
            ("crush", argv(&[], &[]).sub("run").native()),
            ("kilo", argv(&[], &[]).sub("run").native()),
            ("fx", argv(&[], &[]).sub("ask").native()),
        ],
    },
    Alias {
        name: "continue",
        also: &[],
        takes: Takes::Nothing,
        repeatable: false,
        about: "Continue the most recent session",
        agents: [
            (
                "claude",
                argv(&["--continue"], &["-c", "--continue"]).native(),
            ),
            (
                "codex",
                argv(&["--last"], &["--last"])
                    .sub("resume")
                    .after("exec")
                    .native(),
            ),
            (
                "grok",
                argv(&["--continue"], &["-c", "--continue"]).native(),
            ),
            (
                "opencode",
                argv(&["--continue"], &["-c", "--continue"])
                    .after("run")
                    .native(),
            ),
            ("pi", argv(&["--continue"], &["-c", "--continue"]).native()),
            (
                "pool",
                Native::Unsupported("no flag to continue an interactive session"),
            ),
            (
                "qwen",
                argv(&["--continue"], &["-c", "--continue"]).native(),
            ),
            (
                "goose",
                Native::Unsupported("no flag to continue without naming a subcommand"),
            ),
            (
                "aider",
                argv(&["--restore-chat-history"], &["--restore-chat-history"]).native(),
            ),
            (
                "crush",
                argv(&["--continue"], &["-C", "--continue"]).native(),
            ),
            (
                "kilo",
                argv(&["--continue"], &["-c", "--continue"])
                    .after("run")
                    .native(),
            ),
            (
                "fx",
                argv(&["--continue"], &["-c", "--continue"])
                    .under("ask", &["--resume", "last"], None)
                    .native(),
            ),
        ],
    },
    Alias {
        name: "resume",
        also: &[],
        takes: Takes::Optional("id"),
        repeatable: false,
        about: "Resume a session by id; with no id, the agent's own picker or latest",
        agents: [
            (
                "claude",
                argv(&["--resume", "{}"], &["-r", "--resume"])
                    .bare(&["--resume"])
                    .native(),
            ),
            (
                "codex",
                // `exec resume` with no id or `--last` reads the prompt from stdin.
                argv(&["{}"], &[])
                    .bare(&[])
                    .under("exec", &["--last"], None)
                    .sub("resume")
                    .after("exec")
                    .native(),
            ),
            (
                "grok",
                argv(&["--resume", "{}"], &["-r", "--resume"])
                    .bare(&["--resume"])
                    .native(),
            ),
            (
                "opencode",
                argv(&["--session", "{}"], &["-s", "--session"])
                    .after("run")
                    .native(),
            ),
            (
                "pi",
                argv(&["--session", "{}"], &["--session", "-r", "--resume"])
                    .bare(&["--resume"])
                    .native(),
            ),
            (
                "pool",
                argv(&["--resume", "{}"], &["-r", "--resume"])
                    .bare(&["--resume"])
                    .check(Check::Docs)
                    .native(),
            ),
            (
                "qwen",
                argv(&["--resume", "{}"], &["-r", "--resume"])
                    .bare(&["--resume"])
                    .native(),
            ),
            // `session` and `run` both take `--resume`; there is no root flag.
            (
                "goose",
                argv(&["--resume", "--session-id", "{}"], &["--session-id"])
                    .bare(&["--resume"])
                    .native(),
            ),
            (
                "aider",
                Native::Unsupported("no flag to resume a session by id"),
            ),
            (
                "crush",
                argv(&["--session", "{}"], &["-s", "--session"]).native(),
            ),
            (
                "kilo",
                argv(&["--session", "{}"], &["-s", "--session"])
                    .after("run")
                    .native(),
            ),
            (
                "fx",
                argv(&["--resume", "{}"], &["--resume", "--resume-last"])
                    .bare(&["--resume"])
                    .under("ask", &["--resume", "last"], None)
                    .native(),
            ),
        ],
    },
    Alias {
        name: "system",
        also: &[],
        takes: Takes::Value("text"),
        repeatable: false,
        about: "Replace the system prompt",
        agents: [
            (
                "claude",
                argv(&["--system-prompt", "{}"], &["--system-prompt"]).native(),
            ),
            ("codex", Native::Unsupported("no system-prompt flag")),
            (
                "grok",
                argv(
                    &["--system-prompt-override", "{}"],
                    &["--system-prompt-override", "--system-prompt"],
                )
                .native(),
            ),
            ("opencode", Native::Unsupported("no system-prompt flag")),
            (
                "pi",
                argv(&["--system-prompt", "{}"], &["--system-prompt"]).native(),
            ),
            ("pool", Native::Unsupported("no system-prompt flag")),
            (
                "qwen",
                argv(&["--system-prompt", "{}"], &["--system-prompt"]).native(),
            ),
            // Goose only adds to its default prompt; it has no replace flag.
            ("goose", argv(&["--system", "{}"], &["--system"]).native()),
            ("aider", Native::Unsupported("no system-prompt flag")),
            ("crush", Native::Unsupported("no system-prompt flag")),
            ("kilo", Native::Unsupported("no system-prompt flag")),
            // `fx ask --system` replaces the prompt; the interactive session has no flag.
            (
                "fx",
                argv(&["--system", "{}"], &["--system"])
                    .after("ask")
                    .native(),
            ),
        ],
    },
    Alias {
        name: "append-system",
        also: &[],
        takes: Takes::Value("text"),
        repeatable: true,
        about: "Add text to the default system prompt",
        agents: [
            (
                "claude",
                argv(
                    &["--append-system-prompt", "{}"],
                    &["--append-system-prompt"],
                )
                .native(),
            ),
            ("codex", Native::Unsupported("no system-prompt flag")),
            ("grok", argv(&["--rules", "{}"], &["--rules"]).native()),
            ("opencode", Native::Unsupported("no system-prompt flag")),
            (
                "pi",
                argv(
                    &["--append-system-prompt", "{}"],
                    &["--append-system-prompt"],
                )
                .native(),
            ),
            ("pool", Native::Unsupported("no system-prompt flag")),
            (
                "qwen",
                argv(
                    &["--append-system-prompt", "{}"],
                    &["--append-system-prompt"],
                )
                .native(),
            ),
            ("goose", argv(&["--system", "{}"], &["--system"]).native()),
            ("aider", Native::Unsupported("no system-prompt flag")),
            ("crush", Native::Unsupported("no system-prompt flag")),
            ("kilo", Native::Unsupported("no system-prompt flag")),
            (
                "fx",
                Native::Unsupported("no flag to append to the system prompt"),
            ),
        ],
    },
    Alias {
        name: "add-dir",
        also: &[],
        takes: Takes::Value("path"),
        repeatable: true,
        about: "Let the agent use one more directory (repeatable)",
        agents: [
            (
                "claude",
                argv(&["--add-dir", "{}"], &["--add-dir"]).native(),
            ),
            (
                "codex",
                argv(&["--add-dir", "{}"], &["--add-dir"])
                    .before("resume")
                    .native(),
            ),
            ("grok", Native::Unsupported("no flag to add a directory")),
            (
                "opencode",
                Native::Unsupported("no flag to add a directory"),
            ),
            ("pi", Native::Unsupported("no flag to add a directory")),
            ("pool", Native::Unsupported("no flag to add a directory")),
            // `=` form: yargs reads `--add-dir` as an array and would take the prompt.
            (
                "qwen",
                argv(&["--add-dir={}"], &["--add-dir", "--include-directories"]).native(),
            ),
            ("goose", Native::Unsupported("no flag to add a directory")),
            ("aider", Native::Unsupported("no flag to add a directory")),
            ("crush", Native::Unsupported("no flag to add a directory")),
            ("kilo", Native::Unsupported("no flag to add a directory")),
            ("fx", argv(&["--add-dir", "{}"], &["--add-dir"]).native()),
        ],
    },
    Alias {
        name: "cwd",
        also: &[],
        takes: Takes::Value("path"),
        repeatable: false,
        about: "Working directory for the session",
        agents: [
            ("claude", Native::Unsupported("no working-directory flag")),
            (
                "codex",
                argv(&["--cd", "{}"], &["-C", "--cd"])
                    .before("resume")
                    .native(),
            ),
            ("grok", argv(&["--cwd", "{}"], &["--cwd"]).native()),
            ("opencode", Native::Unsupported("no working-directory flag")),
            ("pi", Native::Unsupported("no working-directory flag")),
            (
                "pool",
                argv(&["--directory", "{}"], &["-C", "-d", "--directory"])
                    .check(Check::Docs)
                    .native(),
            ),
            ("qwen", Native::Unsupported("no working-directory flag")),
            ("goose", Native::Unsupported("no working-directory flag")),
            ("aider", Native::Unsupported("no working-directory flag")),
            ("crush", argv(&["--cwd", "{}"], &["-c", "--cwd"]).native()),
            ("kilo", Native::Unsupported("no working-directory flag")),
            ("fx", Native::Unsupported("no working-directory flag")),
        ],
    },
    Alias {
        name: "max-turns",
        also: &[],
        takes: Takes::Value("n"),
        repeatable: false,
        about: "Stop after this many agent turns",
        agents: [
            (
                "claude",
                argv(&["--max-turns", "{}"], &["--max-turns"])
                    .check(Check::Probe)
                    .native(),
            ),
            ("codex", Native::Unsupported("no turn-limit flag")),
            (
                "grok",
                argv(&["--max-turns", "{}"], &["--max-turns"]).native(),
            ),
            ("opencode", Native::Unsupported("no turn-limit flag")),
            ("pi", Native::Unsupported("no turn-limit flag")),
            ("pool", Native::Unsupported("no turn-limit flag")),
            (
                "qwen",
                argv(&["--max-session-turns", "{}"], &["--max-session-turns"]).native(),
            ),
            // `session` and `run` both take `--max-turns`; there is no root flag.
            (
                "goose",
                argv(&["--max-turns", "{}"], &["--max-turns"]).native(),
            ),
            ("aider", Native::Unsupported("no turn-limit flag")),
            ("crush", Native::Unsupported("no turn-limit flag")),
            ("kilo", Native::Unsupported("no turn-limit flag")),
            ("fx", Native::Unsupported("no turn-limit flag")),
        ],
    },
    Alias {
        name: "no-tools",
        also: &["tools-off"],
        takes: Takes::Nothing,
        repeatable: false,
        about: "Start with every built-in tool off",
        agents: [
            // `=` form: Commander's variadic `--tools <tools...>` would take the prompt.
            ("claude", argv(&["--tools="], &["--tools"]).native()),
            ("codex", Native::Unsupported("no tools switch")),
            ("grok", Native::Unsupported("no tools switch")),
            ("opencode", Native::Unsupported("no tools switch")),
            ("pi", argv(&["--no-tools"], &["--no-tools", "-nt"]).native()),
            ("pool", Native::Unsupported("no tools switch")),
            ("qwen", Native::Unsupported("no tools switch")),
            ("goose", Native::Unsupported("no tools switch")),
            ("aider", Native::Unsupported("no tools switch")),
            ("crush", Native::Unsupported("no tools switch")),
            ("kilo", Native::Unsupported("no tools switch")),
            ("fx", Native::Unsupported("no tools switch")),
        ],
    },
    Alias {
        name: "no-skills",
        also: &["skills-off"],
        takes: Takes::Nothing,
        repeatable: false,
        about: "Do not load skills",
        agents: [
            // Also turns off slash commands; the closest switch claude has.
            (
                "claude",
                argv(&["--disable-slash-commands"], &["--disable-slash-commands"]).native(),
            ),
            ("codex", Native::Unsupported("no skills switch")),
            ("grok", Native::Unsupported("no skills switch")),
            ("opencode", Native::Unsupported("no skills switch")),
            (
                "pi",
                argv(&["--no-skills"], &["--no-skills", "-ns"]).native(),
            ),
            ("pool", Native::Unsupported("no skills switch")),
            ("qwen", Native::Unsupported("no skills switch")),
            ("goose", Native::Unsupported("no skills switch")),
            ("aider", Native::Unsupported("no skills switch")),
            ("crush", Native::Unsupported("no skills switch")),
            ("kilo", Native::Unsupported("no skills switch")),
            ("fx", Native::Unsupported("no skills switch")),
        ],
    },
];

pub(crate) fn alias(name: &str) -> Option<&'static Alias> {
    ALIASES
        .iter()
        .find(|a| a.name == name || a.also.contains(&name))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Translated {
    pub(crate) args: Vec<String>,
    /// One line per alias the agent has no flag for.
    pub(crate) notices: Vec<String>,
    /// Env vars to set on the child for aliases the agent reads from its env.
    pub(crate) env: Vec<(&'static str, &'static str)>,
}

#[derive(Debug)]
enum Piece {
    User(String),
    Mapped {
        tokens: Vec<String>,
        anchor: Option<Anchor>,
        /// Replacement under a subcommand, and the flag as the user typed it.
        under: Option<(Under, String)>,
    },
}

#[derive(Debug, Clone, Copy)]
enum Anchor {
    /// Right after this subcommand, wherever the alias was.
    Sub(&'static str),
    /// After this subcommand when the alias came before it.
    After(&'static str),
    /// Right before this subcommand, when it is there.
    Before(&'static str),
}

/// Map portable aliases in an agent's args to the agent's own argv.
///
/// `lead` holds aliases anyr already parsed (`--yolo`, `--effort`); they go
/// first. In `args`, aliases before `raw_from` (the `--` boundary) map in
/// place; tokens from `raw_from` on stay raw.
pub(crate) fn apply(
    agent: &str,
    lead: &[(&str, Option<String>)],
    args: &[String],
    raw_from: usize,
) -> Result<Translated, String> {
    let Some(owned) = launch_flags(agent) else {
        return Ok(Translated {
            args: args.to_vec(),
            notices: Vec::new(),
            env: Vec::new(),
        });
    };
    let raw_from = raw_from.min(args.len());
    let mut pieces = Vec::new();
    let mut uses: Vec<(&Alias, String, Option<String>, usize)> = Vec::new();
    for (name, value) in lead {
        if let Some(a) = alias(name) {
            uses.push((a, name.to_string(), value.clone(), pieces.len()));
            pieces.push(Piece::Mapped {
                tokens: Vec::new(),
                anchor: None,
                under: None,
            });
        }
    }
    let mut i = 0;
    while i < args.len() {
        let token = &args[i];
        let hit = if i < raw_from {
            alias_at(token, owned)
        } else {
            None
        };
        let Some((alias, typed, inline)) = hit else {
            pieces.push(Piece::User(token.clone()));
            i += 1;
            continue;
        };
        let next = args[i + 1..raw_from.max(i + 1)].first();
        let value = match alias.takes {
            Takes::Nothing => None,
            Takes::Value(_) => match inline {
                Some(v) => Some(v),
                None => {
                    let v = next
                        .filter(|n| !n.starts_with("--"))
                        .ok_or_else(|| format!("Flag --{typed} requires a value."))?;
                    i += 1;
                    Some(v.clone())
                }
            },
            Takes::Optional(_) => inline.or_else(|| {
                let v = next.filter(|n| !n.starts_with('-'))?;
                i += 1;
                Some(v.clone())
            }),
        };
        uses.push((alias, typed, value, pieces.len()));
        pieces.push(Piece::Mapped {
            tokens: Vec::new(),
            anchor: None,
            under: None,
        });
        i += 1;
    }

    let present = |same: &[&str], pieces: &[Piece]| {
        pieces.iter().any(|p| match p {
            Piece::User(t) => same
                .iter()
                .any(|s| t == s || t.starts_with(&format!("{s}="))),
            Piece::Mapped { .. } => false,
        })
    };
    let mut notices = Vec::new();
    let mut env = Vec::new();
    let mut subs: Vec<(Option<usize>, &'static str, Option<&'static str>)> = Vec::new();
    for (alias, typed, value, at) in uses {
        let Some(native) = alias.native(agent) else {
            continue;
        };
        let a = match native {
            Native::Env(_) => continue,
            Native::EnvSet(name, value) => {
                env.push((*name, *value));
                continue;
            }
            Native::Unsupported(reason) => {
                notices.push(format!("{agent} has {reason}; ignoring --{typed}."));
                continue;
            }
            Native::Argv(a) => a,
        };
        if !alias.repeatable && present(a.same, &pieces) {
            continue;
        }
        let template = match (&value, alias.takes) {
            (None, Takes::Optional(_)) => a.bare,
            _ => Some(a.argv),
        };
        let Some(template) = template else {
            notices.push(format!(
                "{agent} needs a value for --{typed}; ignoring --{typed}."
            ));
            continue;
        };
        let value = value.map(|v| match a.levels {
            Some(levels) => crate::spawn::clamp_level(levels, &v).unwrap_or(v),
            None => v,
        });
        let tokens = template
            .iter()
            .map(|t| match &value {
                Some(v) => t.replace("{}", v),
                None => t.to_string(),
            })
            .collect();
        let anchor = a
            .sub
            .map(Anchor::Sub)
            .or(a.after.map(Anchor::After))
            .or(a.before.map(Anchor::Before));
        let under = a.under.filter(|_| value.is_none()).map(|u| (u, typed));
        pieces[at] = Piece::Mapped {
            tokens,
            anchor,
            under,
        };
        if let Some(sub) = a.sub {
            let rank = ALIASES.iter().position(|x| x.name == alias.name);
            subs.push((rank, sub, a.after));
        }
    }

    subs.sort_by_key(|(rank, _, _)| *rank);
    let known = subcommands(agent);
    for (_, sub, after) in subs {
        let chain = chain(&pieces, &known);
        if chain.iter().any(|&k| is_user(&pieces[k], sub)) {
            continue;
        }
        let at = after
            .and_then(|a| chain.iter().find(|&&k| is_user(&pieces[k], a)))
            .map_or(0, |&k| k + 1);
        pieces.insert(at, Piece::User(sub.to_string()));
    }

    let chain = chain(&pieces, &known);
    let words_in_chain: Vec<String> = chain
        .iter()
        .filter_map(|&k| match &pieces[k] {
            Piece::User(t) => Some(t.clone()),
            Piece::Mapped { .. } => None,
        })
        .collect();
    let user_tokens: Vec<String> = pieces
        .iter()
        .filter_map(|p| match p {
            Piece::User(t) => Some(t.clone()),
            Piece::Mapped { .. } => None,
        })
        .collect();
    for piece in &mut pieces {
        let Piece::Mapped {
            tokens,
            under: Some((under, typed)),
            ..
        } = piece
        else {
            continue;
        };
        if !words_in_chain.iter().any(|w| w == under.sub) {
            continue;
        }
        // #221: if the user already typed an under-token (e.g. --last),
        // adding it again gives `exec resume --last --last`.
        if under
            .tokens
            .iter()
            .any(|t| user_tokens.iter().any(|u| u == t))
        {
            *tokens = Vec::new();
        } else {
            *tokens = under.tokens.iter().map(|t| t.to_string()).collect();
        }
        if let Some(reason) = under.note {
            notices.push(format!(
                "{agent} {} has {reason}; ignoring --{typed}.",
                under.sub
            ));
        }
    }
    let target = |i: usize, p: &Piece| -> Option<(usize, bool)> {
        let Piece::Mapped {
            anchor: Some(anchor),
            ..
        } = p
        else {
            return None;
        };
        let (word, always, before) = match *anchor {
            Anchor::Sub(s) => (s, true, false),
            Anchor::After(s) => (s, false, false),
            Anchor::Before(s) => (s, true, true),
        };
        let k = *chain.iter().find(|&&k| is_user(&pieces[k], word))?;
        (always || i < k).then_some((k, before))
    };
    let targets: Vec<Option<(usize, bool)>> = pieces
        .iter()
        .enumerate()
        .map(|(i, p)| target(i, p))
        .collect();
    let mut out = Vec::new();
    for (i, piece) in pieces.iter().enumerate() {
        for (j, moved) in pieces.iter().enumerate() {
            if targets[j] == Some((i, true)) {
                push(&mut out, moved);
            }
        }
        if targets[i].is_none() {
            push(&mut out, piece);
        }
        for (j, moved) in pieces.iter().enumerate() {
            if targets[j] == Some((i, false)) {
                push(&mut out, moved);
            }
        }
    }
    Ok(Translated {
        args: out,
        notices,
        env,
    })
}

/// `--name` / `--name=value` naming a portable alias in a spelling anyr does
/// not already own on this launcher. A no-value alias spelled `--name=x` is not one.
fn alias_at(token: &str, owned: &[&str]) -> Option<(&'static Alias, String, Option<String>)> {
    let body = token.strip_prefix("--")?;
    let (name, inline) = match body.split_once('=') {
        Some((n, v)) => (n, Some(v.to_string())),
        None => (body, None),
    };
    let alias = alias(name).filter(|_| !owned.contains(&name))?;
    if alias.takes == Takes::Nothing && inline.is_some() {
        return None;
    }
    Some((alias, name.to_string(), inline))
}

fn subcommands(agent: &str) -> Vec<&'static str> {
    ALIASES
        .iter()
        .filter_map(|a| match a.native(agent) {
            Some(Native::Argv(n)) => Some([n.sub, n.after]),
            _ => None,
        })
        .flatten()
        .flatten()
        .collect()
}

/// Indices of the leading positional args that are known subcommands
/// (`exec resume`, `run`).
fn chain(pieces: &[Piece], known: &[&str]) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, p) in pieces.iter().enumerate() {
        let Piece::User(t) = p else { continue };
        if t.starts_with('-') {
            continue;
        }
        if !known.contains(&t.as_str()) {
            break;
        }
        out.push(i);
    }
    out
}

fn is_user(piece: &Piece, word: &str) -> bool {
    matches!(piece, Piece::User(t) if t == word)
}

fn push(out: &mut Vec<String>, piece: &Piece) {
    match piece {
        Piece::User(t) => out.push(t.clone()),
        Piece::Mapped { tokens, .. } => out.extend(tokens.iter().cloned()),
    }
}

/// One hint line after the agent failed right at launch: the first flag the
/// user forwarded that the agent's `--help` does not list, and the listed
/// flag closest to it. Flags the registry maps for this agent count as known,
/// since some are hidden from `--help`. Portable aliases before `raw_from`
/// (the `--` boundary) were rewritten or dropped by anyr, so they never count.
pub(crate) fn unknown_flag_hint(
    agent: &str,
    help: &str,
    forwarded: &[String],
    raw_from: usize,
) -> Option<String> {
    let owned = launch_flags(agent).unwrap_or(&[]);
    let listed = long_flags(help);
    let mapped: Vec<&str> = ALIASES
        .iter()
        .filter_map(|a| match a.native(agent) {
            Some(Native::Argv(n)) => Some(n.argv),
            _ => None,
        })
        .flatten()
        .copied()
        .collect();
    let unknown = forwarded
        .iter()
        .enumerate()
        .filter(|(i, t)| *i >= raw_from || alias_at(t, owned).is_none())
        .filter_map(|(_, t)| t.strip_prefix("--"))
        .map(|t| t.split('=').next().unwrap_or(t))
        .find(|name| {
            !name.is_empty()
                && !listed.iter().any(|l| l == name)
                && !mapped.iter().any(|m| m.strip_prefix("--") == Some(*name))
        })?;
    let closest = listed
        .iter()
        .find(|l| words(l) == words(unknown))
        .or_else(|| {
            listed
                .iter()
                .map(|l| (crate::cmd::dispatch::edit_distance(unknown, l), l))
                .filter(|(d, _)| *d <= 2)
                .min_by_key(|(d, _)| *d)
                .map(|(_, l)| l)
        });
    Some(match closest {
        Some(l) => format!("{agent} does not list --{unknown} in its --help. Did you mean --{l}?"),
        None => format!("{agent} does not list --{unknown} in its --help."),
    })
}

/// A flag's words, sorted, with `off` / `disable` read as `no`:
/// `skills-off` and `no-skills` match.
fn words(name: &str) -> Vec<&str> {
    let mut w: Vec<&str> = name
        .split('-')
        .map(|p| {
            if matches!(p, "off" | "disable") {
                "no"
            } else {
                p
            }
        })
        .collect();
    w.sort_unstable();
    w
}

/// Long flag names in help text, without the `--`.
fn long_flags(help: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in help.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        let Some(name) = word.strip_prefix("--") else {
            continue;
        };
        let name = name.trim_end_matches('-');
        if name.starts_with(|c: char| c.is_ascii_alphanumeric()) && !out.iter().any(|o| o == name) {
            out.push(name.to_string());
        }
    }
    out
}

fn usage(alias: &Alias) -> String {
    let also: String = alias.also.iter().map(|a| format!(", --{a}")).collect();
    match alias.takes {
        Takes::Nothing => format!("--{}{also}", alias.name),
        Takes::Value(v) => format!("--{} <{v}>", alias.name),
        Takes::Optional(v) => format!("--{} [<{v}>]", alias.name),
    }
}

/// The agent's argv as text: `{}` shown as the alias placeholder.
fn native_text(alias: &Alias, a: &Argv) -> String {
    let placeholder = match alias.takes {
        Takes::Value(v) | Takes::Optional(v) => format!("<{v}>"),
        Takes::Nothing => String::new(),
    };
    let mut words: Vec<String> = a.sub.iter().map(|s| s.to_string()).collect();
    words.extend(a.argv.iter().map(|t| t.replace("{}", &placeholder)));
    words.join(" ")
}

fn check_note(check: Check) -> &'static str {
    match check {
        Check::Help => "",
        Check::Probe => " (hidden from --help)",
        Check::Docs => " (from docs, unverified)",
    }
}

/// The portable-flag lines of `anyr <agent> --help`.
pub(crate) fn help_lines(agent: &str, label: &str) -> String {
    let mut out = String::new();
    for alias in ALIASES {
        let what = match alias.native(agent) {
            Some(Native::Argv(a)) => format!(
                "{}; passes {}{}",
                alias.about,
                native_text(alias, a),
                check_note(a.check)
            ),
            Some(Native::Env(var)) => format!("{}; sets {var}", alias.about),
            Some(Native::EnvSet(name, value)) => {
                format!("{}; sets {name}={value}", alias.about)
            }
            Some(Native::Unsupported(reason)) => format!("No effect: {label} has {reason}"),
            None => continue,
        };
        out.push_str(&format!("  {:<21} {what}\n", usage(alias)));
    }
    out
}

/// The README block between the `portable-flags` markers.
#[cfg(test)]
pub(crate) fn readme_table() -> String {
    let mut out = format!("| Flag | {} |\n", AGENTS.join(" | "));
    out.push_str(&format!("|------|{}\n", "---|".repeat(AGENTS.len())));
    for alias in ALIASES {
        let cells: Vec<String> = AGENTS
            .iter()
            .map(|agent| match alias.native(agent) {
                Some(Native::Argv(a)) => {
                    let mark = match a.check {
                        Check::Help => "",
                        Check::Probe => " †",
                        Check::Docs => " *",
                    };
                    format!("`{}`{mark}", native_text(alias, a))
                }
                Some(Native::Env(var)) => format!("env `{var}`"),
                Some(Native::EnvSet(name, value)) => format!("env `{name}={value}`"),
                Some(Native::Unsupported(_)) | None => "no".into(),
            })
            .collect();
        out.push_str(&format!("| `{}` | {} |\n", usage(alias), cells.join(" | ")));
    }
    let versions: Vec<String> = CHECKED
        .iter()
        .filter(|(a, _)| *a != "pool")
        .map(|(a, v)| format!("{a} {v}"))
        .collect();
    out.push_str(&format!(
        "\nChecked against each agent's `--help`: {}. † hidden from `--help`; the \
agent's parser accepts it. * from docs.poolside.ai (v1.0.16), not run. \
\"no\": the agent has no such flag, so anyr drops it with a one-line note. \
Goose takes these flags only on `goose session` or `goose run`, and its `--system` adds to \
the default prompt. Crush `-y` works on the interactive command only. Qwen's `-p` is \
deprecated but listed. fx takes `--add-dir` only on the interactive command and `--system` only on `fx ask`.\n",
        versions.join(", ")
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn run(agent: &str, args: &[&str]) -> Translated {
        apply(agent, &[], &s(args), args.len()).unwrap()
    }

    #[test]
    fn every_alias_has_a_cell_for_every_launcher() {
        for alias in ALIASES {
            let agents: Vec<&str> = alias.agents.iter().map(|(a, _)| *a).collect();
            assert_eq!(agents, AGENTS, "--{}", alias.name);
        }
    }

    #[test]
    fn codex_print_and_continue_nest_as_exec_resume() {
        assert_eq!(
            run("codex", &["--continue", "--print", "fix it"]).args,
            s(&["exec", "resume", "--last", "fix it"])
        );
        assert_eq!(
            run("codex", &["exec", "--json", "--continue", "fix it"]).args,
            s(&["exec", "resume", "--last", "--json", "fix it"])
        );
    }

    #[test]
    fn codex_resume_does_not_add_last_twice_when_user_typed_it() {
        // WHY #221: --print --resume --last gave exec resume --last --last.
        assert_eq!(
            run("codex", &["--print", "--resume", "--last", "fix it"]).args,
            s(&["exec", "resume", "--last", "fix it"])
        );
    }

    #[test]
    fn codex_session_id_lands_right_after_resume() {
        assert_eq!(
            run("codex", &["--print", "fix it", "--resume", "abc"]).args,
            s(&["exec", "resume", "abc", "fix it"])
        );
        assert_eq!(run("codex", &["--resume"]).args, s(&["resume"]));
    }

    #[test]
    fn subcommand_the_user_typed_is_not_added_twice() {
        assert_eq!(
            run("opencode", &["run", "--print", "x"]).args,
            s(&["run", "x"])
        );
        assert_eq!(
            run("codex", &["exec", "--print", "x"]).args,
            s(&["exec", "x"])
        );
    }

    #[test]
    fn opencode_flags_move_after_run_but_keep_order_after_it() {
        assert_eq!(
            run("opencode", &["--continue", "run", "--resume", "s1", "x"]).args,
            s(&["run", "--continue", "--session", "s1", "x"])
        );
    }

    #[test]
    fn effort_clamps_to_the_agents_levels() {
        let lead = [("effort", Some("max".to_string()))];
        assert_eq!(
            apply("codex", &lead, &[], 0).unwrap().args,
            s(&["-c", "model_reasoning_effort=\"high\""])
        );
        assert_eq!(
            apply("pi", &lead, &[], 0).unwrap().args,
            s(&["--thinking", "xhigh"])
        );
        assert_eq!(
            apply("grok", &lead, &[], 0).unwrap().args,
            s(&["--reasoning-effort", "xhigh"])
        );
        assert_eq!(
            apply("claude", &lead, &[], 0).unwrap(),
            Translated {
                args: vec![],
                notices: vec![],
                env: vec![]
            }
        );
    }

    #[test]
    fn goose_yolo_sets_its_env_and_keeps_its_own_session_flags() {
        let lead = [("yolo", None)];
        let t = apply("goose", &lead, &s(&["session", "--resume"]), 2).unwrap();
        assert_eq!(t.env, vec![("GOOSE_MODE", "auto")]);
        assert_eq!(t.args, s(&["session", "--resume"]));
        assert_eq!(
            run("goose", &["session", "--resume", "abc"]).args,
            s(&["session", "--resume", "--session-id", "abc"])
        );
    }

    #[test]
    fn kilo_follows_opencode_and_crush_keeps_yolo_off_run() {
        assert_eq!(
            run("kilo", &["--continue", "run", "x"]).args,
            s(&["run", "--continue", "x"])
        );
        assert_eq!(run("crush", &["--print", "x"]).args, s(&["run", "x"]));
        let lead = [("yolo", None)];
        assert_eq!(apply("crush", &lead, &[], 0).unwrap().args, s(&["-y"]));
    }

    #[test]
    fn raw_boundary_past_the_end_reads_as_no_raw_tail() {
        assert_eq!(
            apply("pi", &[], &s(&["--continue"]), 9).unwrap().args,
            s(&["--continue"])
        );
    }

    #[test]
    fn native_effort_wins_over_anyr_effort() {
        let lead = [("effort", Some("low".to_string()))];
        let args = s(&["-c", "model_reasoning_effort=\"high\""]);
        assert_eq!(apply("codex", &lead, &args, 2).unwrap().args, args);
    }

    const PI_HELP: &str = "  --no-tools, -nt   Disable all tools\n  --no-skills, -ns  Disable skills\n  --tools, -t <tools>  Allowlist\n";

    #[test]
    fn unknown_flag_hint_names_the_listed_flag_with_the_same_words() {
        assert_eq!(
            unknown_flag_hint("pi", PI_HELP, &s(&["--no-tools", "--skills-off", "x"]), 0)
                .as_deref(),
            Some("pi does not list --skills-off in its --help. Did you mean --no-skills?")
        );
        assert_eq!(
            unknown_flag_hint("pi", PI_HELP, &s(&["--tool=a"]), 0).as_deref(),
            Some("pi does not list --tool in its --help. Did you mean --tools?")
        );
        assert_eq!(
            unknown_flag_hint("pi", PI_HELP, &s(&["--frobnicate"]), 0).as_deref(),
            Some("pi does not list --frobnicate in its --help.")
        );
        assert_eq!(
            unknown_flag_hint("pi", PI_HELP, &s(&["--no-tools", "-x", "a"]), 0),
            None
        );
    }

    #[test]
    fn hidden_flags_the_registry_maps_are_not_unknown() {
        assert_eq!(
            unknown_flag_hint("claude", "  --help\n", &s(&["--max-turns", "3"]), 0),
            None
        );
    }

    #[test]
    fn readme_block_matches_registry() {
        // Windows checkouts may turn LF into CRLF.
        let readme = include_str!("../../README.md").replace("\r\n", "\n");
        let start = "<!-- portable-flags:start -->\n";
        let end = "<!-- portable-flags:end -->";
        let block = readme
            .split_once(start)
            .and_then(|(_, rest)| rest.split_once(end))
            .map(|(b, _)| b)
            .expect("README has the portable-flags markers");
        assert_eq!(
            block,
            readme_table(),
            "README portable-flags block is stale; replace it with:\n{}",
            readme_table()
        );
    }
}
