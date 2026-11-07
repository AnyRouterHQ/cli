# Model picker catalog

The inline model picker lets a user select the `anyrouter/auto` preset without filling the list from a most-used / usage ranking. Typeahead filters the same inline list. No extra screen.

## Sub-features

- `picker-dump` prints one model-picker frame that includes `anyrouter/auto` and does not say `most used`.
- `picker-use-auto` persists `anyrouter/auto` from `models use` without a catalog fetch.
- `ox-alpha-yolo` keeps `anyr claude --model stealth/ox-alpha[1m] --yolo --dry-run` working.

## How to get to it (user POV)

- Run `anyr models --dump-tui` (or `anyr models --pick --dump-tui`) to print the picker frame.
- From the launcher, choose `model…` (same inline picker; TTY).
- From settings, edit the default model row (same inline picker; TTY).
- Run `anyr models use anyrouter/auto` to pin the preset from the CLI.
- Type `anyrouter/auto` in the picker search to select the preset.

## Driving it with control-anyr

Preconditions:

- `control-anyr doctor` is clean.
- Isolated config contains the fixture `default` profile.
- Helper sets `ANYR_NO_CATALOG=1`, so the dump is the picker catalog (preset only), not a live usage list.

- **Picker dump.** Run `control-anyr cli --out artifacts/model-picker/dump.txt -- models --dump-tui`. Exit code `0`. Stdout contains `anyrouter/auto`. Stdout does not contain `most used`. Stdout has no ESC (`\x1b`).
- **Pin the preset.** Run `control-anyr cli --out artifacts/model-picker/use-auto.txt -- models use anyrouter/auto`. Exit code `0`. Stdout contains `anyrouter/auto`. Isolated `config.yaml` no longer pins a concrete catalog id as `default_model` (auto stays unset / preset).
- **Ox-alpha yolo (dry).** Run `control-anyr cli --out artifacts/model-picker/ox-alpha-yolo.txt -- claude --dry-run --yes --key sk-ar-v1-fixture-key-0001 --model stealth/ox-alpha[1m] --yolo`. Exit code `0`. Stdout contains `ANTHROPIC_MODEL=stealth/ox-alpha[1m]` and `"--dangerously-skip-permissions"`. Stdout does not contain the full fixture key.
- **Proof.** Keep `dump.txt` and `use-auto.txt`. They show the picker data source can set `anyrouter/auto` without a most-used dump.

## Gotchas

- `ANYR_NO_CATALOG=1` is expected isolation. The dump then shows the pinned preset, not a live catalog. That is the proof that the picker is not a usage ranking dump.
- `--dump-tui` is not a TTY session. Do not claim keybindings work from the dump alone; use `pty` only if you need to type the query.
- Do not invent other model ids. `anyrouter/auto` is the real preset. Concrete ids like `stealth/ox-alpha` stay CLI `--model` paths.
- `--yolo` without `--dry-run` starts Claude. Skip that.
