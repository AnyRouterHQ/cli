use std::cell::RefCell;
use std::path::Path;

use crate::VERSION;

thread_local! {
    static INVOKED_BIN: RefCell<String> = const { RefCell::new(String::new()) };
}

const LAUNCH_HELP_BODY: &str = "\
Launches the coding agent through AnyRouter (signs in first if needed).

Options:
  --yes, --ok           Skip confirmation prompts (login / install)
  --model auto|<id>     Session model. auto / anyrouter/auto; [1m] and [500k] \
are min-context floors on auto (not catalog SKUs).
@claude   --haiku <id>          Claude /model haiku and subagents
@claude   --sonnet <id>         Claude /model sonnet
@claude   --opus <id>           Claude /model opus
@claude   --fable <id>          Claude /model fable (also the auto-fallback target)
  --preset <slug>       Launch with your saved preset (model @preset/<slug>); fails
                        if it does not exist. Cannot be combined with --model
@claude   --hub <slug>          Load a hub: sync ~/.anyrouter/hubs + claude --plugin-dir
  --profile <name>      Use a named profile
  --command-path <path> Explicit path to the agent executable
  --install             Install a missing agent without prompting (implied on a TTY)
  --config <path>       Override the config file path
  --dry-run             Print the child command and env (secrets redacted);
                        with --json, the plan as one JSON object
  --json                anyr's own output as JSON; never sent to the agent
                        (for the agent's own --json, put it after `--`)
  --no-mcp              Don't connect the AnyRouter MCP gateway (or its
                        artifact tools) this launch
                        (`{bin} mcp auto off` turns it off on this machine)
  --device              Force the device-code flow (headless / SSH)
  --device-code         Alias of --device
  --paste               Force the paste-a-key flow, skipping auto-detection

Portable flags (one name on every agent; anyr passes the agent's own flag):
{portable}
Any other flag goes to the agent unchanged and in order, no `--` needed:
  {bin} {id} --model <id> {example}
Everything after `--` goes to the agent raw, including flags anyr also owns.

First run with no config auto-detects the best way to sign in — browser first,
falling back to the device-code flow automatically.
";

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
    let body: String = LAUNCH_HELP_BODY
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
        "auth" => fill(&bin, AUTH),
        "login" => fill(&bin, LOGIN),
        "token" => fill(&bin, TOKEN),
        "switch" => fill(&bin, SWITCH),
        "usage" => fill(&bin, USAGE),
        "decision" => fill(&bin, DECISION),
        "whoami" => fill(&bin, WHOAMI),
        "account" => fill(&bin, ACCOUNT),
        "logs" => fill(&bin, LOGS),
        "generation" => fill(&bin, GENERATION),
        "presets" => fill(&bin, PRESETS),
        "mcp" => fill(&bin, MCP),
        "models" => fill(&bin, MODELS),
        "config" => fill(&bin, CONFIG),
        "chat" => fill(&bin, CHAT),
        "skills" => fill(
            &bin,
            "{bin} skills — not yet in the native CLI (coming later).\n",
        ),
        "relay" => fill(&bin, RELAY),
        "byok" => fill(&bin, BYOK),
        "artifact" => fill(&bin, ARTIFACT),
        "task" => fill(&bin, TASK),
        "delegate" => fill(&bin, DELEGATE),
        "keys" => fill(&bin, KEYS),
        "audit" => fill(
            &bin,
            "{bin} audit — not yet in the native CLI (coming later).\n",
        ),
        "logout" => fill(&bin, LOGOUT),
        "upgrade" => fill(&bin, UPGRADE),
        "transactions" => fill(&bin, TRANSACTIONS),
        "embed" => fill(&bin, EMBED),
        "eval" => fill(&bin, EVAL),
        "menu" => fill(
            &bin,
            "{bin} menu — fullscreen launcher (default on a TTY)\n\nUsage:\n  {bin}                 Same as `{bin} menu` on a TTY\n  {bin} menu [--dump-tui]\n\nAccount, model, credits in a header. Launch agents, configure, quit.\n↑↓ / j k move, ↵ select (signs in and installs if needed), q/esc quit.\nSet ANYR_TUI=0 for inline HUD.\n\n`--dump-tui` / ANYR_TUI_DUMP=1 prints one plain frame and exits.\n",
        ),
        "commands" => commands_help(),
        "schema" => fill(&bin, SCHEMA),
        "api" => crate::api::help(),
        "publish" => crate::publish::publish_help(),
        "apps" => crate::publish::apps_help(),
        "completion" => fill(&bin, COMPLETION),
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
            fill(&bin, POOL),
            launch_help(&bin, "pool", "Poolside")
        ),
        "cursor" | "cline" | "windsurf" => format!(
            "{bin} {canonical} — not a launch target yet.\n\
Print a key with `{bin} auth token` and the base URL with `{bin} onboard impl`.\n"
        ),
        _ => return None,
    })
}

