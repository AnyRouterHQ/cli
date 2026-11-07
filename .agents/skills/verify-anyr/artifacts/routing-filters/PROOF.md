# Proof: routing-filters (#43)

Feature: `routing-filters`
Entry points driven: `anyr menu --dump-tui`, `anyr models --dump-tui`, `anyr config --dump-tui`, `anyr claude --dry-run --yes --model anyrouter/auto`, `anyr claude --dry-run --yes --model anyrouter/free`, PTY `anyr menu` (focus + exacto toggle)
Harness: `.agents/skills/verify-anyr/control-anyr` (`launch`, `doctor`, `cli`, `test`, `pty`, `cleanup`)
Binary: `target/debug/anyr` from this checkout (`0.1.11`)
Isolated home: `/tmp/anyr-verify-20260903-130039-7117/home` (removed by cleanup)

## Commands

```bash
.agents/skills/verify-anyr/control-anyr launch
.agents/skills/verify-anyr/control-anyr doctor
.agents/skills/verify-anyr/control-anyr test -- --lib --test cli
.agents/skills/verify-anyr/control-anyr cli --out artifacts/routing-filters/menu-dump.txt -- menu --dump-tui
.agents/skills/verify-anyr/control-anyr cli --out artifacts/routing-filters/models-dump.txt -- models --dump-tui
.agents/skills/verify-anyr/control-anyr cli --out artifacts/routing-filters/config-dump.txt -- config --dump-tui
# write agents.claude provider.sort / require_params / min_context (and pin anyrouter/auto)
.agents/skills/verify-anyr/control-anyr cli --out artifacts/routing-filters/menu-dump-after.txt -- menu --dump-tui
.agents/skills/verify-anyr/control-anyr cli --out artifacts/routing-filters/claude-auto.txt -- claude --dry-run --yes --model anyrouter/auto
.agents/skills/verify-anyr/control-anyr cli --out artifacts/routing-filters/claude-free.txt -- claude --dry-run --yes --model anyrouter/free
.agents/skills/verify-anyr/control-anyr pty start -- menu
.agents/skills/verify-anyr/control-anyr pty send Down   # retarget configure to codex
.agents/skills/verify-anyr/control-anyr pty send Up
.agents/skills/verify-anyr/control-anyr pty send --literal 'exa'
.agents/skills/verify-anyr/control-anyr pty send Enter  # toggle claude exacto off
.agents/skills/verify-anyr/control-anyr pty stop
.agents/skills/verify-anyr/control-anyr cleanup
```

## Results

| Artifact | Exit | Observable |
| --- | --- | --- |
| `menu-dump.txt` | 0 | `LAUNCH` before `CONFIGURE`. Rows `exacto` / `tools` / `1M ctx` (off). No `most used`. Key masked. |
| `models-dump.txt` | 0 | Picker pins `anyrouter/auto`. No `most used`. |
| `config-dump.txt` | 0 | Settings chrome; default model `anyrouter/auto`. |
| `config-after-toggles.yaml` | — | `agents.claude` has `provider.sort: exacto`, `require_params: tools`, `min_context: 1000000`. `agents.codex` unchanged (`default_model: auto` only). |
| `menu-dump-after.txt` | 0 | Claude `anyrouter/auto` plus truncated exacto flags. CONFIGURE `on · for claude`. `LAUNCH` still above `CONFIGURE`. |
| `claude-auto.txt` | 0 | `command: claude`, `ANTHROPIC_MODEL=anyrouter/auto`, `CLAUDE_CODE_EXTRA_BODY={"min_context":1000000,"provider":{"sort":"exacto"},"require_params":["tools"]}`. Key redacted. Did not spawn Claude. |
| `claude-free.txt` | 0 | Same extra body. `ANTHROPIC_MODEL=anyrouter/free`. Did not spawn Claude. |
| `pane-home.txt` | PTY | Live home: `LAUNCH` + `CONFIGURE · CLAUDE` together. exacto/tools/1M `on · for claude`. |
| `pane-focus-codex.txt` | PTY | Down retargets to `CONFIGURE · CODEX` with `off · for codex` (does not copy claude's on). Launch list still visible. |
| `pane-filter-exacto.txt` | PTY | Typeahead `exa` highlights the exacto row; launch rows remain on screen. |
| `config-after-pty-toggle.yaml` | — | Enter cleared `provider.sort` on claude. `require_params` / `min_context` remain. Codex sibling unchanged. |

`cargo test --locked --lib --test cli`: 186 lib + 68 cli tests passed.

No `anyr claude` without `--dry-run`. No invented catalog ids. ox-alpha not used on the launch path (`--model anyrouter/auto` and `anyrouter/free` only).
