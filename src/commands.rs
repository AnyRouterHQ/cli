use std::collections::{BTreeMap, HashMap};

use crate::help::{command_help, commands_help, resolve_bin, root_help, set_invoked_bin};
use crate::parse::{parse_cli_args, ParsedArgs};
use crate::term;
use crate::VERSION;

use crate::cmd::account::{run_account, run_logout};
use crate::cmd::auth::run_auth;
use crate::cmd::byok::run_byok;
use crate::cmd::config_tui::run_config;
use crate::cmd::decision::run_decision;
use crate::cmd::dispatch::{
    allowed_flags, assert_known_flags, canonical_command, help_topic, known_command,
    should_open_launcher, stub, suggest_command, tui_wants_dump, wants_help, USAGE,
};
use crate::cmd::embed::run_embed;
use crate::cmd::eval::run_eval;
use crate::cmd::generation::run_generation;
use crate::cmd::keys::run_keys;
use crate::cmd::launch::run_launch;
use crate::cmd::login::run_login;
use crate::cmd::logs::run_logs;
use crate::cmd::mcp::run_mcp;
use crate::cmd::menu::run_menu;
use crate::cmd::models::run_models_cli;
use crate::cmd::pool::{is_pool_subcommand, run_pool};
use crate::cmd::presets::run_presets;
use crate::cmd::status::run_whoami;
use crate::cmd::transactions::run_transactions;
use crate::cmd::usage::run_usage;

pub fn run(argv: Vec<String>, env: HashMap<String, String>) -> i32 {
    let env: BTreeMap<String, String> = crate::sort::map(env);
    let code = run_inner(argv, &env);
    // Counts this run locally; a no-op before argument parsing (`--version`,
    // completion, the telemetry child) and while telemetry is off.
    #[cfg(feature = "native")]
    crate::telemetry::on_exit(code, &env);
    code
}

