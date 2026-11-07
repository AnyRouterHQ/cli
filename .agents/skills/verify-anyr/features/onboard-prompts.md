# Onboard prompts

Onboard prompts let a user copy a paste-ready instruction for a coding agent to wire, plan, or repair LLM calls through AnyRouter, without changing this checkout and without calling the gateway.

## Sub-features

- `onboard-impl` prints the implement contract (base URLs and `ANYROUTER_API_KEY`).
- `onboard-plan` prints a no-code-changes migration plan prompt.
- `onboard-shortcuts` accept `impl`, `plan`, `fix`, and `deploy` as top-level commands.
- `onboard-json` emits machine-readable `{ "mode": ... }` for `onboard <mode> --json`.
- `onboard-needs-mode` errors on `onboard` with no mode when stdin is not a TTY.

## How to get to it (user POV)

- Run `anyr onboard impl` (README quick start).
- Run `anyr impl`, `anyr plan`, `anyr fix`, `anyr deploy`, or `anyr cp`.
- Run `anyr onboard plan --json`.
- Run `anyr onboard` with no mode (non-TTY).

## Driving it with control-anyr

Preconditions:

- `control-anyr doctor` is clean.
- No live AnyRouter key is required. These commands print prompts; they do not call the API.

- **Implement prompt.** Run `control-anyr cli --out artifacts/onboard-prompts/impl.txt -- onboard impl`. Exit code `0`. Stdout contains `ANYROUTER_API_KEY`, `https://anyrouter.dev/api/v1`, and `https://anyrouter.dev/api`.
- **Plan shortcut.** Run `control-anyr cli --out artifacts/onboard-prompts/plan.txt -- plan`. Exit code `0`. Stdout is non-empty and tells the agent not to change code yet.
- **JSON plan.** Run `control-anyr cli --out artifacts/onboard-prompts/plan.json -- onboard plan --json`. Exit code `0`. Stdout contains `"mode":"plan"` or `"mode": "plan"` and `do not change` (case-insensitive).
- **Missing mode.** Run `control-anyr cli --out artifacts/onboard-prompts/no-mode.txt -- onboard`. Exit code is not `0`. Combined output contains `Specify a mode` or `onboard --help`.
- **Proof.** Keep `impl.txt` and `plan.json`. They show a user can copy an impl prompt and a structured plan prompt without a network round-trip.

## Gotchas

- `onboard` with no mode on a TTY may open a picker. `control-anyr cli` is non-TTY and must error instead.
- These prompts mention fetching docs URLs as instructions to the *target* agent. The `anyr` process itself does not need to fetch them for this recipe.
- Do not treat a successful onboard print as proof that `anyr claude` works.
