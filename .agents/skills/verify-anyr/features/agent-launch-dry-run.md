# Agent launch dry-run

Agent launch dry-run lets a user see the exact child command and gateway env `anyr` would use to start Claude Code, Codex, OpenCode, or Pi, without spawning that agent or spending tokens.

## Sub-features

- `dry-run-claude` prints `ANTHROPIC_BASE_URL` and redacts the API key.
- `dry-run-codex` prints `OPENAI_BASE_URL` with the same redaction.
- `dry-run-yes` keeps `--yes` / `--ok` accepted (named launch does not open a picker).
- `dry-run-yolo` expands Claude `--yolo` to `--dangerously-skip-permissions` in the printed args only.
- `dry-run-no-spawn` does not start a `claude` / `codex` process.

## How to get to it (user POV)

- Run `anyr claude --dry-run --ok` (or `--yes`) with a key via `--key` or a saved profile.
- Run the same pattern for `codex`, `opencode`, or `pi`.
- Pass `--yolo` on `anyr claude --dry-run` to inspect the dangerous-permissions flag without launching.

## Driving it with control-anyr

Preconditions:

- `control-anyr doctor` is clean.
- Isolated `ANYROUTER_HOME` is in use.
- Use fixture key `sk-ar-v1-fixture-key-0001` on the command line. Do not export a live `ANYROUTER_API_KEY`.
- Do **not** omit `--dry-run`.

- **Claude dry-run.** Run `control-anyr cli --out artifacts/agent-launch-dry-run/claude.txt -- claude --dry-run --yes --key sk-ar-v1-fixture-key-0001`. Exit code `0`. Stdout contains `command:`, `ANTHROPIC_BASE_URL`, and `env:`. Stdout does not contain `sk-ar-v1-fixture-key-0001`.
- **Codex dry-run.** Run `control-anyr cli --out artifacts/agent-launch-dry-run/codex.txt -- codex --dry-run --yes --key sk-ar-v1-fixture-key-0001`. Exit code `0`. Stdout contains `OPENAI_BASE_URL` and does not contain the full fixture key.
- **Yolo expansion (Claude, still dry).** Run `control-anyr cli --out artifacts/agent-launch-dry-run/claude-yolo.txt -- claude --dry-run --yes --key sk-ar-v1-fixture-key-0001 --yolo`. Exit code `0`. Stdout contains `"--dangerously-skip-permissions"`.
- **Confirm no spawn.** After each dry-run, no `claude` or `codex` child of this run is running. Dry-run stdout is a printout, not a live session.
- **Proof.** Keep `claude.txt` and `claude.txt.exit`. They show the gateway env a user would get, with the secret redacted, and exit `0` without launching an agent.

## Gotchas

- `--ok` / `--yes` without `--dry-run` starts the agent. That is a paid path. Skip it.
- `--yolo` is not a free-token flag. It only adds Claude's skip-permissions switch. Repo docs do not describe a free/yolo inference path.
- `--no-check` skips the pre-launch reachability probe; dry-run already avoids the child, so do not treat `--no-check` as a substitute for `--dry-run`.
- Dummy keys `sk-ar-v1-test`, `sk-ar-v1-testkey`, and `sk-ar-v1-test-key` are ignored as live credentials. Use `sk-ar-v1-fixture-key-0001` like `tests/cli.rs`.
- Existing coverage: `spawn_targets_dry_run_inject_gateway_and_redact_key` and `claude_yolo_flag_expands_to_dangerously_skip_permissions` in `tests/cli.rs`.
