//! `anyr <agent> [flags]`: anyr keeps its own flags, maps portable flags
//! (`--yolo`, `--continue`, `--add-dir`, ...) to the agent's native argv, and
//! hands every other flag to the agent in order.

mod common;

use std::process::Command;

const KEY: &str = "sk-ar-v1-fixture-key-0001";

/// Agent, and its native skip-permission argv (empty: none).
const AGENTS: &[(&str, &[&str])] = &[
    ("claude", &["--dangerously-skip-permissions"]),
    ("codex", &["--dangerously-bypass-approvals-and-sandbox"]),
    ("grok", &["--always-approve"]),
    ("opencode", &["--auto"]),
    ("pi", &[]),
    ("pool", &["--mode", "always-allow"]),
    ("qwen", &["--approval-mode", "yolo"]),
    ("aider", &["--yes-always"]),
    ("crush", &["-y"]),
    ("kilo", &["--auto"]),
];

struct DryRun {
    /// Files anyr wrote for the agent (Crush's private crush.json).
    files: String,
    code: i32,
    args: Vec<String>,
    stdout: String,
    stderr: String,
}

/// `anyr <agent> <anyr flags> <extra...>` in dry-run, with a nonexistent
/// agent path so OpenCode's version probe cannot add `--standalone`.
fn dry_run(agent: &str, extra: &[&str]) -> DryRun {
    let home = common::unique_temp_dir("anyr-passthrough");
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .args([
            agent,
            "--dry-run",
            "--yes",
            "--no-mcp",
            "--key",
            KEY,
            "--command-path",
            "/nonexistent/agent-bin",
        ])
        .args(extra)
        .env("ANYROUTER_HOME", &home)
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .expect("spawn anyr");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let args = stdout
        .lines()
        .find_map(|l| l.strip_prefix("args: "))
        .map(|json| serde_json::from_str(json).expect("args line is JSON"))
        .unwrap_or_default();
    DryRun {
        files: std::fs::read_to_string(home.join("crush").join("crush.json")).unwrap_or_default(),
        code: out.status.code().unwrap_or(1),
        args,
        stdout,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Index of `needle`, requiring it to appear exactly once.
fn only_at(args: &[String], needle: &str) -> usize {
    let at: Vec<usize> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == needle)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(at.len(), 1, "{needle} should appear once in {args:?}");
    at[0]
}

fn count_run(haystack: &[String], needle: &[&str]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| w.iter().zip(needle).all(|(a, b)| a == b))
        .count()
}

#[test]
fn unknown_flags_reach_the_agent_in_order_without_separator() {
    for (agent, _) in AGENTS {
        let base = dry_run(agent, &[]);
        assert_eq!(base.code, 0, "{agent}: {}", base.stderr);
        let extra = ["--frobnicate", "-x", "--tools", "read,grep", "--quux"];
        let got = dry_run(agent, &extra);
        assert_eq!(got.code, 0, "{agent}: {}", got.stderr);
        let mut want = base.args.clone();
        want.extend(strs(&extra));
        assert_eq!(got.args, want, "{agent}");
    }
}

