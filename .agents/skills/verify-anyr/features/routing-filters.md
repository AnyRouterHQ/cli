# Routing filters (exacto, tools, 1M ctx)

CONFIGURE exposes Exacto / require-tools / ≥1M-context toggles for the current coding agent. Values persist per agent in `config.yaml` using AnyRouter preset field names (`provider.sort`, `require_params`, `min_context`) and launch forwards them on the request. The default model picker stays `anyrouter/auto`.

## Sub-features

- `toggles-persist` writes exacto / tools / 1M ctx per agent in `agents.<id>` without changing sibling agents.
- `launch-sends` dry-run of `anyr claude --model anyrouter/auto` (and `anyrouter/free`) includes `CLAUDE_CODE_EXTRA_BODY` with those fields.
- `picker-auto` still pins `anyrouter/auto` and does not dump most-used models.
- `launch-above-configure` keeps `LAUNCH` above `CONFIGURE` in the home dump.

## How to get to it (user POV)

- TUI: `anyr` / `anyr menu`. Highlight an agent, then toggle `exacto` / `tools` / `1M ctx` on the same CONFIGURE list.
- Settings: `anyr config`, agent tab, Routing rows. Enter toggles; `x` turns a row off.
- Launch: `anyr claude --model anyrouter/auto --dry-run --yes` (always `--dry-run` here). Same for `anyrouter/free`.
- `--yes` / `--ok` keep the stored routing settings.

## Driving it with control-anyr

Preconditions:

- `control-anyr doctor` is clean.
- Isolated config has profiles `default` and `work`.
- Helper sets `ANYR_NO_CATALOG=1`. Do not invent catalog ids. Do not use ox-alpha.

- **Menu dump.** Run `ANYR_AGENTS=claude,grok control-anyr cli --out artifacts/routing-filters/menu-dump.txt -- menu --dump-tui`. Exit `0`. Stdout contains `LAUNCH`, `CONFIGURE`, `exacto`, `tools`, `1M ctx`, `anyrouter/auto` or `for claude`. `LAUNCH` appears before `CONFIGURE`. Stdout does not contain `most used`.
- **Models dump.** Run `control-anyr cli --out artifacts/routing-filters/models-dump.txt -- models --dump-tui`. Exit `0`. Stdout contains `anyrouter/auto`. Stdout does not contain `most used`.
- **Launch sends constraints.** Write routing onto `agents.claude` (`provider.sort: exacto`, `require_params: tools`, `min_context: 1000000`). Run `control-anyr cli --out artifacts/routing-filters/claude-auto.txt -- claude --dry-run --yes --model anyrouter/auto` and `… --model anyrouter/free`. Exit `0`. Stdout contains `CLAUDE_CODE_EXTRA_BODY=`, `"sort":"exacto"`, `"require_params":["tools"]`, `"min_context":1000000`. No agent child. Fixture key redacted.
- **Proof.** Keep the dry-run files, `menu-dump.txt`, and a copy of `config.yaml` after the toggles.

## Gotchas

- `--yes` / `--ok` without `--dry-run` starts Claude. Skip that.
- Do not invent model ids. Real presets are `anyrouter/auto` and `anyrouter/free`.
- Routing is per agent (#35/#36). Toggling claude must not rewrite grok.
- Logo restore / “don’t flash CONFIGURE over LAUNCH” stays a home-TUI invariant (#36).
