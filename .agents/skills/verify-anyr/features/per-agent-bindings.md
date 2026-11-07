# Per-agent model, account, and key

CONFIGURE `model…` / `account…` / `key…` bind the **current coding agent** (header `agent` / last launch) without an extra agent-picker screen. Launch uses those bindings. A stored per-agent key is used as-is — it does not fall back to the default profile key.

## Sub-features

- `bind-account` pins an account on one agent via `account use <name> --agent <id>` without changing `active_profile`.
- `bind-model` pins a caller-supplied catalog id on one agent via `models use <id> --agent <id>` (does not invent ids).
- `launch-key` dry-run of claude vs grok uses each agent’s stored key, not the default profile key.
- `menu-dump` shows `per agent · <last>` on CONFIGURE rows and each launch row’s bound model.

## How to get to it (user POV)

- TUI: `anyr` / `anyr menu`. Set the current agent with `agent…` or by launching one, then `model…` / `account…` / `key…` (same pickers as before — no extra screens).
- CLI: `anyr account use work --agent claude`, `anyr models use stealth/ox-alpha --agent claude`.
- Launch: `anyr claude --dry-run --yes` / `anyr grok --dry-run --yes` (always `--dry-run` here).

## Driving it with control-anyr

Preconditions:

- `control-anyr doctor` is clean.
- Isolated config has profiles `default` and `work`.

- **Bind account.** Run `control-anyr cli --out artifacts/per-agent-bindings/account-claude.txt -- account use work --agent claude`. Exit `0`. Stdout contains `claude` and `work`. Isolated `config.yaml` has `active_profile: default` and `agents.claude.profile: work`.
- **Bind grok account separately.** Run `control-anyr cli -- account use default --agent grok` after writing a grok key/model if needed, or skip if the recipe writes the file.
- **Bind model.** Run `control-anyr cli --out artifacts/per-agent-bindings/model-claude.txt -- models use stealth/ox-alpha --agent claude`. Exit `0`. Config contains `default_model: stealth/ox-alpha` under `agents.claude` and does not contain `stealth/ox-alpha[1m]`.
- **Launch uses bindings.** With a config that gives claude `sk-ar-v1-claude-key-cccc` + `stealth/ox-alpha` and grok `sk-ar-v1-grok-key-dddd`, run `control-anyr cli --out artifacts/per-agent-bindings/claude-dry.txt -- claude --dry-run --yes` and `… grok --dry-run --yes`. Claude stdout has `ANTHROPIC_MODEL=stealth/ox-alpha[1m]` and redacted `…cccc`, not `…aaaa`. Grok stdout has redacted `…dddd`, not `…aaaa`. No agent child.
- **Palette dump.** Run `ANYR_AGENTS=claude,grok control-anyr cli --out artifacts/per-agent-bindings/menu-dump.txt -- menu --dump-tui`. Stdout contains `per agent · claude`, `stealth/ox-alpha`, `LAUNCH`, `CONFIGURE`. Does not contain `switch session default`.
- **Yolo still works.** `control-anyr cli -- claude --dry-run --yes --key sk-ar-v1-fixture-key-0001 --model "stealth/ox-alpha[1m]" --yolo` prints `--dangerously-skip-permissions`.
- **Proof.** Keep the dry-run files, `menu-dump.txt`, and a copy of `config.yaml` after the account/model binds.

## Gotchas

- Do not add an agent-picker screen in front of model/account/key. Bind `last_tool` (shown as `per agent · claude`).
- `ANYR_NO_CATALOG=1` is on. `models use <id> --agent` must not fetch the catalog.
- Never launch without `--dry-run`.
- Logo restore and “don’t flash CONFIGURE over LAUNCH” are issue #36, not this recipe.