const AUTH: &str = "\
Authenticate with AnyRouter.

Usage:
  {bin} auth <command> [flags]

Commands:
  login:       Log in to an AnyRouter account
  logout:      Log out of an AnyRouter account
  status:      View authentication status
  switch:      Switch the active account
  token:       Print the API key

Options:
  -h, --help   Show help for command
";

const LOGIN: &str = "\
Log in to an AnyRouter account.

Usage:
  {bin} auth login [flags]

Interactive (TTY): opens a login URL with the code already in it (browser
when one is reachable). Falls back to printing that URL over SSH / CI.
Force a route with --device or --paste.

Non-interactive: pipe the key (`echo $KEY | {bin} auth login --key -`) or set
ANYROUTER_API_KEY. A literal `--key sk-...` works but is visible in `ps` and
shell history.

After login, `{bin} claude` starts Claude Code (the default agent).

Options:
  --key -                  Read the API key from stdin (safest non-interactive)
  --key sk-ar-v1-...       AnyRouter API key as an argument (warns: visible in argv)
  --device, --device-code  Force the device-code flow (headless / SSH)
  --paste                  Force the paste-a-key flow

Also available as `{bin} login`.
";

const TOKEN: &str = "\
Print the API key stored for the active account.

Usage:
  {bin} auth token [flags]

Prints the full secret to stdout (for scripts). Use --masked in a log.

Options:
  --masked     Print a redacted key
  --json       Print as JSON
  --profile    Use a named account
";

const SWITCH: &str = "\
Switch the active AnyRouter account.

Usage:
  {bin} auth switch [<account>]

With no account on a TTY, pick from stored accounts.

Also available as `{bin} account use <account>`.
";

const USAGE: &str = "\
{bin} usage — credits remaining, 24h spend, top models

Usage:
  {bin} usage [options]

Options:
  --json            Print as JSON
  --key sk-ar-v1-…  Use this key instead of the saved profile
  --profile <name>  Use a named profile
";

const DECISION: &str = "\
{bin} decision — call the typed Decisions API (not chat)

Usage:
  {bin} decision --model <id> --state <text|json> --questions <json> [flags]
  cat request.json | {bin} decision [flags]

Sends a POST request to `/api/v1/decisions` with `model`, `state`, and a
question object map. Responses contain structured `answers`, not chat text.
Decision models such as `typesafe/jev`, `fastino/gliner2.5-multi-v1`, and
`anyrouter/decision` cannot be launched through the chat-agent commands.

Options:
  --model <id>         Decision model id (required unless stdin supplies it)
  --state <text|json>  Application state; plain text or JSON object/array
  --questions <json>   Non-empty question map (noul / choice / score)
  --stdin              Read and merge a JSON object from stdin
  --json               Print compact JSON (default: pretty JSON)
  --key sk-ar-v1-…     Use this key instead of the saved profile
  --profile <name>     Use a named account/profile
  --base-url <url>     Override the configured API base URL
  --config <path>      Override the config file path

Examples:
  {bin} decision --model typesafe/jev \\
    --state 'Help! My payouts have been failing for 3 days.' \\
    --questions '{\"is_urgent\":{\"type\":\"noul\",\"instructions\":\"Is this urgent?\"}}'

Aliases:
  decisions, systemone
";

const WHOAMI: &str = "\
View authentication status.

Usage:
  {bin} auth status [flags]

Shows who is signed in (name and email), the account, the masked key and
the default model. Agents with their own key or account get one line each:
the masked key they launch with and where it comes from (agent key, agent
account, account, ANYROUTER_API_KEY or --key). In a terminal it then offers
to switch account, change the default model, log out or open the dashboard.

Options:
  --json       Print as JSON: account, user, key_masked, default_model, agents,
               config_path, base_url (identity_error if the lookup failed)
  --profile    Use a named account
  --verbose    Print the full error on stderr when a lookup failed

Also available as `{bin} whoami`.
";

const ACCOUNT: &str = "\
{bin} account — manage multiple accounts

Usage:
  {bin} account list
  {bin} account use <name>
  {bin} account use <name> --agent <agent>
  {bin} account use --agent <agent> --clear
  {bin} account add [--yes]
  {bin} account rename <old> <new>
  {bin} account remove <name>

