# Update progress

`anyr update` shows a ticking spinner with the current → target version and configured channel, then a checkmark success line and a restart hint. The loader must actually move; a frozen `[loading icon]` is a fail.

## Sub-features

- `update-copy` prints `Updating vX -> vY (<channel> channel)` using the running binary version and the selected GitHub channel.
- `update-success` prints `✔` plus the new version and `Run anyr to start using the new version.`
- `update-spinner` on a TTY rewrites the status line with changing spinner frames (`\r` plus distinct glyphs).
- `update-check` keeps the labeled current/latest/channel report and does not install.

## How to get to it (user POV)

- Run `anyr update` to install the latest release for the configured channel.
- Run `anyr update --beta` or `anyr update --stable` to persist a channel and update it.
- Run `anyr upgrade` (alias) the same way.
- Run `anyr update --check` to compare versions without installing.

## Driving it with control-anyr

Preconditions:

- `control-anyr doctor` is clean.
- Isolated `ANYROUTER_HOME` is in use.
- Set `ANYR_RELEASES_JSON` to `control-anyr path RELEASES_FIXTURE` so the run stays offline.
- Do not hit live GitHub Releases.

- **Non-TTY copy.** Export `ANYR_RELEASES_JSON` to the fixture and `ANYR_CHANNEL=beta`. Run `control-anyr cli --out artifacts/update-progress/update.txt -- update`. Exit code `0`. Stdout contains `Updating`, `->`, `beta channel`, a fixture target version (`0.2.0-beta.1`), `✔`, and `Run anyr to start using the new version.` Stdout does not invent model ids.
- **Check still reports.** Run `control-anyr cli --out artifacts/update-progress/check.txt -- update --check`. Exit code `0`. Stdout contains `current:`, `latest:`, `channel:`, and either `update available` or `up to date`.
- **TTY spinner ticks.** Export `ANYR_RELEASES_JSON`, `ANYR_CHANNEL=beta`, `ANYR_SPINNER_MS=20`, and `ANYR_SPINNER_MIN_TICKS=6`. Run `control-anyr pty run --timeout 6 --out artifacts/update-progress/tty.txt -- update`. The transcript contains at least two distinct spinner glyphs from `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`, a carriage return, `Updating`, `beta channel`, and `Run anyr to start using the new version.`
- **Proof.** Keep `update.txt`, `update.txt.exit`, and `tty.txt`. Together they show from→to + channel copy and that the loader frames change.

## Gotchas

- Fixture mode does not replace the binary (`ANYR_RELEASES_JSON` is treated as dry-run). Success copy is `Would update to` instead of `Updated to`.
- `control-anyr cli` is not a TTY, so it will not animate. Use `pty run` for frame-change proof.
- `ANYR_NO_UPDATE=1` only skips background auto-update. Explicit `anyr update` still runs.
- Existing coverage lives in `tests/cli.rs` (`update_prints_from_to_channel_and_success`, `update_spinner_animates_on_tty`) and `src/spinner.rs`. Run those as a complement, not as the only user-path proof.
