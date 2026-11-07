# Proof: tui-launcher (#36 pick-agent-then-settings)

Feature: `tui-launcher`
Entry points driven: `anyr menu --dump-tui`, empty-agents dump, `anyr config --dump-tui`, `anyr claude --dry-run --yes`, PTY `anyr menu`
Harness: `.agents/skills/verify-anyr/control-anyr` (`launch`, `doctor`, `cli`, `pty`, `cleanup`)
Binary: `target/debug/anyr` from this checkout (`0.1.11`)
Isolated home: `/tmp/anyr-verify-20260903-060828-7216/home` (removed by cleanup)

## Commands

```bash
.agents/skills/verify-anyr/control-anyr launch
.agents/skills/verify-anyr/control-anyr doctor
.agents/skills/verify-anyr/control-anyr cli --out artifacts/tui-launcher/menu-dump.txt -- menu --dump-tui
# empty-agents: helper always exports ANYR_AGENTS=claude,codex; drive the
# isolated binary directly with ANYR_AGENTS=none
ANYR_AGENTS=none ANYROUTER_HOME=… "$BIN" menu --dump-tui > artifacts/tui-launcher/menu-empty.txt
.agents/skills/verify-anyr/control-anyr cli --out artifacts/tui-launcher/config-dump.txt -- config --dump-tui
.agents/skills/verify-anyr/control-anyr cli --out artifacts/tui-launcher/claude-dry-run.txt -- claude --dry-run --yes --key sk-ar-v1-fixture-key-0001
.agents/skills/verify-anyr/control-anyr pty start -- menu
.agents/skills/verify-anyr/control-anyr pty capture --path artifacts/tui-launcher/pane.txt
.agents/skills/verify-anyr/control-anyr pty send Down
.agents/skills/verify-anyr/control-anyr pty capture --path artifacts/tui-launcher/pane-focus-codex.txt
.agents/skills/verify-anyr/control-anyr pty send Escape
.agents/skills/verify-anyr/control-anyr cleanup
```

## Results

| Artifact | Exit | Observable |
| --- | --- | --- |
| `menu-dump.txt` | 0 | Compact 3-row mark (`▄▄ ▄▄▄` / `▄█▀▀█▄▄█▀`). `LAUNCH` and `CONFIGURE · CLAUDE` on the same frame. Claude row: `stealth/ox-alpha · default ·` masked key. Codex row: `auto`. `model…` / `account…` / `key…` say `for claude`. `MORE` holds install/config/quit. No `verify-fixture-not-a-real-key`. |
| `menu-empty.txt` | 0 | `install an agent…`, `none detected`; launch list still present (not replaced by configure). |
| `config-dump.txt` | 0 | `ACCOUNT`, `MODEL`, `AGENT`, `GENERAL`, `auto-update`, `update channel`. |
| `claude-dry-run.txt` | 0 | `command: claude`, `ANTHROPIC_MODEL=stealth/ox-alpha[1m]`, key redacted. Did not spawn Claude. |
| `pane.txt` | PTY | Live home: `LAUNCH` + `CONFIGURE · CLAUDE` together. Highlight on claude. |
| `pane-focus-codex.txt` | PTY | Down moves highlight to codex. `CONFIGURE · CODEX` / `for codex`. Launch list still visible. Claude still shows `stealth/ox-alpha`. |

No `anyr claude` without `--dry-run`. No paid tokens. Model catalog / `anyrouter/auto` most-used dump not exercised (#37).

## Cleanup

`control-anyr cleanup` removed the isolated workdir and left this `artifacts/tui-launcher/` tree in place.
