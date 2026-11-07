# anyr verification map

This directory is the maintained source for verifying user-facing `anyr` CLI behavior. Read the index before driving the app, then use the matching feature file as the recipe.

## Baseline preconditions

- Run `.agents/skills/verify-anyr/control-anyr launch` then `doctor`.
- Isolated home is `control-anyr path ANYROUTER_HOME`. Never use `~/.anyrouter`.
- Seed config is `control-anyr path CONFIG_FILE` with profiles `default` and `work` and fixture keys (not live credentials).
- `ANYR_NO_UPDATE=1`, `ANYR_NO_CATALOG=1`, `ANYR_AGENTS=claude,codex`.
- Put the helper on your command path for the run, or invoke it by repo-relative path.
- Never drive an `anyr` process or tmux session this run did not start.
- Never launch `claude` / `codex` / `grok` / `opencode` / `pi` / `pool` without `--dry-run`.

## Driving conventions

- Start every recipe from the baseline state unless its preconditions say otherwise.
- Run non-interactive commands as `control-anyr cli -- <args>`.
- Prefer `menu --dump-tui` / `config --dump-tui` over an interactive PTY when the claim is frame content.
- Run interactive TUI as `control-anyr pty start` / `send` / `capture` / `stop`.
- Treat every command as literal. Keep profile names `default` / `work` and fixture key prefixes unchanged.
- Restore baseline after a mutation that would break later recipes (`default` remains the active profile; extra files created by a recipe are removed in that recipe).
- Do not remove proof artifacts during cleanup.
- Prefer `control-anyr test` for regressions the suite already covers, then still drive the mapped user command for a user-visible claim.

## Proof and skip reporting

- Capture the user action and the resulting state, not only the last command.
- CLI proof includes the command, stdout, stderr, and exit code.
- Mutation proof includes a read-only second view (`whoami`, `config path`, or the isolated `config.yaml`).
- TUI proof includes a `--dump-tui` frame or a PTY pane with `LAUNCH` or the AR mark visible.
- `--dry-run` proof includes the printed `command:` / `env:` block and confirmation that no agent child was started.
- Record the feature ID and entry point used with every artifact.
- Report an unreachable path with the attempted command and the unmet precondition.
- Do not report a skipped entry point as verified through a different path.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph describing the user-visible behavior. It then uses exactly four H2 sections in this order.

1. `Sub-features` lists short IDs with one line for each behavior.
2. `How to get to it (user POV)` lists every user entry point.
3. `Driving it with control-anyr` starts with `Preconditions:` and uses labeled bullets that pair each user action with an exact command and observable result.
4. `Gotchas` lists traps that can waste or invalidate a verification run.

Keep implementation details out of the map. Name only user paths, stable handles, required state, commands, and observable proof.

## Features

- [Help and version](./help-and-version.md) covers root `--help`, `--version`, grouped command help, and unknown-command errors.
- [Onboard prompts](./onboard-prompts.md) covers paste-ready impl/plan/fix prompts and JSON output without network.
- [Agent launch dry-run](./agent-launch-dry-run.md) covers printing the child command and redacted env without spawning an agent.
- [TUI launcher](./tui-launcher.md) covers the compact HUD (`menu --dump-tui`), unsigned/empty PATH still offering Launch claude, `config --dump-tui`, and optional PTY quit.
- [Model picker catalog](./model-picker.md) covers the inline model picker pinning `anyrouter/auto` without a most-used dump.
- [Accounts and config](./accounts-and-config.md) covers whoami, account switch, config path, and logout against the isolated home.
- [Per-agent bindings](./per-agent-bindings.md) covers inline model/account/key binds per coding agent and launch that honors per-agent keys.
- [Routing filters](./routing-filters.md) covers exacto / tools / 1M ctx toggles per agent and launch extra-body fields.
- [Update progress](./update-progress.md) covers `anyr update` from→to + channel copy, the success hint, and a TTY spinner that actually ticks.
