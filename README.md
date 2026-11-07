# anyr

Point any coding agent at AnyRouter. One key, every provider.

```bash
curl -fsSL https://anyrouter.dev/setup.sh | bash
```

`anyr` is the command line for [AnyRouter](https://anyrouter.dev). It is one
native binary, so there is no runtime to install. It signs you in, then starts
Claude Code, Codex, Grok Build, OpenCode and other coding agents with your key
and model already wired to the gateway. It also chats with models, manages keys
and presets, publishes pages, and serves your local models. This line is
**0.1.x**.

[![CI](https://github.com/AnyRouterHQ/cli/actions/workflows/ci.yml/badge.svg)](https://github.com/AnyRouterHQ/cli/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/AnyRouterHQ/cli/branch/main/graph/badge.svg)](https://codecov.io/gh/AnyRouterHQ/cli)

## Features

- **Launchers**: `anyr claude`, `anyr codex` and ten more start a coding agent through the gateway. The key reaches the agent through its environment, never argv or a file ([Launchers](#launchers)).
- **Chat and models**: `anyr chat` asks a model from the terminal, a pipe or a REPL. `anyr models` browses the catalog and sets your default.
- **Scripts first**: `--json` works wherever a command has a JSON form, errors become one JSON object on stderr, and exit codes are fixed. Without a TTY there are no prompts, spinners or update notices ([For agents and scripts](#for-agents-and-scripts)).
- **Keys, presets and logs**: scoped keys for CI with a spend cap, saved routing presets, request logs and the credit ledger.
- **Publish**: `anyr artifact publish` turns a file or folder into a shareable URL.
- **Relay and pool**: `anyr relay` serves your local models through AnyRouter, and `anyr pool` shows the shared pool.
- **Plain terminal output**: text from the top left and thin lines. Color is off when stdout is not a terminal or `NO_COLOR=1` is set.

## Install

```bash
curl -fsSL https://anyrouter.dev/setup.sh | bash
```

The installer downloads `anyr-<os>-<arch>` from
[GitHub Releases](https://github.com/AnyRouterHQ/cli/releases) into
`~/.local/bin`. Add that directory to `PATH` if the installer says so. CI builds
linux x86_64/arm64, macOS x86_64/arm64, Windows x86_64 and wasm32.

The same script is served from this repo, and `--channel beta` installs the
prerelease that every push to `main` publishes:

```bash
curl -fsSL https://raw.githubusercontent.com/AnyRouterHQ/cli/main/setup.sh | bash
curl -fsSL https://raw.githubusercontent.com/AnyRouterHQ/cli/main/setup.sh | bash -s -- --channel beta
```

Use `setup.sh` instead of a hand-built `releases/latest/download/...` URL. When
the latest non-prerelease has no assets (v0.1.11 shipped empty) that URL 404s,
and `setup.sh` falls back to a release that has `anyr-linux-x86_64`. Direct tag
URLs look like
`https://github.com/AnyRouterHQ/cli/releases/download/<tag>/anyr-linux-x86_64`.
More at [anyrouter.dev/cli](https://anyrouter.dev/cli).

### npm / npx

npm `@anyr/cli@latest` is still the previous JS CLI. The native line stays
**0.1.x** and publishes with `--tag next` until a human retags.

```bash
npx @anyr/cli@next --help
npx @anyr/cli@next login
npm install -g @anyr/cli@next
```

The npm package is a thin Node wrapper. It execs a shipped binary in
`binaries/` when present, otherwise it downloads the matching GitHub Release
asset (`anyr-linux-x86_64`, `anyr-linux-arm64`, `anyr-darwin-x86_64`, `anyr-darwin-arm64`, `anyr-windows-x86_64.exe`).

### Upgrade

```bash
anyr update                 # install latest for your current channel
anyr update --beta          # switch to beta (prereleases) and update
anyr update --stable        # switch to stable and update

anyr upgrade --check
anyr upgrade --check --channel beta   # one-shot, does not persist
```

You can also re-run the installer or bump the npm `next` tag. Auto-update
follows `update.auto` and `update.channel` in `anyr config`, and `--quiet` or
`ANYR_QUIET=1` turns off update notices and the background update.

## Quick start

Sign in once. The key is saved on this machine.

```bash
anyr login            # browser, device code or paste
anyr login --device   # headless / SSH
```

Then launch an agent through the gateway:

```bash
anyr claude           # asks before it logs in or installs anything
anyr claude --ok      # same, skipping the confirmation prompts
anyr codex --ok       # also: grok, opencode, pi, pool, qwen, goose, aider, crush, kilo, fx
```

With no command on a terminal, `anyr` opens a launcher that confirms account,
model and agent, then starts it. To send one request without an agent, use
`anyr chat`. Add `--json` when a script reads the result:

```bash
anyr chat "Reply with ok"
git diff | anyr chat "Review this" -
anyr chat "Reply with ok" --json
```

More first steps:

```bash
anyr --help                 # the everyday commands; anyr help --all for every command
anyr usage                  # credit balance
anyr models use <id>        # persist default model
anyr keys audit             # keys that look unused, with the reason
anyr account use work       # switch saved profiles
anyr claude --install --ok  # install Claude Code first if missing
anyr onboard impl           # paste-ready prompt for a coding agent
anyr plan                   # migration plan prompt (no code changes)
anyr fix                    # repair LLM wiring through AnyRouter
anyr decision --model typesafe/jev \
  --state 'Help! My payouts have been failing for 3 days.' \
  --questions '{"is_urgent":{"type":"noul","instructions":"Does this convey urgency?"}}'
```

## Commands

`anyr help --all` prints the full map, and `anyr <command> --help` the detail
for one command.

| Group | Commands |
|-------|----------|
| Sign in | `login`, `logout`, `whoami`, `account`, `auth` |
| Chat and models | `chat`, `models`, `embed`, `eval`, `task`, `delegate`, `decision` |
| Launch | `claude`, `codex`, `grok`, `opencode`, `pi`, `pool`, and more ([Launchers](#launchers)) |
| Relay | `relay` |
| Publish | `publish`, `apps`, `artifact` |
| Account | `api`, `usage`, `logs`, `generation`, `presets`, `mcp`, `transactions`, `keys`, `byok` |
| Setup and help | `config`, `menu`, `onboard`, `upgrade`, `completion`, `commands`, `help` |

### For agents and scripts

`--json` (or `ANYR_OUTPUT=json`) works on every command: JSON on stdout where
the command has a JSON form, and every error as one JSON object on stderr.
Without a TTY there are no prompts, spinners or update notices.

```bash
anyr commands --json                     # the whole command tree
anyr chat "Reply with ok" --json         # {"id","model","content","finish_reason","usage"}
git diff | anyr chat "Review this" - --no-stream
anyr help agents                         # the guide, from docs/agents.md
```

Exit codes: 0 ok, 1 error, 2 usage, 3 not found, 4 auth, 5 rate limited.

## Command reference

### Dashboard from the terminal: `anyr api`

```bash
anyr api me                          # profile and balance
anyr api keys list --json
anyr api keys create ci-bot          # secret on stdout only: KEY=$(anyr api keys create x)
anyr api GET /credits                # raw request, like `gh api`
anyr api PATCH /keys/<hash> disabled=true limit:=25
anyr api POST /apps/explain/submissions --data @body.json     # body from a file
cat body.json | anyr api POST /apps/explain/submissions --data - -H 'X-Trace: 1'
```

Resources: `me credits keys logs presets aliases models providers dashboard hubs connections`.
Run `anyr api --help` for verbs. Piped output is TSV; pass `--json` for JSON.
Errors exit 2 (usage), 3 (404) or 4 (401/403).

### Publish to an app: `anyr publish`

Apps publish their own JSON Schema; the CLI knows no app by name. `publish` checks
your document against that schema locally (problems listed by field, exit 2, nothing
uploaded), then sends it and prints the URL.

```bash
anyr apps schema explain > schema.json        # what an agent must produce
<agent produces explain json> | anyr publish explain -
anyr publish explain fig.json --dry-run       # server checks it; nothing uploaded
anyr publish explain fig.json --visibility public --yes
anyr apps ls explain                          # what you published
anyr apps visibility explain <id> private
anyr apps rm explain <id> --yes
```

Visibility is `unlisted` by default (link works, not listed), `private` (only you) or
`public` (listed after review). Re-running the same document is safe: it carries a
stable `Idempotency-Key`, so the server returns the original instead of a copy.

### Publish a page: `anyr artifact`

```bash
anyr artifact publish report.html --title "Q3 report"   # prints the URL
anyr artifact publish ./site --visibility link          # folder upload
anyr artifact publish report.html --id <id> --label v2  # new version
anyr artifact ls                     # first page
anyr artifact ls --cursor <c>        # next page (cursor printed to stderr)
anyr artifact ls --all --json        # every page, one merged result
anyr artifact visibility <id> public --yes
anyr artifact rm <id> --yes
```

A file is sent inline; a folder uploads file by file and skips dotfiles,
`node_modules` and `.git` (limits: 200 files and 100 MB per publish, 25 MB per asset, 5 MB per page or source file, 5 MB for a single file). `--id`
builds on the current version; if someone published first the command exits 2
and prints the current version. `public` asks for confirmation unless `--yes`.

### Scoped keys for CI: `anyr keys`

```bash
anyr keys create ci-bot --limit 25 --reset monthly --endpoints chat,embeddings --expires 30d
KEY=$(anyr keys create ci-bot --limit 5 --quiet)   # secret only on stdout, not saved
anyr keys update <hash> --limit 50 --disable       # --enable turns it back on
anyr keys show <hash>                              # limit, spent, endpoints, expiry
```

`--limit` is a USD cap, `--reset` its window (daily, weekly, monthly),
`--expires` takes `30d`, `12h`, `45m` or `2w`. `--endpoints` takes names (chat,
embeddings, moderations, rerank, documents, speech, decisions, mcp, me, relay,
models, keys, credits, logs, dashboard, trace). `chat`, `messages` and
`responses` share one grant on the server, so each covers all three.

### Request logs: `anyr logs`

```bash
anyr logs                            # latest 20 requests, newest first
anyr logs --status error --limit 50  # ok | error, limit 1..100
anyr logs --model z-ai/glm-4.7
anyr logs <id>                       # one request in detail
anyr logs sessions                   # grouped by session
anyr logs --json --status error | jq '.data | length'
```

Columns: time (local), model, provider, status (error code inline), tokens in
and out, cost, latency. `--json` prints the raw API body. A key without the
needed scope exits 4 and names the scope.

### Shared pool: `anyr pool`

```bash
anyr pool stats                      # providers, keys, donors, requests today, free coverage
anyr pool providers                  # per provider: keys, donors, recent masked donors
anyr pool free                       # backends the pool can serve for free right now
anyr pool relay                      # on-device relay pool: online devices and models
anyr pool providers --json | jq '.providers[].key_count'
```

All four read public endpoints, so no key is needed (one is sent if you have it).
`--json` prints the raw body; `stats --json` prints the three bodies under
`stats`, `providers` and `free_coverage`. `anyr pool` with no subcommand still
launches Poolside. Per-key status and last success are not exposed by the API.

### Saved routing presets: `anyr presets`

```sh
anyr presets                                   # slug, name, models, routing
anyr presets show coding
anyr presets create coding --model a/one,b/two --fallback --sort price
anyr presets edit coding --model a/one,b/two,c/three
anyr presets rm coding --yes
anyr claude --preset coding                    # launch with model @preset/coding
```

`--model` lists models in order (first is primary, the rest are the fallback chain); `--fallback` also allows paid fallback; `--sort` is `price`, `throughput`, `latency` or `exacto`. Presets need a management key (`ak_…`, scopes `read:presets` and `write:presets`) from <https://anyrouter.dev/settings/management-keys>: set `ANYROUTER_MANAGEMENT_KEY` or pass `--key ak_…`. `--preset` on a launch command checks the preset exists first and starts nothing if it does not; it cannot be combined with `--model`.

### MCP gateway: `anyr mcp`

```sh
anyr mcp url                 # https://anyrouter.dev/api/v1/mcp
anyr mcp add claude          # runs: claude mcp add --transport http anyrouter <url> --header ...
anyr mcp add codex           # edits ~/.codex/config.toml
anyr mcp add opencode        # edits ~/.config/opencode/opencode.json
anyr mcp add cursor          # edits ~/.cursor/mcp.json
anyr mcp add claude --print  # show the command, change nothing
anyr mcp connections         # upstream MCP connections (GET /v1/connections)
```

Config files keep their existing entries (JSON keys may be re-sorted), and a second run changes nothing. A commented `opencode.jsonc` or a config that is not plain JSON is left alone with an error; use `--print` and paste. The snippet and the written config read the key from `$ANYROUTER_API_KEY`; the key itself is only printed or written with `--show-key`. `add claude` without `--print` runs the `claude` binary (set `ANYROUTER_CLAUDE_PATH` to override) and stores the key in Claude Code's own config. An API key reaches the unscoped tools; pass a management key (`--key ak_…`) for key, credit and preset tools.

### One call: `anyr generation <id>`

```bash
anyr generation gen_abc123           # alias: anyr gen
anyr gen gen_abc123 --json | jq .total_cost
```

Shows model, provider, status, tokens, cost, latency, cache hit and time for
the response `id` (or `x-anyrouter-generation-id`). An unknown id prints the server message and exits 1.

### Plan, then implement: `anyr task`

```bash
anyr task "write a script that dedupes my downloads folder"
anyr task "refactor this CSV parser to stream" --plan-model openai/gpt-5.2 --do-model z-ai/glm-4.7-flash
anyr task "summarize RFC 8628 in five bullets" --yes --json | jq -r .result
```

A planning model writes a numbered plan, then an implementation model follows
it (two chat completions, plan text in the second prompt). Both stream. The
models and catalog prices are shown first and you confirm; scripts and CI must
pass `--yes`. `--json` skips streaming and prints `task`, `plan_model`,
`do_model`, `plan`, `result`, `usage` and `cost`.

### Hand a task to another agent: `anyr delegate`

```bash
anyr delegate --to codex "add unit tests for src/utils/slug.ts"
anyr delegate --to claude "explain this stack trace" --model deepseek/deepseek-v3
anyr delegate --to opencode "fix the lint errors in packages/ui" --yes
```

Runs the agent's non-interactive mode (`claude -p`, `codex exec`, `opencode run`)
with your AnyRouter key, base URL and model, streams its output and exits with
its exit code. You confirm first; scripts pass `--yes`. `--dry-run` prints the
command with the key redacted. One hop only: a delegated agent cannot delegate.

### Compare models: `anyr eval`

```bash
anyr eval "Summarize RFC 8628 in 3 bullets" --model openai/gpt-5-mini,z-ai/glm-4.7
anyr eval --file prompt.md --model a,b,c --max-tokens 400
anyr eval --prompts cases.jsonl --model a,b --json > results.jsonl   # {"prompt":...} per line
```

Sends one non-streaming chat completion per model (`--concurrency`, default 4)
and prints each answer (cut at 600 characters, `--full` for all), then a summary
table: model, provider that answered, latency, tokens in/out, cost, status.
`--json` prints one line per (prompt, model). Output follows `--model` order.
If a model fails it shows `error` and the others still print; the exit code is 1.

### Credit ledger: `anyr transactions`

```bash
anyr transactions                    # newest first, 20 rows
anyr transactions --type purchase
anyr transactions --limit 100
anyr transactions --json | jq '.data[0]'
```

Columns: date, type, amount (signed, $), balance after (when present), note.

### Embeddings: `anyr embed`

```bash
anyr embed "hello world"                         # default embedding model: dims + first 8 values
anyr embed --model baai/bge-base-en-v1.5 "hello"
anyr embed --file notes.txt                      # one input per line, one request
echo "hello" | anyr embed -                      # stdin
anyr embed --dimensions 256 "hello"
anyr embed --json hi | jq '.data[0].embedding | length'
```

Prints model, input count, dimensions, tokens, cost and a truncated vector per
input. `--json` prints the full response body.

### Bring your own provider keys: `anyr byok`

```bash
anyr byok                            # keys with a BYOK or DONATED badge
ANYR_BYOK_KEY=sk-... anyr byok add openai
anyr byok test openai                # one real call with the saved key
echo $KEY | anyr byok test openai --key-stdin   # check a key before saving it
anyr byok rm openai --yes            # delete the saved key (asks on a terminal)
anyr byok providers                  # providers that accept BYOK, ready/donated badge
```

`rm` without `--yes` and without a terminal exits 2. `test` exits 1 when the
provider rejects the key; the provider key is redacted from any error text.

Needs a management key (`ANYROUTER_MANAGEMENT_KEY=ak_...`) with `read:byok` and
`write:byok`. The provider key is read from `$ANYR_BYOK_KEY`, stdin or a hidden
prompt, never from argv.

`--donate` shares the key with the community pool. Caveat: server support is
still pending ([duyet/anyrouter#4220](https://github.com/duyet/anyrouter/issues/4220)),
so treat `--donate` as not yet effective.

### Serve this Mac's models: `anyr relay`

```bash
anyr relay                                  # where you stand + the next command
anyr relay start                            # serve, with a live status screen
anyr relay start --detach                   # serve in the background
anyr relay status [--json]                  # running?, connection, models, requests, tokens, pool
anyr relay logs --follow                    # background log
anyr relay stop                             # graceful stop (same as Ctrl-C)
anyr relay models [--json]                  # local models and which are served
anyr relay models disable apple/foundation-model
anyr relay devices [--json]                 # your paired devices: state, models, last seen, pool
anyr relay devices rename <id> "Studio Mac"
anyr relay devices pool <id> off            # stop sharing one device with the pool
anyr relay devices rm <id> --yes            # revoke a device (management key or session)
anyr relay models --devices                 # per model: devices, online, pool-shared
anyr relay pool off                         # stop donating idle capacity (saved)
anyr relay doctor                           # macOS, fm serve, Decider, network, auth
anyr relay unpair --yes                     # remove this device from your account
```

The relay detects the on-device models this Mac can serve:

- `apple/foundation-model`: macOS 26+ with Apple Intelligence on, via `fm serve`.
- `perplexity/pplx-decider-flash-v1.2`: the Decider for Mac app (port 8080).

The first `start` at a terminal opens a checklist (space toggles, enter
starts) and saves the answer; change it later with `anyr relay models
enable|disable <id>`. `--model <id>` (repeatable) and `--all` override the saved
choice for one run. Other local servers (Ollama, LM Studio, `--target <url>`)
are always served.

You don't need a second terminal. When a chosen model's server is installed
but not running, `start` launches it: `fm serve` as a child process, or the
Decider app with `open -g -a Decider`. It waits for each to answer, restarts
one that crashes (backoff 1s, 2s, 4s … up to 60s), and stops it when the relay
exits. The status screen marks these `(managed)`. A server you started
yourself shows as `(external)`; the relay never starts a second copy and
leaves it running on exit. `--no-launch` uses only servers already running,
and an explicit `--target` never launches anything.

When a model cannot be served, `start`, `relay models` and `relay doctor` say
which case it is and the next step: not installed (install the
Decider for Mac app into /Applications), installed but stopped (`start`
launches it, or open the app), not a Mac, or a Mac on a macOS that is too old (update it). The other models keep serving in
every case. `relay doctor --json` adds a `models` array with each model's
`state` (`running`, `stopped`, `not_installed`, `needs_mac`, `needs_newer_macos`) and `fix`.

At a terminal, `start` shows one live screen: connection, models, requests,
tokens and pool, with keys `p` (pool on/off), `r` (rescan local servers) and
`q` / Ctrl-C (stop). Piped, with `--quiet`, or with `TERM=dumb` it prints plain
log lines instead. `NO_COLOR` turns color off.

`--detach` runs the relay in the background on macOS and Linux. Its pidfile,
state and log live in `relay/` next to your AnyRouter config
(`~/.anyrouter/relay/`, or `$ANYROUTER_HOME/relay/`). `status`, `stop` and
`logs` work from any terminal, for background and foreground runs alike.

Pool sharing is on by default: when your own requests leave the device idle,
it serves other users' requests and earns you credits. `anyr relay pool off`
saves the opt-out and updates the paired device; `--pool` / `--no-pool`
override it for one run. See https://anyrouter.dev/pool and manage devices at
https://dash.anyrouter.dev/devices.

Exit codes: 0 ok, 1 failure (for `doctor`, a failed check), 2 usage error, 130
after Ctrl-C.

### Models: `anyr model list`

```bash
anyr model list                      # catalog ids usable with --model (alias of `anyr models`)
anyr model list --json
anyr model use <id>                  # persist the default model
anyr models --free                   # $0 models only (explicit $0 platform price)
anyr models --type embedding         # chat | embedding | decision
anyr models --owner nvidia --sort price   # sort: price | context | new
anyr models --search glm
anyr models show z-ai/glm-4.7        # pricing, context, modalities, providers, free route
anyr models show z-ai/glm-4.7 --json
```

Filtered lists print id, type, context, $/1M in, $/1M out and free; `--json`
prints the filtered list. `show` with an unknown id exits 1.

### Shell completion

```bash
anyr completion bash > ~/.local/share/bash-completion/completions/anyr
anyr completion zsh > "${fpath[1]}/_anyr"
anyr completion fish > ~/.config/fish/completions/anyr.fish
```

### Structured decisions

Decision models use AnyRouter's typed `POST /api/v1/decisions` endpoint, not a
chat completion. Pass the fields as flags, or pipe a JSON object containing
`model`, `state`, and `questions`:

```bash
anyr decision --model typesafe/jev \
  --state 'Help! My payouts have been failing for 3 days.' \
  --questions '{"is_urgent":{"type":"noul","instructions":"Does this convey urgency?"}}'

echo '{"model":"typesafe/jev","state":"A payout is failing","questions":{"urgent":{"type":"noul"}}}' \
  | anyr decision
```

The command uses the active AnyRouter profile (or `--key`, `--profile`, and
`--base-url`) and prints structured answers. `anyr claude`, `codex`, and other
chat-agent launchers refuse known Decisions models; use `anyr decision` for
`typesafe/jev`, `fastino/gliner2.5-multi-v1`, and `anyrouter/decision`.

On a terminal, bare `anyr` / `anyr menu` opens the Ratatui launcher by default.
Pipes and CI still get `--help` (or `anyr menu --dump-tui` for a plain frame).

Release size is tracked on every PR (`scripts/bench.py`): stripped
`anyr-linux-x86_64` must stay ≤ 4.0 MiB. Locally:

```bash
cargo build --release
strip -s target/release/anyr 2>/dev/null || true
python scripts/bench.py measure --bin target/release/anyr \
  --asset anyr-linux-x86_64 --kind native --out /tmp/anyr-bench.json
```

Interactive `menu` / `config` draw plain text from the top left with thin lines,
no frame and no centering. `anyr menu --dump-tui` prints one ANSI-free frame for CI.

## Launchers

`anyr <agent>` starts a coding agent pointed at AnyRouter with your key and
model. The key goes to the agent through the environment only, never argv or
a file. `--dry-run` prints the command and environment (key masked), and
`--install` installs a missing agent first.

| Agent | Command | How anyr points it at AnyRouter | MCP auto-connect |
|-------|---------|---------------------------------|------------------|
| Claude Code | `anyr claude` | `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`, model slots | yes |
| Codex | `anyr codex` | `-c model_providers.anyrouter.*`, key in `OPENAI_API_KEY` | yes |
| Grok Build | `anyr grok` | private `GROK_HOME` plus an auth provider command | yes |
| OpenCode | `anyr opencode` | inline `OPENCODE_CONFIG_CONTENT` | yes |
| Pi | `anyr pi` | private `PI_CODING_AGENT_DIR` with `models.json` | no (Pi has no MCP) |
| Poolside | `anyr pool` | `POOLSIDE_STANDALONE_*` | no |
| Qwen Code | `anyr qwen` | `OPENAI_BASE_URL`, `OPENAI_API_KEY`, `-m <model>` | no (settings.json only) |
| Goose | `anyr goose` | `GOOSE_PROVIDER=openai`, `OPENAI_HOST`, `OPENAI_BASE_PATH`, `GOOSE_MODEL` | no (config.yaml only) |
| Aider | `anyr aider` | `AIDER_OPENAI_API_BASE`, `AIDER_OPENAI_API_KEY`, `AIDER_MODEL=openai/<model>` | no (Aider has no MCP) |
| Crush | `anyr crush` | private `CRUSH_GLOBAL_CONFIG` with your `crush.json` and `crushrc` copied in | yes |
| Kilo Code | `anyr kilo` | inline `KILO_CONFIG_CONTENT` | yes |
| fx | `anyr fx` | `providers.anyrouter` in `~/.fx/settings.json`, selected with `FX_PROVIDER`, `FX_MODEL` | no (`~/.fx/mcp.json` only) |

**MCP auto-connect.** On every agent marked "yes", anyr connects one MCP server
named `anyrouter` with the key the launch uses (read from `$ANYROUTER_API_KEY`,
never written to your config files). When **Auto-connect in coding agents** is
on in the dashboard it is the whole gateway; otherwise it is the artifact tools
alone (`/api/v1/mcp?tools=artifacts`: `publish_artifact`, `list_artifacts`,
`get_artifact`, `delete_artifact`, `set_artifact_visibility`), so the agent can
publish a page and hand you a link with no setup. Nothing is connected when the
workspace owner turns auto-connect off, with `--no-mcp`, or after
`anyr mcp auto off`. Agents marked "no" print one line saying so on an
interactive launch, and nothing in scripts.

The AIDER_* names outrank a `.aider.conf.yml`, and Qwen Code gets `-m` because
a `model.name` in its settings.json beats `OPENAI_MODEL`. Your own agent config
files are read, never written, with one exception: fx only takes a custom
endpoint from `~/.fx/settings.json` and has no variable to move that file, so
`anyr fx` sets one `providers.anyrouter` entry there (no key; it reads
`$ANYROUTER_API_KEY`). Your selected provider and every other key stay as they
are, and `--dry-run` only says what it would change. `anyr fx` needs fx 0.0.11
or newer, the first release with custom connections.

## Passing flags to the agent

On the launchers (`anyr help --all` lists them under LAUNCH) you do
not need `--`. anyr takes its own flags (`--model`, `--key`, `--dry-run`,
`--yolo`, `--help`, ...) and forwards every other flag to the agent in order:

```bash
anyr codex exec --search "fix the tests" # --search reaches codex
anyr claude --quiet -p "hi"              # --quiet reaches claude
anyr codex exec "fix it" -- --json       # the agent's own --json goes after `--`
anyr claude -- --model raw               # after `--`, everything is raw
```

`--help` and `--json` are always anyr's. `--json` sets anyr's own output to
JSON and never reaches the agent; to give the agent its own `--json`, put it
after `--`. `--quiet` belongs to the agent; anyr's quiet mode on a launcher is
`ANYR_QUIET=1`.
Other commands (`anyr models --bogus`) still reject unknown flags with exit 2.

Portable flags have one name on every launcher. anyr swaps each for the
agent's own flag, in place, so `anyr codex --print --continue "fix it"` runs
`codex exec resume --last "fix it"`. If you already pass the agent's own flag,
anyr does not add it again. `anyr <agent> --help` lists what each one becomes
for that agent. This table is generated from `src/cmd/portable.rs`, and a test
keeps it in sync:

<!-- portable-flags:start -->
| Flag | claude | codex | grok | opencode | pi | pool | qwen | goose | aider | crush | kilo | fx |
|------|---|---|---|---|---|---|---|---|---|---|---|---|
| `--yolo` | `--dangerously-skip-permissions` | `--dangerously-bypass-approvals-and-sandbox` | `--always-approve` | `--auto` | no | `--unsafe-auto-allow` * | `--approval-mode yolo` | env `GOOSE_MODE=auto` | `--yes-always` | `-y` | `--auto` | env `FX_PERMISSION_MODE=full-access` |
| `--effort <level>` | env `MAX_THINKING_TOKENS` | `-c model_reasoning_effort="<level>"` | `--reasoning-effort <level>` | no | `--thinking <level>` | no | no | no | `--reasoning-effort <level>` | no | no | `--effort <level>` |
| `--print` | `-p` | `exec` | `--single` | `run` | `-p` | no | `-p` | no | `--message` | `run` | `run` | `ask` |
| `--continue` | `--continue` | `resume --last` | `--continue` | `--continue` | `--continue` | no | `--continue` | no | `--restore-chat-history` | `--continue` | `--continue` | `--continue` |
| `--resume [<id>]` | `--resume <id>` | `resume <id>` | `--resume <id>` | `--session <id>` | `--session <id>` | `--resume <id>` * | `--resume <id>` | `--resume --session-id <id>` | no | `--session <id>` | `--session <id>` | `--resume <id>` |
| `--system <text>` | `--system-prompt <text>` | no | `--system-prompt-override <text>` | no | `--system-prompt <text>` | no | `--system-prompt <text>` | `--system <text>` | no | no | no | `--system <text>` |
| `--append-system <text>` | `--append-system-prompt <text>` | no | `--rules <text>` | no | `--append-system-prompt <text>` | no | `--append-system-prompt <text>` | `--system <text>` | no | no | no | no |
| `--add-dir <path>` | `--add-dir <path>` | `--add-dir <path>` | no | no | no | no | `--add-dir=<path>` | no | no | no | no | `--add-dir <path>` |
| `--cwd <path>` | no | `--cd <path>` | `--cwd <path>` | no | no | `--directory <path>` * | no | no | no | `--cwd <path>` | no | no |
| `--max-turns <n>` | `--max-turns <n>` † | no | `--max-turns <n>` | no | no | no | `--max-session-turns <n>` | `--max-turns <n>` | no | no | no | no |
| `--no-tools, --tools-off` | `--tools=` | no | no | no | `--no-tools` | no | no | no | no | no | no | no |
| `--no-skills, --skills-off` | `--disable-slash-commands` | no | no | no | `--no-skills` | no | no | no | no | no | no | no |

Checked against each agent's `--help`: claude 2.1.310, codex 0.133.0, grok 1.0.52, opencode 2.0.24, pi 0.73.1, qwen 0.26.1, goose 1.56.0, aider 0.86.2, crush 0.98.2, kilo 7.8.17, fx 0.0.14. † hidden from `--help`; the agent's parser accepts it. * from docs.poolside.ai (v1.0.16), not run. "no": the agent has no such flag, so anyr drops it with a one-line note. Goose takes these flags only on `goose session` or `goose run`, and its `--system` adds to the default prompt. Crush `-y` works on the interactive command only. Qwen's `-p` is deprecated but listed. fx takes `--add-dir` only on the interactive command and `--system` only on `fx ask`.
<!-- portable-flags:end -->

## Launcher settings: `anyr config`

`anyr config` prints your settings, including the model each agent launches
with. Read or change one setting by name:

```bash
anyr config get launcher.codex.model              # prints the value; exit 1 when unset
anyr config set launcher.codex.model openai/gpt-5.5
anyr config set launcher.claude.args -- --verbose  # values that start with `-` go after `--`
anyr config unset launcher.claude.args
anyr config get launcher.agent --json             # {"key":"launcher.agent","value":"codex"}
```

| Key | What it does |
|-----|--------------|
| `launcher.agent` | Agent a bare `anyr` starts. Order: `--tool`, then this, then the last agent you launched |
| `launcher.<agent>.model` | Model that agent launches with (`auto` clears it) |
| `launcher.<agent>.args` | Flags passed to that agent on every launch, space separated. They go after the portable switches and before your own flags, so yours win |
| `launcher.<agent>.yolo` | `true`: always skip that agent's permission prompts |
| `launcher.claude.{haiku,sonnet,opus,fable}` | Claude Code's model for each tier |
| `launcher.mcp_auto` | `false`: never wire the MCP gateway into a launched agent |
| `update.auto`, `update.channel` | Auto-update on or off; `stable` or `beta` |

An unknown key or a bad value exits 2 and lists the valid choices. The values
live in the same `~/.anyrouter/config.yaml` fields as before. The one new field
is `agents.<id>.args`.

`anyr config --json` has a `launcher` object: `agent`, `last_agent`,
`mcp_auto`, `claude.{haiku,sonnet,opus,fable}`, and per agent `model`,
`model_source` (`agent`, `account` or `default`), `args` and `yolo`.

Migration: the top-level `claude_haiku`, `claude_sonnet` and `claude_opus`
keys of `anyr config --json` moved to `launcher.claude.haiku`, `.sonnet` and
`.opus`. A script that read `.claude_opus` now reads `.launcher.claude.opus`,
or runs `anyr config get launcher.claude.opus`, which prints only a value you
set yourself.

## Documentation

[anyrouter.dev/cli](https://anyrouter.dev/cli) covers install and usage.
`anyr help --all` lists every command, `anyr <command> --help` explains one,
and `anyr help agents` is the guide for scripts and coding agents.

## Feedback

[Open an issue](https://github.com/AnyRouterHQ/cli/issues/new) in this repo.

## Contributing

You need a Rust toolchain. Test your change with the binary you just built, not
an older `anyr` on your `PATH`:

```bash
cargo build --locked
./target/debug/anyr --help
cargo test --locked --all-targets
cargo clippy --locked --all-targets
cargo fmt --check
```

Read [AGENTS.md](AGENTS.md) first. It lists the invariants (version lock, key
redaction, config file fields) and the size budget.

### Version lock (0.1.x)

`Cargo.toml`, `package.json`, and `.release-please-manifest.json` stay on
`0.1.x`. release-please uses `"versioning": "always-bump-patch"` so `feat`,
`feat!`, and `BREAKING CHANGE` all stay `0.1.x` (no 0.2.0 / 1.0.0 path).

Do not auto-merge release-please PRs.

### Git hooks

`.githooks/prepare-commit-msg` appends:

```
Co-authored-by: Duyet Le <me@duyet.net>
Co-authored-by: duyetbot <bot@duyet.net>
```

Enable:

```bash
./scripts/install-hooks.sh
# same as: git config core.hooksPath .githooks
```

Skip one commit: `ANYR_SKIP_COAUTHORS=1 git commit`

## License

MIT — Duyet Le