#[test]
fn equals_form_unknown_flag_is_forwarded_not_parsed_as_bool() {
    let got = dry_run("pi", &["--tools=read,grep"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert_eq!(
        got.args.last().map(String::as_str),
        Some("--tools=read,grep")
    );
}

#[test]
fn yolo_maps_to_each_agents_native_flag() {
    for (agent, native) in AGENTS {
        let got = dry_run(agent, &["--yolo", "--verbose"]);
        assert_eq!(got.code, 0, "{agent}: {}", got.stderr);
        assert!(
            !got.args.iter().any(|a| a == "--yolo"),
            "{agent}: {:?}",
            got.args
        );
        if native.is_empty() {
            assert!(
                got.stderr
                    .contains("pi has no flag to skip permission prompts"),
                "{agent} must say --yolo was dropped: {}",
                got.stderr
            );
            assert_eq!(got.args, dry_run(agent, &["--verbose"]).args, "{agent}");
        } else {
            assert_eq!(count_run(&got.args, native), 1, "{agent}: {:?}", got.args);
            assert_eq!(got.args.last().map(String::as_str), Some("--verbose"));
        }
    }
}

#[test]
fn native_flag_passed_directly_is_not_doubled_by_yolo() {
    for (agent, native) in AGENTS.iter().filter(|(_, n)| !n.is_empty()) {
        let mut extra = native.to_vec();
        extra.push("--yolo");
        let got = dry_run(agent, &extra);
        assert_eq!(got.code, 0, "{agent}: {}", got.stderr);
        assert_eq!(count_run(&got.args, native), 1, "{agent}: {:?}", got.args);
    }
}

#[test]
fn separator_sends_anyr_flag_names_to_the_agent_raw() {
    for (agent, native) in AGENTS {
        let base = dry_run(agent, &[]);
        let raw = ["--model", "raw/agent-model", "--yolo", "--dry-run"];
        let mut extra = vec!["--"];
        extra.extend(raw);
        let got = dry_run(agent, &extra);
        assert_eq!(got.code, 0, "{agent}: {}", got.stderr);
        let mut want = if *agent == "kilo" {
            // Kilo suppresses anyr's own -m when the raw tail picks a model,
            // so there is no anyr prefix to keep (#230).
            vec![]
        } else {
            base.args.clone()
        };
        want.extend(strs(&raw));
        assert_eq!(got.args, want, "{agent}: raw tail, no yolo mapping");
        if let Some(first) = native.first() {
            assert!(!got.args.iter().any(|a| a == first), "{agent}");
        }
        let anyr_side: String = got
            .stdout
            .lines()
            .filter(|l| !l.starts_with("args: "))
            .collect();
        assert!(
            !anyr_side.contains("raw/agent-model"),
            "{agent}: anyr took the raw --model:\n{}",
            got.stdout
        );
    }
}

#[test]
fn anyr_model_flag_is_still_consumed() {
    for (agent, _) in AGENTS {
        let with_model = dry_run(agent, &["--model", "test-owner/m1"]);
        assert_eq!(with_model.code, 0, "{agent}: {}", with_model.stderr);
        assert!(
            with_model.stdout.contains("test-owner/m1")
                || with_model.files.contains("test-owner/m1"),
            "{agent} never got the model:\n{}",
            with_model.stdout
        );
        let mixed = dry_run(agent, &["--frobnicate", "--model", "test-owner/m1", "-v"]);
        let mut want = with_model.args.clone();
        want.extend(strs(&["--frobnicate", "-v"]));
        assert_eq!(mixed.args, want, "{agent}");
    }
}

#[test]
fn claude_only_flags_go_to_other_agents() {
    let got = dry_run("codex", &["--haiku", "x"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert!(
        got.args.ends_with(&strs(&["--haiku", "x"])),
        "{:?}",
        got.args
    );
    let claude = dry_run("claude", &["--haiku", "test-owner/h"]);
    assert!(
        !claude.args.iter().any(|a| a == "--haiku"),
        "{:?}",
        claude.args
    );
    assert!(claude.stdout.contains("test-owner/h"), "{}", claude.stdout);
}

#[test]
fn non_launcher_commands_still_reject_unknown_flags() {
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .args(["usage", "--no-skills"])
        .env("ANYR_NO_UPDATE", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Unknown flag --no-skills"));
}

#[test]
fn yolo_follows_the_subcommand_the_flag_belongs_to() {
    let opencode = dry_run("opencode", &["run", "--yolo", "fix it"]);
    assert_eq!(opencode.code, 0, "{}", opencode.stderr);
    assert!(
        opencode.args.ends_with(&strs(&["run", "--auto", "fix it"])),
        "{:?}",
        opencode.args
    );
    let codex = dry_run("codex", &["exec", "--yolo", "fix it"]);
    assert!(
        codex.args.ends_with(&strs(&[
            "--dangerously-bypass-approvals-and-sandbox",
            "exec",
            "fix it"
        ])),
        "{:?}",
        codex.args
    );
}

#[test]
fn cc_alias_forwards_like_claude() {
    let got = dry_run("cc", &["--frobnicate", "--yolo"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert_eq!(
        got.args,
        strs(&["--dangerously-skip-permissions", "--frobnicate"])
    );
}

#[test]
fn pool_yolo_passes_mode_always_allow_and_not_yolo() {
    let got = dry_run("pool", &["--yolo", "--dry-run"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert_eq!(count_run(&got.args, &["--mode"]), 1, "{:?}", got.args);
    assert_eq!(count_run(&got.args, &["always-allow"]), 1, "{:?}", got.args);
    assert_eq!(count_run(&got.args, &["--yolo"]), 0, "{:?}", got.args);
    assert_eq!(
        count_run(&got.args, &["--unsafe-auto-allow"]),
        0,
        "{:?}",
        got.args
    );
}

#[test]
fn json_is_anyrs_and_reaches_the_agent_only_after_separator() {
    for agent in ["claude", "codex"] {
        let owned = dry_run(agent, &["--json", "-p", "x"]);
        assert_eq!(owned.code, 0, "{}", owned.stderr);
        assert!(
            !owned.args.iter().any(|a| a == "--json"),
            "{agent}: {:?}",
            owned.args
        );
        let raw = dry_run(agent, &["--", "--json"]);
        assert_eq!(raw.code, 0, "{}", raw.stderr);
        assert_eq!(
            raw.args.last().map(String::as_str),
            Some("--json"),
            "{agent}"
        );
    }
    let claude = dry_run("claude", &["--quiet", "-p", "x"]);
    assert_eq!(claude.code, 0, "{}", claude.stderr);
    assert!(
        claude.args.ends_with(&strs(&["--quiet", "-p", "x"])),
        "{:?}",
        claude.args
    );
}

#[test]
fn pool_keeps_its_documented_json_flag() {
    let got = dry_run("pool", &["--json"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert!(!got.args.iter().any(|a| a == "--json"), "{:?}", got.args);
}

#[test]
fn non_launcher_unknown_flag_still_exits_2() {
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .args(["models", "--bogus"])
        .env("ANYR_NO_UPDATE", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn opencode_top_level_yolo_leads_the_args() {
    // opencode 2.0.24 `--help` lists `--auto` on the root command too.
    let base = dry_run("opencode", &["fix it"]).args;
    let got = dry_run("opencode", &["--yolo", "fix it"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    let mut want = strs(&["--auto"]);
    want.extend(base);
    assert_eq!(got.args, want);
}

#[test]
fn skills_off_and_tools_off_map_to_pi_native_flags() {
    let got = dry_run("pi", &["--skills-off", "--tools-off"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    only_at(&got.args, "--no-skills");
    only_at(&got.args, "--no-tools");
    assert!(
        !got.args.iter().any(|a| a.ends_with("-off")),
        "{:?}",
        got.args
    );
    let native = dry_run("pi", &["--no-skills"]);
    assert_eq!(native.args.last().map(String::as_str), Some("--no-skills"));
}

#[test]
fn skills_off_leads_a_claude_subcommand_and_a_literal_separator() {
    let sub = dry_run("claude", &["--skills-off", "mcp", "list"]);
    assert_eq!(sub.code, 0, "{}", sub.stderr);
    assert!(only_at(&sub.args, "--disable-slash-commands") < only_at(&sub.args, "mcp"));
    assert!(
        !sub.args.iter().any(|a| a == "--skills-off"),
        "{:?}",
        sub.args
    );
    // anyr consumes the separator; what follows is forwarded raw, after the flag.
    let sep = dry_run("claude", &["--skills-off", "--", "-p", "hi"]);
    assert!(only_at(&sep.args, "--disable-slash-commands") < only_at(&sep.args, "-p"));
    assert!(sep.args.ends_with(&strs(&["-p", "hi"])), "{:?}", sep.args);
}

#[test]
fn skills_off_is_dropped_with_a_note_where_the_agent_has_no_switch() {
    let got = dry_run("codex", &["--skills-off"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert!(
        !got.args.iter().any(|a| a == "--skills-off"),
        "{:?}",
        got.args
    );
    assert!(
        got.stderr
            .contains("codex has no skills switch; ignoring --skills-off"),
        "{}",
        got.stderr
    );
    let claude = dry_run("claude", &["--skills-off"]);
    only_at(&claude.args, "--disable-slash-commands");
}

#[test]
fn skills_off_after_separator_stays_raw() {
    let base = dry_run("pi", &[]).args;
    let got = dry_run("pi", &["--", "--skills-off", "--tools-off"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    let mut want = base;
    want.extend(strs(&["--skills-off", "--tools-off"]));
    assert_eq!(got.args, want);
}

#[test]
fn skills_off_equals_true_maps_and_never_rewrites_a_value() {
    let got = dry_run("pi", &["--skills-off=true"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    only_at(&got.args, "--no-skills");
    let value = dry_run("pi", &["-p", "--", "--skills-off"]);
    assert_eq!(value.args.last().map(String::as_str), Some("--skills-off"));
}

/// Portable flag × agent: what the agent receives after anyr's own args.
/// None: the agent has no such flag, so anyr drops it and says so.
/// Each cell is from that agent's `--help` (claude 2.1.310, codex 0.133.0,
/// grok 1.0.52, opencode 2.0.24, pi 0.73.1) or Poolside's docs (v1.0.16).
type Row = (
    &'static [&'static str],
    [(&'static str, Option<&'static [&'static str]>); 6],
);

const MATRIX: &[Row] = &[
    (
        &["--effort", "high"],
        [
            ("claude", Some(&[])),
            ("codex", Some(&["-c", "model_reasoning_effort=\"high\""])),
            ("grok", Some(&["--reasoning-effort", "high"])),
            ("opencode", None),
            ("pi", Some(&["--thinking", "high"])),
            ("pool", None),
        ],
    ),
    (
        &["--print", "hi"],
        [
            ("claude", Some(&["-p", "hi"])),
            ("codex", Some(&["exec", "hi"])),
            ("grok", Some(&["--single", "hi"])),
            ("opencode", Some(&["run", "hi"])),
            ("pi", Some(&["-p", "hi"])),
            ("pool", None),
        ],
    ),
    (
        &["--continue"],
        [
            ("claude", Some(&["--continue"])),
            ("codex", Some(&["resume", "--last"])),
            ("grok", Some(&["--continue"])),
            ("opencode", Some(&["--continue"])),
            ("pi", Some(&["--continue"])),
            ("pool", None),
        ],
    ),
    (
        &["--resume", "s1"],
        [
            ("claude", Some(&["--resume", "s1"])),
            ("codex", Some(&["resume", "s1"])),
            ("grok", Some(&["--resume", "s1"])),
            ("opencode", Some(&["--session", "s1"])),
            ("pi", Some(&["--session", "s1"])),
            ("pool", Some(&["--resume", "s1"])),
        ],
    ),
    (
        &["--resume"],
        [
            ("claude", Some(&["--resume"])),
            ("codex", Some(&["resume"])),
            ("grok", Some(&["--resume"])),
            ("opencode", None),
            ("pi", Some(&["--resume"])),
            ("pool", Some(&["--resume"])),
        ],
    ),
    (
        &["--system", "be terse"],
        [
            ("claude", Some(&["--system-prompt", "be terse"])),
            ("codex", None),
            ("grok", Some(&["--system-prompt-override", "be terse"])),
            ("opencode", None),
            ("pi", Some(&["--system-prompt", "be terse"])),
            ("pool", None),
        ],
    ),
    (
        &["--append-system", "cite files"],
        [
            ("claude", Some(&["--append-system-prompt", "cite files"])),
            ("codex", None),
            ("grok", Some(&["--rules", "cite files"])),
            ("opencode", None),
            ("pi", Some(&["--append-system-prompt", "cite files"])),
            ("pool", None),
        ],
    ),
    (
        &["--add-dir", "/a", "--add-dir=/b"],
        [
            ("claude", Some(&["--add-dir", "/a", "--add-dir", "/b"])),
            ("codex", Some(&["--add-dir", "/a", "--add-dir", "/b"])),
            ("grok", None),
            ("opencode", None),
            ("pi", None),
            ("pool", None),
        ],
    ),
    (
        &["--cwd", "/w"],
        [
            ("claude", None),
            ("codex", Some(&["--cd", "/w"])),
            ("grok", Some(&["--cwd", "/w"])),
            ("opencode", None),
            ("pi", None),
            ("pool", Some(&["--directory", "/w"])),
        ],
    ),
    (
        &["--max-turns", "3"],
        [
            ("claude", Some(&["--max-turns", "3"])),
            ("codex", None),
            ("grok", Some(&["--max-turns", "3"])),
            ("opencode", None),
            ("pi", None),
            ("pool", None),
        ],
    ),
    (
        &["--no-tools"],
        [
            ("claude", Some(&["--tools="])),
            ("codex", None),
            ("grok", None),
            ("opencode", None),
            ("pi", Some(&["--no-tools"])),
            ("pool", None),
        ],
    ),
    (
        &["--no-skills"],
        [
            ("claude", Some(&["--disable-slash-commands"])),
            ("codex", None),
            ("grok", None),
            ("opencode", None),
            ("pi", Some(&["--no-skills"])),
            ("pool", None),
        ],
    ),
];

#[test]
fn portable_flags_map_to_each_agents_own_argv_or_drop_with_a_note() {
    for (input, cells) in MATRIX {
        let alias = input[0].split('=').next().unwrap();
        for (agent, want) in cells {
            let base = dry_run(agent, &[]).args;
            let got = dry_run(agent, input);
            assert_eq!(got.code, 0, "{agent} {input:?}: {}", got.stderr);
            let mut expected = base.clone();
            match want {
                Some(native) => {
                    expected.extend(strs(native));
                    assert!(
                        !got.stderr.contains("ignoring"),
                        "{agent} {input:?}: {}",
                        got.stderr
                    );
                }
                None => {
                    if input == &["--print", "hi"] {
                        expected.push("hi".into());
                    }
                    assert!(
                        got.stderr.contains(&format!("{agent} "))
                            && got.stderr.contains(&format!("ignoring {alias}")),
                        "{agent} must say {alias} was dropped: {}",
                        got.stderr
                    );
                }
            }
            assert_eq!(got.args, expected, "{agent} {input:?}");
        }
    }
}

#[test]
fn every_cell_with_its_own_spelling_is_not_doubled() {
    for (input, cells) in MATRIX {
        let alias = input[0].split('=').next().unwrap();
        if matches!(alias, "--add-dir" | "--append-system") {
            continue;
        }
        let leftover: &[&str] = if alias == "--print" { &input[1..] } else { &[] };
        for (agent, want) in cells {
            let Some(native) = want else { continue };
            if !native
                .first()
                .is_some_and(|n| n.starts_with('-') && *n != alias)
            {
                continue;
            }
            let mut extra = native.to_vec();
            extra.extend(input.iter());
            let got = dry_run(agent, &extra);
            assert_eq!(got.code, 0, "{agent} {extra:?}: {}", got.stderr);
            let mut expected = dry_run(agent, &[]).args;
            expected.extend(strs(native));
            expected.extend(strs(leftover));
            assert_eq!(got.args, expected, "{agent} {extra:?}");
        }
    }
}

#[test]
fn agents_own_spelling_wins_and_is_not_doubled() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "claude",
            &["--system-prompt", "mine", "--system", "portable"],
            &["--system-prompt", "mine"],
        ),
        ("codex", &["-C", "/a", "--cwd", "/b"], &["-C", "/a"]),
        ("opencode", &["-c", "--continue"], &["-c"]),
        ("pi", &["-p", "--print", "hi"], &["-p", "hi"]),
        (
            "grok",
            &["--reasoning-effort", "low", "--effort", "high"],
            &["--reasoning-effort", "low"],
        ),
    ];
    for (agent, input, want) in cases {
        let mut expected = dry_run(agent, &[]).args;
        expected.extend(strs(want));
        let got = dry_run(agent, input);
        assert_eq!(got.code, 0, "{agent}: {}", got.stderr);
        assert_eq!(got.args, expected, "{agent} {input:?}");
    }
}

#[test]
fn portable_flags_map_in_place_among_other_agent_flags() {
    let got = dry_run(
        "claude",
        &[
            "--frob",
            "--add-dir",
            "/a",
            "-x",
            "--max-turns",
            "2",
            "--quux",
            "hi",
        ],
    );
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert!(
        got.args.ends_with(&strs(&[
            "--frob",
            "--add-dir",
            "/a",
            "-x",
            "--max-turns",
            "2",
            "--quux",
            "hi"
        ])),
        "{:?}",
        got.args
    );
    let codex = dry_run(
        "codex",
        &["--search", "--continue", "--add-dir", "/a", "fix it"],
    );
    assert!(
        codex.args.ends_with(&strs(&[
            "--add-dir",
            "/a",
            "resume",
            "--last",
            "--search",
            "fix it"
        ])),
        "{:?}",
        codex.args
    );
}

#[test]
fn opencode_flags_follow_run_and_codex_nests_exec_resume() {
    let opencode = dry_run("opencode", &["--continue", "--print", "fix it"]);
    assert_eq!(opencode.code, 0, "{}", opencode.stderr);
    assert!(
        opencode
            .args
            .ends_with(&strs(&["run", "--continue", "fix it"])),
        "{:?}",
        opencode.args
    );
    let codex = dry_run("codex", &["--print", "--continue", "fix it"]);
    assert!(
        codex
            .args
            .ends_with(&strs(&["exec", "resume", "--last", "fix it"])),
        "{:?}",
        codex.args
    );
}

#[test]
fn portable_names_after_separator_stay_raw() {
    let base = dry_run("codex", &[]).args;
    let raw = ["--continue", "--system", "x", "--no-skills"];
    let mut extra = vec!["--"];
    extra.extend(raw);
    let got = dry_run("codex", &extra);
    assert_eq!(got.code, 0, "{}", got.stderr);
    let mut want = base;
    want.extend(strs(&raw));
    assert_eq!(got.args, want);
    assert!(!got.stderr.contains("ignoring"), "{}", got.stderr);
}

#[test]
fn portable_value_flag_without_a_value_is_an_error() {
    let got = dry_run("claude", &["--system"]);
    assert_ne!(got.code, 0);
    assert!(
        got.stderr.contains("Flag --system requires a value."),
        "{}",
        got.stderr
    );
}

#[test]
fn launch_help_lists_portable_flags_for_that_agent() {
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .args(["codex", "--help"])
        .env("ANYR_NO_UPDATE", "1")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    for line in [
        "--continue            Continue the most recent session; passes resume --last",
        "--max-turns <n>       No effect: Codex has no turn-limit flag",
        "--cwd <path>          Working directory for the session; passes --cd <path>",
    ] {
        assert!(help.contains(line), "codex help lacks `{line}`:\n{help}");
    }
}

/// A stand-in agent that lists `--search` and rejects anything else, the
/// way a real agent fails on another agent's flag.
#[cfg(unix)]
#[test]
fn launch_failure_names_the_flag_the_agent_does_not_list() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("anyr-hint-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let agent = dir.join("codex");
    std::fs::write(
        &agent,
        "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in --help) echo '  --search  Web search'; exit 0;; --serch) echo \"Error: Unknown option: $a\" >&2; exit 2;; esac; done\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_anyr"))
            .args(["codex", "--yes", "--no-mcp", "--key", KEY, "--command-path"])
            .arg(&agent)
            .args(extra)
            .env("ANYROUTER_HOME", &dir)
            .env("ANYR_NO_UPDATE", "1")
            .env("ANYR_NO_CATALOG", "1")
            .env_remove("ANYROUTER_API_KEY")
            .output()
            .unwrap()
    };
    let bad = run(&["--serch"]);
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert_eq!(bad.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("codex does not list --serch in its --help. Did you mean --search?"),
        "{stderr}"
    );
    let ok = run(&["--search"]);
    assert_eq!(ok.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&ok.stderr).contains("does not list"),
        "no hint on success"
    );
}

/// A stand-in `agent` that prints a `--help` listing only `--search` and then
/// fails on every other call right away, like an auth or network error.
#[cfg(unix)]
fn failing_agent(agent: &str, extra: &[&str]) -> String {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("anyr-fail-{agent}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join(agent);
    std::fs::write(
        &bin,
        "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = --help ] && { echo '  --search  Web search'; exit 0; }; done\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .args([agent, "--yes", "--no-mcp", "--key", KEY, "--command-path"])
        .arg(&bin)
        .args(extra)
        .env("ANYROUTER_HOME", &dir)
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_NO_CATALOG", "1")
        .env_remove("ANYROUTER_API_KEY")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[cfg(unix)]
#[test]
fn launch_failure_never_blames_a_portable_flag_anyr_rewrote() {
    for (agent, extra) in [
        ("codex", vec!["--continue"]),
        ("codex", vec!["--print", "fix it"]),
        ("opencode", vec!["--print", "hi"]),
        ("crush", vec!["--yolo", "--print", "hi"]),
        // Dropped for this agent: never forwarded, so never the cause.
        ("pi", vec!["--add-dir", "/a"]),
    ] {
        let stderr = failing_agent(agent, &extra);
        assert!(
            !stderr.contains("does not list"),
            "{agent} {extra:?}: {stderr}"
        );
    }
    let stderr = failing_agent("codex", &["--continue", "--", "--raw-flag"]);
    assert!(
        stderr.contains("codex does not list --raw-flag in its --help."),
        "a flag after -- is the user's own: {stderr}"
    );
}

#[test]
fn crush_run_drops_yolo_with_a_note() {
    let got = dry_run("crush", &["--yolo", "--print", "hi"]);
    assert_eq!(got.code, 0, "{}", got.stderr);
    assert!(got.args.ends_with(&strs(&["run", "hi"])), "{:?}", got.args);
    assert!(!got.args.iter().any(|a| a == "-y"), "{:?}", got.args);
    assert!(
        got.stderr
            .contains("crush run has no flag to skip permission prompts; ignoring --yolo."),
        "{}",
        got.stderr
    );
    let interactive = dry_run("crush", &["--yolo"]);
    assert!(
        interactive.args.iter().any(|a| a == "-y"),
        "{:?}",
        interactive.args
    );
}

#[test]
fn codex_global_flags_stay_before_resume() {
    let after = dry_run(
        "codex",
        &[
            "--print",
            "--continue",
            "--add-dir",
            "/a",
            "--cwd",
            "/b",
            "fix it",
        ],
    );
    let before = dry_run(
        "codex",
        &[
            "--add-dir",
            "/a",
            "--cwd",
            "/b",
            "--print",
            "--continue",
            "fix it",
        ],
    );
    let want = strs(&[
        "exec",
        "--add-dir",
        "/a",
        "--cd",
        "/b",
        "resume",
        "--last",
        "fix it",
    ]);
    for got in [after, before] {
        assert_eq!(got.code, 0, "{}", got.stderr);
        assert!(got.args.ends_with(&want), "{:?}", got.args);
    }
}

#[test]
fn codex_print_resume_without_an_id_means_last() {
    let got = dry_run("codex", &["--print", "--resume", "--search"]);
    assert!(
        got.args
            .ends_with(&strs(&["exec", "resume", "--last", "--search"])),
        "{:?}",
        got.args
    );
    let with_id = dry_run("codex", &["--print", "--resume", "abc", "go"]);
    assert!(
        with_id
            .args
            .ends_with(&strs(&["exec", "resume", "abc", "go"])),
        "{:?}",
        with_id.args
    );
    let picker = dry_run("codex", &["--resume"]);
    assert!(
        picker.args.ends_with(&strs(&["resume"])),
        "{:?}",
        picker.args
    );
    assert!(
        !picker.args.iter().any(|a| a == "--last"),
        "{:?}",
        picker.args
    );
}