Options:
  --yes            Skip confirmations
  --agent <id>     Bind this account to a coding agent (claude, codex, opencode,
                   grok, pi, ...); does not change the session default
  --clear          With --agent: drop the binding, so the agent uses the active account

An agent's own key (`{bin} keys set --agent`) wins over its account.
Renaming an account keeps its agents bound; removing one unbinds them.
`{bin} auth status` shows the key each agent launches with.
";

const MODELS: &str = "\
{bin} models — list catalog model ids

Usage:
  {bin} models [list|ls] [options]     (alias: {bin} model)
  {bin} models [--free] [--type chat|embedding|decision] [--owner <org>] [--search <text>] [--sort price|context|new]
  {bin} models show <id> [--json]
  {bin} models use <id>
  {bin} models use <id> --agent <claude|codex|grok|opencode|pi|pool>
  {bin} models use --haiku|--sonnet|--opus|--fable <id>
  {bin} models --pick
  {bin} models --dump-tui

Lists every model id usable with --model. `use` / `--pick` persist the session
default, or Claude Code's opus / sonnet / haiku / fable aliases. `--agent`
pins the launch model for that coding agent only (the id must already be known —
this command does not invent catalog ids).

The inline picker pins `anyrouter/auto` as a selectable preset. Type to
filter catalog ids. It does not dump a most-used / usage ranking.

Filters print id, type, context, $/1M in, $/1M out and free. `free` means an
explicit $0 platform price (BYOK list prices never count). `show` prints pricing,
context, modalities, the free route and the providers a request tries in order.

Options:
  --free            Only $0 models
  --type <t>        chat | embedding | decision
  --owner <org>     Only models from this owner (nvidia, z-ai, ...)
  --search <text>   Match on id or name
  --sort <s>        price (cheapest first) | context (largest first) | new
  --json            Print as JSON
  --pick            Interactive picker (TTY) — default / haiku / sonnet / opus / fable
  --agent <id>     Pin the model on one coding agent instead of the session default
  --dump-tui        Print one picker frame (pins anyrouter/auto) and exit
  --haiku <id>      Persist Claude haiku alias
  --sonnet <id>     Persist Claude sonnet alias
  --opus <id>       Persist Claude opus alias
  --fable <id>      Persist Claude fable alias (auto-fallback target)
  --key sk-ar-v1-…  Optional inference key
";

const CONFIG: &str = "\
Print current settings. Interactive picker only when asked.

Usage:
  {bin} config                 Print account / model / agent / credits
  {bin} config --pick          Open the settings TUI (TTY)
  {bin} config get [--json]    Same dump; `--json` for scripts
  {bin} config path            Print the config file path
  {bin} config use <account>   Switch the active account
  {bin} config get <key>       Print one setting (exit 1 when unset)
  {bin} config set <key> <v>   Save one setting
  {bin} config unset <key>     Clear one setting

Settings:
  launcher.agent               Agent a bare `{bin}` starts (else the last one)
  launcher.<agent>.model       Model that agent launches with
  launcher.<agent>.args        Flags passed to that agent on every launch
  launcher.<agent>.yolo        Always skip that agent's permission prompts
  launcher.claude.<tier>       Claude haiku / sonnet / opus / fable model
  launcher.mcp_auto            false: never wire the MCP gateway in
  update.auto, update.channel  Auto-update on/off; stable or beta

  {bin} config set launcher.codex.model openai/gpt-5.5
  {bin} config set launcher.claude.args -- --verbose

Values that start with `-` go after `--`. `{bin} models --pick` chooses a
model. `--dump-tui` prints one settings frame and exits. Off a terminal the
dump skips the account lookup and credits; `--verbose` adds them.
";

const KEYS: &str = "\
{bin} keys — manage API keys

Usage:
  {bin} keys list [--json]
  {bin} keys create [name] [--limit USD] [--reset daily|weekly|monthly]
                        [--endpoints chat,embeddings,...] [--expires 30d] [--quiet]
                        [--agent <agent>]
  {bin} keys set --agent <agent> -
  {bin} keys unset --agent <agent>
  {bin} keys show <hash> [--json]
  {bin} keys update <hash> [--limit USD] [--reset ...] [--endpoints ...] [--expires 30d]
                           [--disable|--enable]
  {bin} keys audit [--json]
  {bin} keys revoke <last4|id|hash> [--yes]

Scoped keys for CI: `--limit` caps spend in USD, `--reset` sets its window,
`--endpoints` restricts the key to named endpoints (chat covers chat,
messages and responses, which share one grant), `--expires` takes 30d, 12h,
45m or 2w. `--quiet` prints only the secret on stdout and does not save it:
  KEY=$({bin} keys create ci-bot --limit 5 --quiet)
