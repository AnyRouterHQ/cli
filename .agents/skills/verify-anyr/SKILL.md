---
name: verify-anyr
description: Drive the AnyRouter CLI (`anyr`) the way a user does — isolated ANYROUTER_HOME, existing cargo tests, PTY/tmux for the TUI, dry-run for agent launch. Use when proving a CLI/TUI change, checking help/onboard/auth/config, or confirming a spawn path without burning paid tokens.
---

# Verify anyr

Project-local control skill for agents. Read cold: this is how you launch, doctor, drive, capture evidence, and clean up without guessing.

The primary surface is the **`anyr` CLI** (native Rust binary in this repo). On a TTY, bare `anyr` / `anyr menu` opens the compact HUD (Launch claude first); pipes and CI get `--help`. Secondary surfaces (npm wrapper `scripts/npx-anyr.js`, wasm demo, `setup.sh` installer) are out of scope unless a feature file names them.

There is no long-lived server. Launch builds the binary once, then each drive is a short-lived `anyr` process (or a PTY/tmux session) against a disposable `ANYROUTER_HOME`.

Never point this skill at the operator's real `~/.anyrouter`. Two verification homes can exist on disk, but this helper allows only one active run. Refuse to drive an `anyr` process you did not start.

Do **not** run `anyr claude`, `anyr codex`, or any other agent launch without `--dry-run`. `--yolo` / `--ok` / `--yes` without `--dry-run` starts a real coding agent and can burn paid tokens. `--yolo` is not a free path; it only adds `--dangerously-skip-permissions` to Claude. There is no documented free/yolo token path in this repo.

## Launch

From the repo root:

```bash
.agents/skills/verify-anyr/control-anyr launch
.agents/skills/verify-anyr/control-anyr doctor
```

Launch is ready when it prints `ready: anyr --help is examples-first` and doctor prints only `ok` lines.

What launch does:

1. `cargo build --locked --bin anyr` (debug binary at `target/debug/anyr`). Needs a recent stable Rust — CI uses `dtolnay/rust-toolchain@stable`. rustc 1.83 cannot parse crates in this lockfile (`edition2024`).
2. Creates `$TMPDIR/anyr-verify-$RUN_ID/` with disposable `ANYROUTER_HOME=$WORKDIR/home`.
3. Copies `.agents/skills/verify-anyr/seed-config.yaml` to `$ANYROUTER_HOME/config.yaml` (fixture keys, `auto_update: false`, profiles `default` and `work`).
4. Sets `ANYR_NO_UPDATE=1`, `ANYR_NO_CATALOG=1`, `ANYR_AGENTS=claude,codex`, `ANYR_GRAPHICS=off`, and unsets `ANYROUTER_API_KEY`.

Teardown:

```bash
.agents/skills/verify-anyr/control-anyr cleanup
```

Cleanup deletes the tmux session this run started (if any) and the isolated workdir. It does not delete `.agents/skills/verify-anyr/artifacts/`.

## Doctor

Run before driving, and again whenever output looks like the operator's real config leaked in:

```bash
.agents/skills/verify-anyr/control-anyr doctor
```

Doctor must report:

- `binary` — executable `target/debug/anyr` from this checkout.
- `version` — `anyr --version` starts with `0.1.` (this line stays 0.1.x).
- `help` — `anyr --help` contains `point any coding agent`, `auth login`, `claude`, and `help commands`.
- `isolated` — `ANYROUTER_HOME` is under `anyr-verify-` and is not `~/.anyrouter`.
- `config` / `config_path` — `anyr config path` prints that isolated `config.yaml`.
- `whoami` — starts with `Signed in`, account row `default`, key masked, fixture secret not printed in full.
- `update_guard` — auto-update and live catalog are off.
- `pty` — `stopped` or `running` for session `anyr-verify`.
- `paid_guard` — reminder not to spawn agents without `--dry-run`.

Any `FAIL` line means do not drive.

Inspect paths with:

```bash
.agents/skills/verify-anyr/control-anyr paths
.agents/skills/verify-anyr/control-anyr path CONFIG_FILE
```

`paths` prints shell-quoted `KEY=value` lines (`eval "$(... paths)"` is safe).

## Drive

Prefer the in-repo suite first, then the real binary:

```bash
.agents/skills/verify-anyr/control-anyr test
.agents/skills/verify-anyr/control-anyr test -- --test cli help_lists_login_claude_account_and_spawn_targets
```

`tests/cli.rs` already covers help, onboard, `--dry-run` spawn, `--dump-tui`, account switch, logout, and upgrade fixtures. A passing test is not a substitute for driving the mapped user path when the claim is user-visible CLI behavior.

Non-interactive commands go through the helper so they inherit the isolated env:

```bash
.agents/skills/verify-anyr/control-anyr cli -- --help
.agents/skills/verify-anyr/control-anyr cli --out artifacts/help-and-version/help.txt -- --help
```

`cli --` is followed by arguments to `anyr` (no extra `anyr` token). Relative `--out` paths resolve from `.agents/skills/verify-anyr/`. The helper also writes `$out.err` and `$out.exit`.

Stable handles. Match these strings, not column layout or ANSI color.

