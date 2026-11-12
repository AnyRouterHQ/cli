//! Paste-ready prompts for coding agents to wire AnyRouter into a project.
//!
//! Mirrors `anyrouter/packages/lib/src/agent-prompts.ts` (implement / plan /
//! discover + migration sources) plus CLI-only `fix` and `deploy`. Keep
//! prompts self-contained and pointed at raw markdown docs.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::parse::ParsedArgs;
use crate::term;

pub const OPENAI_BASE_URL: &str = "https://anyrouter.dev/api/v1";
pub const ANTHROPIC_BASE_URL: &str = "https://anyrouter.dev/api";
pub const QUICKSTART_MD: &str = "https://docs.anyrouter.dev/getting-started/quickstart.md";
pub const LLMS_TXT: &str = "https://anyrouter.dev/llms.txt";
pub const LLMS_FULL_TXT: &str = "https://anyrouter.dev/llms-full.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptVariant {
    pub id: &'static str,
    pub label: &'static str,
    pub tagline: &'static str,
}

impl PromptVariant {
    /// The prompt text, packed with the help pages (`assets/help/onboard_<id>.txt`).
    pub fn prompt(&self) -> String {
        crate::help::doc(&format!("onboard_{}", self.id))
    }
}

/// Core + migration variants shown in the interactive picker (display order).
pub const VARIANTS: &[PromptVariant] = &[
    PromptVariant {
        id: "impl",
        label: "Set up AnyRouter",
        tagline: "Wire AnyRouter into an existing project.",
    },
    PromptVariant {
        id: "plan",
        label: "Plan a migration",
        tagline: "Audit the codebase and draft a migration plan — no changes yet.",
    },
    PromptVariant {
        id: "fix",
        label: "Fix LLM wiring",
        tagline: "Repair base URL, key, and model ids when requests fail.",
    },
    PromptVariant {
        id: "deploy",
        label: "Deploy / CI secrets",
        tagline: "Wire ANYROUTER_API_KEY and base URL into prod and CI.",
    },
    PromptVariant {
        id: "discover",
        label: "Pick the right models",
        tagline: "Recommend models based on what this project actually does.",
    },
    PromptVariant {
        id: "openai",
        label: "From OpenAI",
        tagline: "Migrate an OpenAI SDK project.",
    },
    PromptVariant {
        id: "anthropic",
        label: "From Anthropic",
        tagline: "Migrate an Anthropic SDK project.",
    },
    PromptVariant {
        id: "openrouter",
        label: "From OpenRouter",
        tagline: "Swap base URL; keep provider/model ids.",
    },
    PromptVariant {
        id: "claude-code",
        label: "Claude Code",
        tagline: "Point Claude Code / Anthropic tools at AnyRouter.",
    },
    PromptVariant {
        id: "hermes",
        label: "Hermes / OpenClaw",
        tagline: "Run Hermes or OpenClaw through the gateway.",
    },
];

/// Normalize user aliases (`implement` → `impl`, etc.).
pub fn resolve_mode(raw: &str) -> Option<&'static PromptVariant> {
    let lower = raw.trim().to_ascii_lowercase();
    let key = match lower.as_str() {
        "implement" | "implementation" => "impl",
        "migrate" => "plan",
        other => other,
    };
    VARIANTS.iter().find(|v| v.id == key)
}

pub fn usage_hint(bin: &str) -> String {
    format!(
        "\
{bin} onboard — paste-ready prompts for coding agents

Usage:
  {bin} onboard                 Interactive pick (TTY)
  {bin} onboard <mode>          Print the prompt
  {bin} onboard cp [mode]       Copy to clipboard (or print + hint)
  {bin} impl | plan | fix | deploy | cp

Modes:
  impl        Set up AnyRouter (alias: implement)
  plan        Plan a migration (no code changes)
  fix         Repair LLM wiring through AnyRouter
  deploy      Wire secrets into deploy / CI
  discover    Recommend models for this project
  openai | anthropic | openrouter | claude-code | hermes

Options:
  --json      Emit {{\"mode\",\"label\",\"prompt\"}}
  --copy      Copy instead of printing (same as `cp`)
  -h, --help  Show this help
"
    )
}

fn want_copy(parsed: &ParsedArgs, command: &str, mode_args: &[String]) -> bool {
    parsed.flag_true("copy")
        || command == "cp"
        || mode_args.first().map(|s| s.as_str()) == Some("cp")
}

fn mode_token(command: &str, mode_args: &[String]) -> Option<String> {
    match command {
        "onboard" => {
            let mut args = mode_args.iter().map(|s| s.as_str());
            match args.next() {
                Some("cp") => args.next().map(|s| s.to_string()),
                Some(other) => Some(other.to_string()),
                None => None,
            }
        }
        "cp" => mode_args.first().cloned(),
        "impl" | "plan" | "fix" | "deploy" => Some(command.to_string()),
        _ => None,
    }
}

fn pick_variant_interactive() -> Result<&'static PromptVariant, String> {
    let items: Vec<String> = VARIANTS
        .iter()
        .map(|v| format!("{} — {}", v.id, v.label))
        .collect();
    let idx = term::pick("Agent onboard prompt", &items, Some(0))?;
    Ok(&VARIANTS[idx])
}

fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let candidates: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("pbcopy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
        ("clip.exe", &[]),
    ];
    for (bin, args) in candidates {
        let Ok(mut child) = Command::new(bin)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(text.as_bytes()).is_err() {
                continue;
            }
        }
        if child.wait().map(|s| s.success()).unwrap_or(false) {
            return Ok(());
        }
    }
    Err("No clipboard tool found (tried wl-copy, pbcopy, xclip, xsel, clip.exe).".into())
}

