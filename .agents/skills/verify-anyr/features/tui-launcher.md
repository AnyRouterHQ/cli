# TUI launcher

The compact HUD is the default launcher on a TTY. One status line, then `What do you want to do?` with arrow-key rows. First row is always `Launch claude` — even unsigned, even with nothing on PATH. Enter signs in if needed, installs Claude Code if missing, and starts it. `--dump-tui` prints a single ANSI-free frame for tests and pipes.

## Sub-features

- `menu-dump` prints the compact HUD: status line, `What do you want to do?`, `Launch claude`, `Config`, `Models`, `Quit`.
- `menu-dump-empty` still offers `Launch claude` when no agents are detected (`ANYR_AGENTS=none`).
- `menu-dump-unsigned` still offers `Launch claude` with no key (no separate Login row).
- `config-dump` prints grouped settings (`ACCOUNT`, `MODEL`, `AGENT`, `GENERAL`).
- `menu-quit` (PTY) leaves the interactive HUD with `q` / Escape without launching an agent.

## How to get to it (user POV)

- Run `anyr` or `anyr menu` on a TTY.
- Run `anyr menu --dump-tui` (or `ANYR_TUI_DUMP=1`) to print one frame and exit.
- Run `anyr config` (status dump) or `anyr config --pick` / `--dump-tui` for the settings TUI.
- ↑↓ / j k move, ↵ select, q / esc quit.
- Fullscreen palette only if `ANYR_TUI=1`.

## Driving it with control-anyr

Preconditions:

- `control-anyr doctor` is clean.
- Isolated config contains the fixture `default` profile.
- Helper sets `ANYR_AGENTS=claude,codex` unless a recipe overrides it.

- **HUD dump.** Run `control-anyr cli --out artifacts/tui-launcher/menu-dump.txt -- menu --dump-tui`. Exit code `0`. Stdout has no ESC (`\x1b`). Stdout contains `What do you want to do?`, `Launch claude`, `Config`, `Models`, `Quit`. Stdout does not contain `verify-fixture-not-a-real-key` or `Login`.
- **Empty agents.** Run `ANYR_AGENTS=none control-anyr cli --out artifacts/tui-launcher/menu-empty.txt -- menu --dump-tui` only if you temporarily export `ANYR_AGENTS=none` for that one command. Exit code `0`. Stdout contains `Launch claude`. Restore `ANYR_AGENTS=claude,codex` afterward (a new `doctor` / next `cli` uses the helper default).
- **Settings dump.** Run `control-anyr cli --out artifacts/tui-launcher/config-dump.txt -- config --dump-tui`. Exit code `0`. Stdout contains `ACCOUNT`, `MODEL`, `AGENT`, `GENERAL`. Secret substring `config-dump` / `verify-fixture-not-a-real-key` is absent.
- **Interactive HUD (optional).** Run `control-anyr pty start -- menu`, then `control-anyr pty capture --path artifacts/tui-launcher/pane.txt`. The pane shows `Launch claude` and `What do you want to do?`. Send `q`, then `control-anyr pty stop`. Do not press Enter on a launch row (that path launches an agent unless `--dry-run`).
- **Proof.** Keep `menu-dump.txt` and `config-dump.txt`. They identify this binary's HUD and settings without attaching a fullscreen TTY.

## Gotchas

- `ANYR_AGENTS` overrides PATH detection. Leave it at `claude,codex` for the default dump; `none` still shows `Launch claude` (install happens on launch).
- Dump mode is not a TTY session. Do not claim keybindings work from `--dump-tui` alone; use `pty` for ↑↓ / Enter.
- Credits rows may show a placeholder when `ANYR_NO_CATALOG=1`. That is expected isolation, not a product outage.
- Do not select `Launch claude` in the PTY without `--dry-run`. That path launches an agent.
- Do not dump the live model catalog from this flow. `anyrouter/auto` / most-used models are a separate job.