Endpoint names: chat, embeddings, moderations, rerank, documents, speech,
decisions, mcp, me, relay, models, keys, credits, logs, dashboard, trace.

The signed-in API key must have Key Management permission.
CLI login (`{bin} auth login`) creates a key with full access.
`keys audit` lists keys that look unused (never used after 7 days, idle 30
days, or replaced by a newer login on the same device) with the reason. It
never removes anything; revoke each one yourself.

One key per coding agent: `--agent <agent>` (claude, codex, opencode, grok,
pi, ...) gives that agent its own key, so its spend, limits and revocation stay
apart from the rest. `keys create --agent` mints the key in the account that
agent uses and saves it for that agent (with `--quiet` it still saves, and
prints only the secret). It refuses while --key or ANYROUTER_API_KEY is set.
`keys set --agent <agent> -` stores a key you paste on stdin.
`keys unset --agent <agent>` removes it; the key keeps working until you
revoke it. `{bin} auth status` shows which key each agent launches with.

A different key on this machine comes from `{bin} login` (a new key named
`<host> anyr <date>`) or `{bin} login --key -`. After a browser login replaces
this machine's key, the CLI asks before revoking the old one.
";

const LOGOUT: &str = "\
Log out of an AnyRouter account.

Usage:
  {bin} auth logout [flags]

Removes stored keys for the active account (or --profile).

Also available as `{bin} logout`.
";

const UPGRADE: &str = "\
{bin} upgrade — install the latest CLI from GitHub Releases

Usage:
  {bin} update [--check] [--beta|--stable] [--channel stable|beta] [--dry-run] [--json]
  {bin} update --status [--json]   what the background updater last did
  {bin} update --rollback          put the previous build back
  {bin} upgrade ...                (alias)

Bare `{bin} update` / `{bin} update --check` keep the config channel.
Only `--beta` or `--stable` switch the channel and persist it.

Switch channel and update:
  {bin} update --beta     follow GitHub prereleases (persist + install)
  {bin} update --stable   follow latest non-prerelease (persist + install)

Auto-update never makes a command wait. At most once a day a command starts
a background check that downloads the new build beside the binary and
verifies it against the release checksums (published with the build, so
they catch a corrupt download, not a tampered release). The next run swaps it in
and prints `anyr updated A → B`. A running agent keeps its build until you
start it again. A beta build stays on beta; nothing is ever downgraded.
On Windows, if a swap is cut off and `{bin}` is missing, rename `.{bin}.exe.old`
beside it back to `{bin}.exe`.

It stays off in CI, when output is not a terminal, with --json, and inside
agents {bin} launches. An npm install gets the npm command to run instead. Failures are silent; `{bin} update --status` shows them.

  update:
    auto: false          in ~/.anyrouter/config.yaml to turn it off
  channel: beta          follow main (GitHub prereleases)
  ANYR_NO_AUTO_UPDATE=1  turn it off for this process
  ANYR_AUTO_UPDATE=1     allow it when output is not a terminal
  ANYR_CHANNEL=beta      same as channel: in the config file

Channels:
  stable  (default)  latest non-prerelease
  beta               every push to main (GitHub prerelease)

Downloads:
  https://github.com/AnyRouterHQ/cli/releases/download/<tag>/anyr-<os>-<arch>

While installing, a spinner ticks with the from → to versions and channel:

  ⠋ Updating v0.1.11 -> v0.1.99 (stable channel)
  ✔ Updated to v0.1.99

  Run anyr to start using the new version.

--check reports current vs latest without installing.
--fixture <path> / ANYR_RELEASES_JSON skips the network (tests / dry-run).
--channel stable|beta overrides the config file for this run only (does not persist).
--beta / --stable write channel: into the config, then install that channel.
If the newest build fails checksum, is missing, or is corrupt, the next
good release on the same channel is installed instead of aborting.
";

const RELAY: &str = "\
{bin} relay — serve this machine's local models through https://anyrouter.dev

Runs your local models (fm serve on Apple Silicon, Ollama, LM Studio, …)
behind one https://anyrouter.dev endpoint. The CLI dials out; no ports open.