fn run_inner(argv: Vec<String>, env: &BTreeMap<String, String>) -> i32 {
    let raw = if argv.first().map(String::as_str) == Some("--") {
        argv[1..].to_vec()
    } else {
        argv
    };
    // Completion callback runs before parsing: partial words like `--model`
    // with no value must not error, and nothing may hit the network.
    if raw.first().map(String::as_str) == Some("__complete") {
        for line in crate::completion::complete(&raw[1..], env) {
            println!("{line}");
        }
        return 0;
    }
    // Detached daily-telemetry sender (spawned by `telemetry::on_startup`).
    // Reads the payload from stdin, never argv; nothing here may run the
    // startup hooks, or the child would recurse.
    #[cfg(feature = "native")]
    if raw.first().map(String::as_str) == Some("__telemetry-daily") {
        crate::telemetry::send_from_stdin(env);
        return 0;
    }
    #[cfg(not(target_arch = "wasm32"))]
    let argv0 = std::env::args().next();
    #[cfg(target_arch = "wasm32")]
    let argv0: Option<String> = None;
    set_invoked_bin(resolve_bin(
        argv0.as_deref(),
        env.get("ANYR_DISPLAY_BIN").map(String::as_str),
    ));

    crate::help::set_invoked_args(&raw);
    let mut parsed = match parse_cli_args(&raw) {
        Ok(p) => p,
        Err(err) => {
            #[cfg(feature = "native")]
            crate::counters::start(raw.first().map(String::as_str).unwrap_or_default());
            crate::exit::set_json_errors(raw.iter().any(|a| a == "--json") || env_says_json(env));
            return usage_fail(&err);
        }
    };
    let json_mode = parsed.flag_true("json") || env_says_json(env);
    crate::exit::set_json_errors(json_mode);
    if json_mode && !parsed.flags.contains_key("json") {
        parsed
            .flags
            .insert("json".into(), crate::parse::FlagValue::Bool(true));
    }

    let command = parsed.command.as_str();
    if command == "--version" || command == "-v" {
        if json_mode {
            println!(
                "{}",
                serde_json::json!({"version": VERSION, "built": crate::buildinfo::display_time()})
            );
            return 0;
        }
        println!("{VERSION} (built {})", crate::buildinfo::display_time());
        return 0;
    }

    // parse_cli_args maps empty argv to command "help". Check emptiness first
    // so a real terminal gets the TUI launcher, not --help. Dump mode also
    // opens the launcher without a TTY (`ANYR_TUI_DUMP=1`).
    #[cfg(feature = "native")]
    crate::counters::start(if raw.is_empty() { "menu" } else { command });
    if should_open_launcher(&raw, term::is_interactive(), tui_wants_dump(&parsed, env)) {
        #[cfg(feature = "native")]
        crate::autoupdate::on_startup("menu", &parsed, env);
        let empty = ParsedArgs {
            command: "menu".into(),
            flags: parsed.flags.clone(),
            passthrough: Vec::new(),
            raw_from: None,
        };
        return match run_menu(&empty, env) {
            Ok(code) => code,
            Err(err) => fail(&err),
        };
    }

    // Update notices and the background update check stay out of agent and
    // scripted runs (`--json`, `--quiet`, no TTY).
    #[cfg(feature = "native")]
    crate::autoupdate::on_startup(command, &parsed, env);

    if raw.is_empty() || command == "help" || command == "--help" || command == "-h" {
        if let Some(bad) = unknown_help_topic(&parsed.passthrough) {
            return help_topic_fail(&bad);
        }
        let topic = parsed.passthrough.first().map(String::as_str);
        if json_mode && matches!(topic, None | Some("commands" | "help" | "all")) {
            println!("{}", crate::registry::tree_json());
        } else if topic == Some("agents") {
            print!("{}", crate::registry::agents_guide());
        } else if parsed.flag_true("all") || matches!(topic, Some("commands" | "help" | "all")) {
            print!("{}", commands_help());
        } else if let Some(help) = topic_help(&parsed.passthrough) {
            print!("{help}");
        } else {
            print!("{}", root_help());
        }
        return 0;
    }
    if !known_command(command) {
        let bin = crate::help::invoked_bin();
        let hint = match suggest_command(command) {
            Some(near) => format!("did you mean `{bin} {near}`?"),
            None => format!("run `{bin} --help`"),
        };
        return usage_fail(&format!(
            "{} unknown command \"{command}\"\n{} {hint}",
            term::err("error:"),
            term::dim("hint:")
        ));
    }
    if json_mode
        && (wants_help(&parsed)
            || (parsed.passthrough.is_empty()
                && crate::registry::bare_is_help(canonical_command(command))))
    {
        if let Some(entry) = crate::registry::command_entry(command) {
            println!("{entry}");
            return 0;
        }
    }
    if wants_help(&parsed) {
        let topic = help_topic(&parsed);
        if let Some(help) = command_help(&topic).or_else(|| {
            if parsed.command == "auth" {
                command_help("auth")
            } else {
                None
            }
        }) {
            print!("{help}");
            return 0;
        }
    }
    if let Some(allowed) = allowed_flags(command) {
        if let Err(err) = assert_known_flags(command, &parsed.flags, allowed) {
            return usage_fail(&err);
        }
    }
    let canonical = canonical_command(command);
    // Test hook for crash capture: a dev build, or one reporting to this
    // machine, panics here on request, after parsing and the telemetry gate.
    #[cfg(feature = "native")]
    if let Some(v) = env
        .get("ANYR_TEST_PANIC")
        .filter(|v| !v.is_empty() && *v != "0")
    {
        if cfg!(debug_assertions) || crate::telemetry::loopback_endpoint(env) {
            panic!("test panic: {v}");
        }
    }
    let mut parsed = parsed.clone();
    // Login handles `--key` itself (acquire_api_key); `auth login` is the same.
    let own_key_handling = matches!(canonical, "login" | "setup")
        || (canonical == "auth"
            && matches!(
                parsed.passthrough.first().map(String::as_str),
                Some("login" | "setup")
            ));
    if !own_key_handling {
        // JSON mode keeps stderr to error objects, so the `--key` advice is
        // dropped there.
        let mut warn: Box<dyn std::io::Write> = if json_mode {
            Box::new(std::io::sink())
        } else {
            Box::new(std::io::stderr())
        };
        if let Err(err) = crate::key::normalize_key_flag(
            &mut parsed.flags,
            &mut std::io::stdin().lock(),
            &mut warn,
        ) {
            return usage_fail(&err);
        }
    }
    match dispatch(canonical, &parsed, env) {
        Ok(code) => code,
        Err(err) => fail(&err),
    }
}

/// `ANYR_OUTPUT=json` turns on `--json` for every command.
fn env_says_json(env: &BTreeMap<String, String>) -> bool {
    env.get("ANYR_OUTPUT")
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("json"))
}

/// Print a failed command's error once and return its exit code. Tagged
/// errors carry their code (`crate::exit::tag`); a restricted-key message
/// exits 4; anything else exits 1.
fn fail(err: &str) -> i32 {
    let (code, message) = crate::exit::classify(err);
    let code = crate::exit::refine(code, &message);
    crate::exit::print(code, &message);
    code
}

/// The one exit path for usage errors (bad flag, bad subcommand, bad topic).
fn usage_fail(message: &str) -> i32 {
    crate::exit::print(crate::exit::USAGE, message);
    crate::exit::USAGE
}

/// First word of `help <words>` that is not a real topic, if any.
fn unknown_help_topic(words: &[String]) -> Option<String> {
    let first = words.first()?;
    let known =
        |w: &str| matches!(w, "agents" | "all") || known_command(w) || command_help(w).is_some();
    if !known(first) {
        return Some(first.clone());
    }
    if first == "auth" {
        return words.get(1).filter(|w| !known(w)).cloned();
    }
    None
}

