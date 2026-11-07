# per-agent-bindings proof

Isolated `control-anyr` run. Feature: `per-agent-bindings`.

## Entry points

- CLI: `account use work --agent claude`, `models use stealth/ox-alpha --agent claude`
- Launch: `claude --dry-run --yes`, `grok --dry-run --yes`
- TUI: `menu --dump-tui`; PTY `menu` → type `acc` → Enter → `claude account` picker (no extra agent screen) → bind profile

## Results

- `account-claude.txt` / `config-after-account.yaml`: `active_profile: default`, `agents.claude.profile: work`
- `model-claude.txt` / `config-after-model.yaml`: `agents.claude.default_model: stealth/ox-alpha` (no `[1m]` in config)
- `claude-dry.txt`: `ANTHROPIC_AUTH_TOKEN=sk-ar-...cccc`, `ANTHROPIC_MODEL=stealth/ox-alpha[1m]`
- `grok-dry.txt`: `GROK_CODE_XAI_API_KEY=sk-ar-...dddd` (not the default profile key)
- `claude-yolo.txt`: `--dangerously-skip-permissions` and `stealth/ox-alpha[1m]`
- `menu-dump.txt`: `per agent · claude`, LAUNCH claude `stealth/ox-alpha`, no `switch session default`
- `pty-account-picker.txt`: title `claude account` immediately after Enter on `account…`
- `config-after-tui.yaml`: picker wrote `agents.claude.profile: default`

No agent child was started. Logo / CONFIGURE-over-LAUNCH chrome was not changed (issue #36).
