# Proof: update-progress

Feature: `update-progress`
Entry points driven: `anyr update` (fixture, non-TTY), `anyr update --check`, `anyr update` on a PTY
Harness: `.agents/skills/verify-anyr/control-anyr` (`launch`, `doctor`, `cli`, `pty run`, `test`, `cleanup`)
Binary: `target/debug/anyr` from this checkout (`0.1.11`)
Isolated home: `/tmp/anyr-verify-20260903-060715-6103/home` (removed by cleanup)
Releases: `tests/fixtures/releases.json` via `ANYR_RELEASES_JSON` (offline)

## Commands

```bash
.agents/skills/verify-anyr/control-anyr launch
.agents/skills/verify-anyr/control-anyr doctor
ANYR_RELEASES_JSON="$(.agents/skills/verify-anyr/control-anyr path RELEASES_FIXTURE)"
ANYR_CHANNEL=beta
.agents/skills/verify-anyr/control-anyr cli --out artifacts/update-progress/update.txt -- update
.agents/skills/verify-anyr/control-anyr cli --out artifacts/update-progress/check.txt -- update --check
ANYR_SPINNER_MS=20 ANYR_SPINNER_MIN_TICKS=6 \
  .agents/skills/verify-anyr/control-anyr pty run --timeout 6 --out artifacts/update-progress/tty.txt -- update
.agents/skills/verify-anyr/control-anyr test -- --test cli update_prints_from_to_channel_and_success
.agents/skills/verify-anyr/control-anyr test -- --test cli update_spinner_animates_on_tty
.agents/skills/verify-anyr/control-anyr cleanup
```

## Results

| Artifact | Exit | Observable |
| --- | --- | --- |
| `update.txt` | 0 | `Updating v0.1.11 -> v0.2.0-beta.1 (beta channel)`, `✔ Would update to v0.2.0-beta.1`, `Run anyr to start using the new version.` Fixture mode does not replace the binary. |
| `check.txt` | 0 | labeled `current:` / `latest:` / `channel: beta` and `update available` |
| `tty.txt` | 0 | PTY transcript: 10 carriage returns, 6 distinct spinner glyphs (`⠋⠙⠹⠸⠼⠴`), from→to, `beta channel`, success checkmark, restart hint |
| `frames.txt` | — | binary counts for `tty.txt` (animation proof) |

Complementary suite: `update_prints_from_to_channel_and_success` and `update_spinner_animates_on_tty` passed. Lib tests `spinner::tests::live_spinner_writes_multiple_changing_frames` and `upgrade::tests::updating_line_shows_from_to_and_channel` passed.

No `anyr claude` without `--dry-run`. No paid tokens. No live GitHub Releases.

## Cleanup

`control-anyr cleanup` removed the isolated workdir and left this `artifacts/update-progress/` tree in place.