/// Help for `help <topic>` / `help auth <sub>`; None falls back to root help.
fn topic_help(words: &[String]) -> Option<String> {
    let first = words.first()?;
    if first == "auth" {
        if let Some(help) = words.get(1).and_then(|sub| command_help(sub)) {
            return Some(help);
        }
    }
    command_help(first)
}

fn help_topic_fail(topic: &str) -> i32 {
    let bin = crate::help::invoked_bin();
    let hint = match suggest_command(topic) {
        Some(near) => format!("did you mean `{bin} help {near}`?"),
        None => format!("run `{bin} --help`"),
    };
    usage_fail(&format!(
        "{} unknown help topic \"{topic}\"\n{} {hint}",
        term::err("error:"),
        term::dim("hint:")
    ))
}

fn dispatch(
    command: &str,
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    match command {
        "auth" => run_auth(parsed, env),
        "login" | "setup" => run_login(parsed, env),
        "logout" => run_logout(parsed, env),
        "models" => run_models_cli(parsed, env),
        "usage" => run_usage(parsed, env),
        "credits" => crate::cmd::usage::run_credits(parsed, env),
        "stats" => crate::cmd::stats::run_stats(parsed, env),
        "whoami" | "status" => run_whoami(parsed, env),
        "config" => run_config(parsed, env),
        "decision" => run_decision(parsed, env),
        "account" => run_account(parsed, env),
        "keys" => run_keys(parsed, env),
        "byok" => run_byok(parsed, env),
        "logs" => run_logs(parsed, env),
        "generation" => run_generation(parsed, env),
        "presets" => run_presets(parsed, env),
        "mcp" => run_mcp(parsed, env),
        "embed" => run_embed(parsed, env),
        "eval" => run_eval(parsed, env),
        "task" => {
            #[cfg(feature = "native")]
            {
                crate::cmd::task::run_task(parsed, env)
            }
            #[cfg(not(feature = "native"))]
            {
                stub("task")
            }
        }
        "delegate" => {
            #[cfg(feature = "native")]
            {
                crate::cmd::delegate::run_delegate(parsed, env)
            }
            #[cfg(not(feature = "native"))]
            {
                stub("delegate")
            }
        }
        "transactions" => run_transactions(parsed, env),
        "menu" => run_menu(parsed, env),
        "commands" => {
            if parsed.flag_true("json") {
                println!("{}", crate::registry::tree_json());
            } else {
                print!("{}", commands_help());
            }
            Ok(0)
        }
        "schema" => {
            // JSON only, so its errors are JSON too.
            crate::exit::set_json_errors(true);
            // The whole map is one compact line (about 100 KB); one entry is
            // pretty, for reading.
            let entry = crate::registry::schema(&parsed.passthrough)?;
            let text = if parsed.passthrough.is_empty() {
                serde_json::to_string(&entry)
            } else {
                serde_json::to_string_pretty(&entry)
            };
            println!("{}", text.unwrap_or_else(|_| "{}".into()));
            Ok(0)
        }
        "report" => {
            #[cfg(feature = "native")]
            {
                crate::cmd::report::run(parsed, env)
            }
            #[cfg(not(feature = "native"))]
            {
                stub("report")
            }
        }
        "telemetry" => {
            #[cfg(feature = "native")]
            {
                crate::cmd::telemetry::run(parsed, env)
            }
            #[cfg(not(feature = "native"))]
            {
                stub("telemetry")
            }
        }
        "pool" if is_pool_subcommand(parsed) => run_pool(parsed, env),
        c if crate::registry::LAUNCHERS.contains(&c) => run_launch(command, parsed, env),
        "chat" => {
            #[cfg(feature = "native")]
            {
                crate::chat::run_chat(parsed, env)
            }
            #[cfg(not(feature = "native"))]
            {
                stub("chat")
            }
        }
        "relay" => {
            #[cfg(feature = "native")]
            {
                crate::relay::run(parsed, env)
            }
            #[cfg(not(feature = "native"))]
            {
                let _ = parsed;
                stub("relay")
            }
        }
        "cursor" | "cline" | "windsurf" => stub(command),
        "upgrade" | "update" => crate::upgrade::run(parsed, env),
        "api" => crate::api::run(parsed, env),
        "publish" => crate::publish::run_publish(parsed, env),
        "apps" => crate::publish::run_apps(parsed, env),
        "artifact" => crate::artifact::run(parsed, env),
        "completion" => run_completion(parsed),
        "onboard" | "impl" | "plan" | "fix" | "deploy" | "cp" => {
            crate::onboard::run(command, parsed)
        }
        _ => stub(command),
    }
}

fn run_completion(parsed: &ParsedArgs) -> Result<i32, String> {
    let bin = crate::help::invoked_bin();
    let shell = parsed.passthrough.first().map(String::as_str).unwrap_or("");
    match crate::completion::script(shell, &bin) {
        Some(script) => {
            print!("{script}");
            Ok(0)
        }
        None => Err(format!(
            "{USAGE}Usage: {bin} completion <{}>",
            crate::completion::SHELLS.join("|")
        )),
    }
}
