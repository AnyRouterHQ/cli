# Proof: model-picker

Feature: `model-picker`
Entry points driven: `anyr models --dump-tui`, `anyr models use anyrouter/auto`, `anyr claude --dry-run --yes --model stealth/ox-alpha[1m] --yolo`, `anyr claude --dry-run --yes` after pinning auto, `anyr menu --dump-tui`
Harness: `.agents/skills/verify-anyr/control-anyr` (`launch`, `doctor`, `cli`, `cleanup`)
Binary: `target/debug/anyr` from this checkout (`0.1.11`)
Isolated home: `/tmp/anyr-verify-20260903-060433-6302/home` (removed by cleanup)

## Commands

```bash
.agents/skills/verify-anyr/control-anyr launch
.agents/skills/verify-anyr/control-anyr doctor
.agents/skills/verify-anyr/control-anyr cli --out artifacts/model-picker/dump.txt -- models --dump-tui
.agents/skills/verify-anyr/control-anyr cli --out artifacts/model-picker/use-auto.txt -- models use anyrouter/auto
.agents/skills/verify-anyr/control-anyr cli --out artifacts/model-picker/launch-auto.txt -- claude --dry-run --yes --key sk-ar-v1-fixture-key-0001
.agents/skills/verify-anyr/control-anyr cli --out artifacts/model-picker/ox-alpha-yolo.txt -- claude --dry-run --yes --key sk-ar-v1-fixture-key-0001 --model 'stealth/ox-alpha[1m]' --yolo
.agents/skills/verify-anyr/control-anyr cli --out artifacts/model-picker/menu-after.txt -- menu --dump-tui
.agents/skills/verify-anyr/control-anyr cleanup
```

## Results

| Artifact | Exit | Observable |
| --- | --- | --- |
| `dump.txt` | 0 | picker frame with `◆ anyrouter/auto`; no `most used`; no usage-ranked catalog dump |
| `use-auto.txt` | 0 | `Saved  default model  anyrouter/auto` |
| `launch-auto.txt` | 0 | `ANTHROPIC_MODEL=anyrouter/auto`; `command:`; key redacted |
| `ox-alpha-yolo.txt` | 0 | `ANTHROPIC_MODEL=stealth/ox-alpha[1m]` and `"--dangerously-skip-permissions"` |
| `menu-after.txt` | 0 | `model    anyrouter/auto` on the launcher; no `most used` |

Complementary suite: `models_dump_tui_pins_anyrouter_auto_without_usage_dump`, `models_use_anyrouter_auto_persists_without_network`, `claude_yolo_with_ox_alpha_1m_still_works`, `picker_catalog_tests::*` passed.

No `anyr claude` without `--dry-run`. No paid tokens.

## Cleanup

`control-anyr cleanup` removed the isolated workdir and left this `artifacts/model-picker/` tree in place.