Usage:
  {bin} relay                     status and the next command to run
  {bin} relay start [--detach]    serve; live status at a terminal, Ctrl-C stops
  {bin} relay stop                stop the running relay
  {bin} relay status [--json]     running?, connection, models, requests, tokens, pool
  {bin} relay logs [--follow] [--lines N]   log of the background relay
  {bin} relay models [--json]     local models and which ones are served
  {bin} relay models enable|disable <id>   choose exposed models (saved)
  {bin} relay models allow-all    expose every detected local model again
  {bin} relay models auto [on|off]  re-check local servers every 15s (default on)
  {bin} relay models --devices [--json]     per model: devices, online, pool (all your devices)
  {bin} relay devices [--json]    your paired devices: id, name, state, models, last seen, pool
  {bin} relay devices rename <id> <name>    rename a device
  {bin} relay devices pool <id> on|off      share one device with the pool, or stop
  {bin} relay devices rm <id> --yes         remove a device (needs --yes; a management key
                                  ak_… via ANYROUTER_MANAGEMENT_KEY, or sign in)
  {bin} relay pool [on|off]       show or set pool sharing (saved, updates the device)
  {bin} relay pair [--name <device>]        pair this device (start pairs too)
  {bin} relay rename <name>       rename this device (here and in the cloud)
  {bin} relay unpair [--yes]      remove this device from your account
  {bin} relay doctor [--json]     check macOS, fm serve, Decider, network and auth

On-device models it detects:
  apple/foundation-model              macOS 26+, Apple Intelligence on, `fm serve`
  perplexity/pplx-decider-flash-v1.2  the Decider for Mac app (:8080)
The first start at a terminal asks which to serve and saves the answer.
Start launches a chosen server that is installed but not running (`fm serve`,
or `open -g -a Decider`) and stops it when the relay exits. A server you
started yourself is used as is and left running.

While the relay runs it re-checks the local servers every 15s and re-advertises
models that were pulled or removed, without dropping the connection — so an
`ollama pull`ed model becomes servable without a restart. `relay models auto off`
freezes the list from connect time. The enable|disable whitelist only controls
what the relay advertises (what the cloud and the pool know); a locally served
model is not blocked by it. Join keys (Apple system model) stay advertised while
fm serve is served.