| Action | Command | Observable |
| --- | --- | --- |
| Help | `cli -- --help` | `point any coding agent`, `auth login`, `claude`, `help commands`, exit `0`. Must not contain `Install:` or `npx @anyr/cli` |
| Version | `cli -- --version` | stdout starts with `0.1.`, exit `0` |
| Command help | `cli -- auth --help` / `cli -- claude --help` | subcommands or `--dry-run`; exit `0`; not `Unknown command` |
| Onboard impl | `cli -- onboard impl` | `ANYROUTER_API_KEY`, `https://anyrouter.dev/api/v1`, `https://anyrouter.dev/api` |
| Onboard plan JSON | `cli -- onboard plan --json` | `"mode":"plan"` (or spaced) and `do not change` |
| Whoami | `cli -- whoami` | starts `Signed in`; `account` row `default`; full fixture key absent |
| Config path | `cli -- config path` | isolated `config.yaml` |
| Account switch | `cli -- account use work` | stdout contains `work`; follow with `whoami` |
| Menu dump | `cli -- menu --dump-tui` | ANSI-free HUD dump with `What do you want to do?`, `Launch claude`, `Config`, `Models`, `Quit`; secret absent |
| Config dump | `cli -- config --dump-tui` | sections `ACCOUNT`, `MODEL`, `AGENT`, `GENERAL` |
| Agent dry-run | `cli -- claude --dry-run --yes --key sk-ar-v1-fixture-key-0001` | `command:`, `ANTHROPIC_BASE_URL`, key redacted. **Does not spawn Claude** |
| Upgrade check | `ANYR_RELEASES_JSON=$(control-anyr path RELEASES_FIXTURE)` then `cli -- upgrade --check --dry-run` | no install; no live GitHub required |

`--dump-tui` / `ANYR_TUI_DUMP=1` prints one plain frame and exits. Use it instead of an interactive TUI when the claim is frame content.

Interactive TUI (only when dump is not enough):

```bash
.agents/skills/verify-anyr/control-anyr pty start -- menu
.agents/skills/verify-anyr/control-anyr pty send --literal 'q'
.agents/skills/verify-anyr/control-anyr pty capture --path artifacts/tui-launcher/pane.txt
.agents/skills/verify-anyr/control-anyr pty stop
```

Keys on the HUD: ↑↓ / j k move · ↵ select · `q` / esc quit. First row is Launch claude (signs in and installs if needed). Settings (`anyr config --pick`): ↑↓ / j k, ↵ edit, `x` reset row, `q` / esc close. One-shot PTY without tmux:

```bash
.agents/skills/verify-anyr/control-anyr pty run --wait 'CORE COMMANDS' --out artifacts/help-and-version/help-pty.txt -- --help
```

That invokes `.agents/skills/verify-anyr/helpers/pty-anyr.py`.

## Evidence

Proof artifacts go under `.agents/skills/verify-anyr/artifacts/<feature-id>/`. Cleanup must not delete that tree.

Standards:

- Drive the shipped `anyr` binary through `control-anyr`, not by calling `anyr_cli::run` from a unit test and calling that the user path.
- Capture the command of the action and a second read-only view of the resulting state (`whoami`, `config path`, `menu --dump-tui`, or the isolated `config.yaml`).
- CLI proof is stdout, stderr, and exit code (`$out`, `$out.err`, `$out.exit`).
- TUI proof is either `--dump-tui` (preferred) or a PTY pane that shows `Launch claude` / `What do you want to do?` plus the action result.
- Mutation proof includes the isolated `config.yaml` after the write (`account use`, `logout`, `models use`).
- `--dry-run` is the safe spawn path. Prove it skipped the child by observing no new agent process and stdout that starts with `command:` / `env:`. Do not trust the flag name alone.
- Do not hit `https://anyrouter.dev` for usage, models, keys, login validation, or live `anyr claude`. Onboard prompts and `--help` are offline. Upgrade checks use `tests/fixtures/releases.json`.
- Record the feature ID and entry point with every artifact.
- Report an unreachable path with the attempted command and the unmet precondition. Do not report a skipped entry point as verified through a different path.

## Cleanup

```bash
.agents/skills/verify-anyr/control-anyr pty stop   # if a PTY was started
.agents/skills/verify-anyr/control-anyr cleanup
```

Cleanup kills tmux session `anyr-verify` (the session this run created) and `rm -rf` on the run's `anyr-verify-*` workdir only. It never `pkill anyr` and never deletes `artifacts/`.

## Helpers

`control-anyr` is executable. Invoke it with a path from the repo root, or `cd` into the skill directory.

```bash
.agents/skills/verify-anyr/control-anyr launch
.agents/skills/verify-anyr/control-anyr doctor
.agents/skills/verify-anyr/control-anyr test -- --test cli help_lists_login_claude_account_and_spawn_targets
.agents/skills/verify-anyr/control-anyr cli --out artifacts/help-and-version/help.txt -- --help
.agents/skills/verify-anyr/control-anyr path CONFIG_FILE
.agents/skills/verify-anyr/control-anyr paths
.agents/skills/verify-anyr/control-anyr pty run --wait 'CORE COMMANDS' -- --help
.agents/skills/verify-anyr/control-anyr cleanup
```

`helpers/pty-anyr.py` is executable. `control-anyr pty run` is the supported invocation; do not reverse-engineer flags from the script unless that wrapper is missing.

`seed-config.yaml` is verification scaffolding. Launch copies it; cleanup removes the copy with the workdir.

Read `features/README.md` before choosing what to drive.