fn emit(variant: &PromptVariant, json: bool, copy: bool) -> Result<i32, String> {
    let prompt = variant.prompt();
    if json {
        let payload = serde_json::json!({
            "mode": variant.id,
            "label": variant.label,
            "tagline": variant.tagline,
            "prompt": prompt,
        });
        println!("{payload}");
        return Ok(0);
    }
    if copy {
        match copy_to_clipboard(&prompt) {
            Ok(()) => {
                eprintln!(
                    "{} copied ({})",
                    term::accent(variant.id),
                    term::dim(variant.label)
                );
                return Ok(0);
            }
            Err(err) => {
                eprintln!("{err}");
                eprintln!("{}", term::dim("Printing prompt instead:"));
            }
        }
    }
    print!("{prompt}");
    if term::is_interactive() && !copy {
        eprintln!(
            "\n{}  {}  ·  pipe or `{bin} onboard cp {id}`",
            term::dim("Paste into your coding agent."),
            term::dim(variant.tagline),
            bin = crate::help::invoked_bin(),
            id = variant.id,
        );
    }
    Ok(0)
}

/// Entry for `onboard` / `impl` / `plan` / `fix` / `deploy` / `cp`.
pub fn run(command: &str, parsed: &ParsedArgs) -> Result<i32, String> {
    let mode_args = &parsed.passthrough;
    let copy = want_copy(parsed, command, mode_args);
    let json = parsed.flag_true("json");

    let variant = if let Some(token) = mode_token(command, mode_args) {
        resolve_mode(&token).ok_or_else(|| {
            format!(
                "Unknown onboard mode \"{token}\". Run \"{} onboard --help\".",
                crate::help::invoked_bin()
            )
        })?
    } else if term::is_interactive() {
        pick_variant_interactive()?
    } else {
        return Err(format!(
            "Specify a mode (e.g. impl, plan, fix). Run \"{} onboard --help\".",
            crate::help::invoked_bin()
        ));
    };

    emit(variant, json, copy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse_cli_args, FlagValue};
    use std::collections::HashMap;

    #[test]
    fn resolve_aliases() {
        assert_eq!(resolve_mode("implement").unwrap().id, "impl");
        assert_eq!(resolve_mode("IMPL").unwrap().id, "impl");
        assert_eq!(resolve_mode("plan").unwrap().id, "plan");
        assert_eq!(resolve_mode("fix").unwrap().id, "fix");
        assert_eq!(resolve_mode("deploy").unwrap().id, "deploy");
        assert_eq!(resolve_mode("claude-code").unwrap().id, "claude-code");
        assert!(
            resolve_mode("setup").is_none(),
            "setup is login at the top level, not an onboard alias"
        );
        assert!(resolve_mode("nope").is_none());
    }

    #[test]
    fn prompts_mention_contract() {
        for v in VARIANTS {
            assert!(
                v.prompt().contains("ANYROUTER_API_KEY") || v.id == "discover" || v.id == "hermes",
                "{} missing key hint",
                v.id
            );
            assert!(
                v.prompt().contains(OPENAI_BASE_URL)
                    || v.prompt().contains(ANTHROPIC_BASE_URL)
                    || v.prompt().contains(QUICKSTART_MD),
                "{} missing base URL / quickstart",
                v.id
            );
        }
        let impl_p = resolve_mode("impl").unwrap().prompt();
        assert!(impl_p.contains(OPENAI_BASE_URL));
        assert!(impl_p.contains(ANTHROPIC_BASE_URL));
        assert!(impl_p.contains("ANYROUTER_API_KEY"));
        let plan_p = resolve_mode("plan").unwrap().prompt();
        assert!(plan_p.to_ascii_lowercase().contains("do not change"));
    }

    #[test]
    fn mode_token_from_aliases() {
        let parsed = parse_cli_args(["onboard", "impl"]).unwrap();
        assert_eq!(
            mode_token("onboard", &parsed.passthrough).as_deref(),
            Some("impl")
        );
        let parsed = parse_cli_args(["onboard", "cp", "plan"]).unwrap();
        assert_eq!(
            mode_token("onboard", &parsed.passthrough).as_deref(),
            Some("plan")
        );
        assert!(want_copy(&parsed, "onboard", &parsed.passthrough));
        let parsed = parse_cli_args(["fix"]).unwrap();
        assert_eq!(
            mode_token("fix", &parsed.passthrough).as_deref(),
            Some("fix")
        );
        let mut flags = HashMap::new();
        flags.insert("copy".into(), FlagValue::Bool(true));
        let with_flag = ParsedArgs {
            command: "onboard".into(),
            flags,
            passthrough: vec!["deploy".into()],
            raw_from: None,
        };
        assert!(want_copy(&with_flag, "onboard", &with_flag.passthrough));
    }

    #[test]
    fn emit_json_shape() {
        let v = resolve_mode("impl").unwrap();
        let payload = serde_json::json!({
            "mode": v.id,
            "label": v.label,
            "tagline": v.tagline,
            "prompt": v.prompt(),
        });
        assert_eq!(payload["mode"], "impl");
        assert!(payload["prompt"]
            .as_str()
            .unwrap()
            .contains("ANYROUTER_API_KEY"));
    }
}