Start options:
  --detach               run in the background (log: `{bin} relay logs`)
  --model <id>           serve this on-device model; repeat for several
  --all                  serve every on-device model, no checklist
  --no-pool              do not donate idle capacity to the shared pool
  --no-launch            do not start `fm serve` or the Decider app; use
                         only servers that are already running
  --pool                 donate idle capacity (the default; see
                         https://anyrouter.dev/pool). Both override
                         the saved `{bin} relay pool on|off`
  --target <url>         local base URL; repeat for several (default:
                         probe fm serve :1976, :8000, Ollama :11434,
                         LM Studio :1234, Perplexity Decider :8080)
  --token <rk_…>         pairing token (else $ANYROUTER_RELAY_TOKEN or the
                         stored relay_token; an sk-ar key also works)
  --url <wss://…>        relay websocket URL
  --name <device>        device name (default: this Mac's name, else a random one;
                         saved after the first start)
                         One config file = one device: machines sharing it (synced
                         dotfiles) or two relays under one HOME replace each other.
  --max-concurrency <n>  concurrent requests advertised to the pool
  --quiet                print errors only
  --verbose              show relay transport diagnostics

Live screen keys: p pool on/off, r rescan local servers, q or Ctrl-C quit.
State lives in the relay/ folder next to your AnyRouter config.
Exit codes: 0 ok, 1 failure (doctor: a check failed), 2 usage error,
130 stopped with Ctrl-C.

Examples:
  {bin} relay start --detach
  {bin} relay start --model apple/foundation-model --no-pool
  {bin} relay status --json
  {bin} relay doctor
";

const CHAT: &str = "\
{bin} chat [prompt] [--model ID] [--system TEXT] [--json | --no-stream]

Ask a model through /v1/chat/completions. With a prompt (or piped stdin) it
answers once; in a terminal with no prompt it opens a REPL (/exit or Ctrl-D).

Input:
  {bin} chat \"Summarise this\"            the prompt as words
  git diff | {bin} chat                     the prompt from stdin
  git diff | {bin} chat \"Review this\" -    `-` puts stdin where it stands

Options:
  --model <id>      Model (default: your default, else anyrouter/auto)
  --system <text>   System prompt
  --no-stream       Wait for the whole answer, then print it
  --json            One JSON object: id, model, content, finish_reason, usage
                    (`id` works with `{bin} generation <id>`)

Exit codes: 0 ok, 1 error, 2 usage, 3 not found, 4 auth, 5 rate limited.
";

const ARTIFACT: &str = "{bin} artifact — publish a page or folder to a shareable URL

Usage:
  {bin} artifact publish <file|dir> [options]   Print the artifact URL
  {bin} artifact ls [--limit N] [--cursor C|--all]  List your artifacts (one page, or all)
  {bin} artifact rm <id>                        Delete one (asks first)
  {bin} artifact visibility <id> <v>            private | unlisted | public
  {bin} artifact visibility <id> --show-author  Show or hide your name on the page
  {bin} artifact visibility <id> --owner-data   Show or hide your usage on the page

A file (html or md) is sent inline. A folder is uploaded file by file;
dotfiles, node_modules and .git are skipped. Limits: 5 MB for a single
file; a folder takes 200 files and 100 MB per publish. The per-file cap
depends on your plan: 10 MB Free, 25 MB Go, 50 MB Pro, and 5 MB for a
page or source file on every plan.

Options:
  --title <text>       Page title
  --visibility <v>     private (default) | unlisted | public
                       private links open only for you; unlisted to share
  --id <id>            Publish a new version of an existing artifact
  --label <text>       Version label (with --id)
  --show-author        Show your profile name on the page (default: hidden)
  --no-show-author     Hide your name again
  --owner-data         Show your usage on the page (capabilities.owner_data).
                       Turning it on needs a signed-in session or an ak_
                       management key; with an API key the server refuses
                       and the dashboard artifact settings are the way on.
                       When the page is unlisted or public, anyone with the
                       link sees your usage.
  --no-owner-data      Hide your usage again
  --limit <n>          ls: rows per page, 1 to 100
  --cursor <c>         ls: continue from a next_cursor
  --all                ls: follow every page (with --json, one merged result)
  --yes                Skip confirmation (public, rm)
  --json               Print the server response instead of the URL

A version conflict (someone published first) exits 2 and prints the
current version. A file edited while it uploads fails with exit 1 and
commits nothing. Exit codes: 0 ok, 1 error, 2 usage or conflict,
3 not found, 4 auth.
";

const LOGS: &str = "{bin} logs — recent requests and failures

Usage:
  {bin} logs                       Latest 20 requests, newest first
  {bin} logs <id>                  One request in detail
  {bin} logs sessions              Requests grouped by session

Options:
  --limit <n>       Rows to fetch, 1 to 100
  --status <s>      ok | error
  --model <id>      Only this model
  --json            Print the raw API body
  --key <key>       Use this API key (or $ANYROUTER_API_KEY, or the saved login)
  --profile <name>  Use another account

Examples:
  {bin} logs --status error --limit 50
  {bin} logs --json --status error | jq '.data | length'
";

const POOL: &str = "{bin} pool — shared-pool health (no key needed)

Usage:
  {bin} pool stats             Providers, keys, donors, requests today, free coverage
  {bin} pool providers         Per provider: keys, donors, recent masked donors
  {bin} pool free              Backends the pool can serve for free right now
  {bin} pool relay             On-device relay pool: online devices and models

Options:
  --json            Print the raw API body (stats prints all three bodies in one object)
  --profile <name>  Use another account
  --base-url <url>  Use another API host

Examples:
  {bin} pool stats
  {bin} pool providers --json | jq '.providers[].key_count'

Exit codes: 0 ok, 1 error, 2 usage, 3 not found, 4 auth, 5 rate limited.
Without a subcommand, `{bin} pool` launches Poolside (below).
";

const PRESETS: &str = "{bin} presets — saved routing presets

Usage:
  {bin} presets [list]             Slug, name, models, routing
  {bin} presets show <slug>        One preset in full
  {bin} presets create <slug> --model a,b [--fallback] [--sort price]
  {bin} presets edit <slug> [--model a,b] [--fallback] [--sort <by>] [--name <n>]
  {bin} presets rm <slug> --yes    Delete a preset
  {bin} claude --preset <slug>     Launch an agent with model @preset/<slug>

--model takes models in order: the first is primary, the rest are the fallback
chain. --fallback also allows falling back to paid models. --sort is one of
price, throughput, latency, exacto. --description <text> sets a description.

Presets need a management key (ak_…), not an API key. Create one at
https://anyrouter.dev/settings/management-keys with read:presets and
write:presets, then `export ANYROUTER_MANAGEMENT_KEY=ak_…` (or pass --key ak_…).
`--preset` at launch checks the preset exists first and starts nothing if not.

Options:
  --json            Print the raw API body
  --yes             Confirm rm in a script
  --profile <name>  Use another account

Examples:
  {bin} presets create coding --model z-ai/glm-4.7,qwen/qwen3-coder --fallback --sort price
  {bin} presets edit coding --model z-ai/glm-4.7,qwen/qwen3-coder,openai/gpt-5
  {bin} claude --preset coding

Exit codes: 0 ok, 1 error, 2 usage, 3 not found, 4 auth, 5 rate limited.
";

const MCP: &str = "{bin} mcp — wire the AnyRouter MCP gateway into an agent

Usage:
  {bin} mcp url                      Print the endpoint (https://anyrouter.dev/api/v1/mcp)
  {bin} mcp add <agent> [--print] [--show-key]
                                     agent: claude, codex, opencode, cursor
  {bin} mcp connections              Upstream MCP connections behind the gateway

add claude runs `claude mcp add --transport http anyrouter <url> --header ...`.
add codex edits ~/.codex/config.toml, add opencode edits
~/.config/opencode/opencode.json, add cursor edits ~/.cursor/mcp.json. Existing
content is kept and a second run changes nothing. --print shows the snippet and
changes nothing.

The snippet reads the key from $ANYROUTER_API_KEY; the key is only printed or
written with --show-key. Running `add claude` without --print stores the key
in Claude Code's own config, because that is where the header lives. An API key
(sk-ar-v1-...) reaches the unscoped tools; use a management key (ak_...) via
--key for key, credit and preset tools.

Options:
  --print           Show the snippet, change nothing
  --show-key        Put the real key in the snippet or file
  --json            Raw API body (connections) or a result object
  --profile <name>  Use another account

Examples:
  {bin} mcp add claude --print
  {bin} mcp add opencode
  {bin} mcp connections --json

Exit codes: 0 ok, 1 error, 2 usage, 3 not found, 4 auth, 5 rate limited.
";

const GENERATION: &str = "{bin} generation — cost, provider and timing of one call

Usage:
  {bin} generation <id>            Model, provider, status, tokens, cost, latency
  {bin} gen <id>                   Same, short alias

The id is the response `id` or the `x-anyrouter-generation-id` header
(`{bin} chat --json` prints it too).

Options:
  --json            Print the raw API body
  --key <key>       Use this API key (or $ANYROUTER_API_KEY, or the saved login)
  --profile <name>  Use another account

Examples:
  {bin} generation gen_abc123
  {bin} gen gen_abc123 --json | jq .total_cost

Exit codes: 0 ok, 1 error, 2 usage, 3 unknown id, 4 auth, 5 rate limited.
";

const TRANSACTIONS: &str = "{bin} transactions — your credit ledger

Usage:
  {bin} transactions                      Newest first, 20 rows
  {bin} transactions --type purchase      Filter by type
  {bin} transactions --limit 100          1 to 100 rows

Columns: date, type, amount (signed, $), balance after (when present), note.

Options:
  --type <type>     purchase | usage | refund | …
  --limit <n>       Rows to fetch, 1 to 100
  --json            Print the raw API body
  --key <key>       Use this API key (or $ANYROUTER_API_KEY, or the saved login)
  --profile <name>  Use another account

Examples:
  {bin} transactions --type purchase
  {bin} transactions --json | jq '.data[0]'
";

const EMBED: &str = "{bin} embed — embeddings from the terminal

Usage:
  {bin} embed \"hello world\"            Default embedding model; dims + first 8 values
  {bin} embed --model <id> \"hello\"
  {bin} embed --file notes.txt           One input per line
  echo hello | {bin} embed -             Read inputs from stdin, one per line
  {bin} embed --json \"a\" \"b\"           Raw OpenAI-shaped response (all inputs, one request)

Options:
  --model <id>       Embedding model (default: first embedding model in the catalog)
  --dimensions <n>   Ask for a shorter vector
  --file <path>      Add one input per line
  --json             Print the full response for piping into a vector store
  --key <key>        Use this API key (or $ANYROUTER_API_KEY, or the saved login)
  --profile <name>   Use another account

Example:
  {bin} embed --json hi | jq '.data[0].embedding | length'
";

const EVAL: &str = "{bin} eval — one prompt across several models, compared

Usage:
  {bin} eval \"<prompt>\" --model a,b,c
  {bin} eval --file prompt.md --model a,b,c --max-tokens 400
  {bin} eval --prompts cases.jsonl --model a,b --json > results.jsonl

Per model: the answer (cut at 600 characters), then a summary table with the
provider that answered, latency, tokens in/out, cost and status. Output order
follows --model, whatever finishes first.

Options:
  --model <a,b,c>     Models to compare (comma separated, required)
  --file <path>       Read the prompt from a file
  --prompts <path>    JSONL file, one {\"prompt\": \"...\"} per line
  --max-tokens <n>    Cap each answer
  --concurrency <n>   Parallel requests, 1 to 32 (default 4)
  --full              Show whole answers
  --json              One JSON line per (prompt, model)
  --key <key>         Use this API key (or $ANYROUTER_API_KEY, or the saved login)
  --profile <name>    Use another account

A failing model is shown with status error, the others still print, and the
exit code is 1. Exit codes: 0 all ok, 1 any request failed, 2 usage.

Example:
  {bin} eval \"Summarize RFC 8628 in 3 bullets\" --model openai/gpt-5-mini,z-ai/glm-4.7
";

const TASK: &str = "{bin} task — plan with one model, implement with another

Usage:
  {bin} task \"<request>\"
  {bin} task \"<request>\" --plan-model <id> --do-model <id>
  {bin} task \"<request>\" --yes --json | jq -r .result

A planning model writes a numbered plan, then an implementation model follows
it. Both answers stream. Before running, the models and their catalog prices
are shown and you confirm; a task makes two model calls.

Options:
  --plan-model <id>   Model that writes the plan (default: profile default model)
  --do-model <id>     Model that follows the plan (default: profile default model)
  --yes               Skip the confirm prompt (required in scripts and CI)
  --json              No streaming; print task, models, plan, result, usage, cost
  --key <key>         Use this API key (or $ANYROUTER_API_KEY, or the saved login)
  --profile <name>    Use another account

Example:
  {bin} task \"summarize RFC 8628 in five bullets\" --yes --json | jq -r .result
";

const DELEGATE: &str = "{bin} delegate — hand a task to another coding agent

Usage:
  {bin} delegate --to <agent> \"<task>\"
  {bin} delegate --to codex \"add unit tests for src/utils/slug.ts\"
  {bin} delegate --to claude \"explain this stack trace\" --model deepseek/deepseek-v3
  {bin} delegate --to opencode \"fix the lint errors in packages/ui\" --yes

Runs the agent's non-interactive mode (claude -p, codex exec, opencode run)
with the same gateway key, base URL and model as `{bin} <agent>`, streams its
output and exits with its exit code. One hop only: a delegated agent cannot
delegate again.

Options:
  --to <agent>        claude, codex or opencode (required)
  --model <id>        Model for the run (default: profile default model)
  --yes               Skip the confirm prompt (required without a terminal)
  --dry-run           Print the command and environment (key redacted)
  --from <agent>      Name the calling agent (passed to the child as a marker)
  --key <key>         Use this API key (or $ANYROUTER_API_KEY, or the saved login)
  --profile <name>    Use another account
";

const BYOK: &str = "{bin} byok — bring your own provider keys

Usage:
  {bin} byok [list]                     Keys with a BYOK or DONATED badge
  {bin} byok add <provider>             Keep the key private to your account
  {bin} byok add <provider> --donate    Share it with the community pool
  {bin} byok --donate <provider>        Same as add --donate
  {bin} byok rm <provider> [--yes]      Delete the saved key (asks on a terminal)
  {bin} byok test <provider>            One real call with the saved key
  echo $KEY | {bin} byok test <provider> --key-stdin
                                        Check a key before saving it
  {bin} byok providers                  Providers that accept BYOK; ready/donated badge

The key is read from $ANYR_BYOK_KEY, else stdin when piped, else a hidden
prompt. It is never accepted as an argument and never printed.
--donate shows the pool terms and asks to confirm; scripts pass --yes.
Without a terminal and without --yes, --donate exits 2 and sends nothing.

Needs a management key (ak_…) with read:byok and write:byok:
  export ANYROUTER_MANAGEMENT_KEY=ak_…

Options:
  --donate          Donate the key to the community pool
  --yes             Accept the pool terms (add --donate) or confirm rm
  --key-stdin       test: read the key to check from stdin
  --json            Print JSON (kind: byok | donated | unknown)
  --profile <name>  Use another account
";

const SCHEMA: &str = "{bin} schema — typed command map for agents (JSON)

Usage:
  {bin} schema                  Every command and subcommand
  {bin} schema <command> [sub]  One entry, e.g. {bin} schema keys create

Always JSON on stdout (schema anyr.schema/v2). Each entry has name, summary,
aliases, subcommands, flags ({name, type: bool|string|int|enum, required,
enum?, env?}), positional args, auth (none|sk|ak), mutating, json and the
exit codes it can return. An unknown command exits 3.
";

const COMPLETION: &str = "{bin} completion — shell completion

Usage:
  {bin} completion <bash|zsh|fish|powershell>

Install:
  bash        {bin} completion bash > ~/.local/share/bash-completion/completions/{bin}
  zsh         {bin} completion zsh > \"${fpath[1]}/_{bin}\"
  fish        {bin} completion fish > ~/.config/fish/completions/{bin}.fish
  powershell  {bin} completion powershell | Out-String | Invoke-Expression

Completes commands, subcommands, flags, `api` resources and verbs,
--effort / --agent values and account names. Offline; never prompts.
";

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
}
