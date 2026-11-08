# Proof: help-and-version

Feature: `help-and-version`
Entry points driven: `anyr --help`, non-TTY empty argv, `anyr --version`, `anyr auth --help`, `anyr claude --help`, `anyr task --help`, `anyr nope`
Harness: `.agents/skills/verify-anyr/control-anyr` (`launch`, `doctor`, `cli`, `test`, `cleanup`)
Binary: `target/debug/anyr` from this checkout (`0.1.11`)
Isolated home: `/tmp/anyr-verify-20260901-170042-5328/home` (removed by cleanup)

## Commands

```bash
.agents/skills/verify-anyr/control-anyr launch
.agents/skills/verify-anyr/control-anyr doctor
.agents/skills/verify-anyr/control-anyr cli --out artifacts/help-and-version/help.txt -- --help
.agents/skills/verify-anyr/control-anyr cli --out artifacts/help-and-version/no-args.txt --
.agents/skills/verify-anyr/control-anyr cli --out artifacts/help-and-version/version.txt -- --version
.agents/skills/verify-anyr/control-anyr cli --out artifacts/help-and-version/auth-help.txt -- auth --help
.agents/skills/verify-anyr/control-anyr cli --out artifacts/help-and-version/claude-help.txt -- claude --help
.agents/skills/verify-anyr/control-anyr cli --out artifacts/help-and-version/task-help.txt -- task --help
.agents/skills/verify-anyr/control-anyr cli --out artifacts/help-and-version/unknown.txt -- nope
.agents/skills/verify-anyr/control-anyr test -- --test cli help_lists_login_claude_account_and_spawn_targets
.agents/skills/verify-anyr/control-anyr cleanup
```

## Results

| Artifact | Exit | Observable |
| --- | --- | --- |
| `help.txt` | 0 | `CORE COMMANDS`, `LAUNCH`, `auth`, `claude`, `▀█████████▄`, `Sign in if needed`; no `setup.sh` / `npx @anyr/cli` |
| `no-args.txt` | 0 | same grouped help on non-TTY empty argv |
| `version.txt` | 0 | starts with `0.1.11` |
| `auth-help.txt` | 0 | `login`, `logout`, `status`, `token`, `switch` |
| `claude-help.txt` | 0 | `--dry-run`, `--ok` |
| `task-help.txt` | 0 | `not yet in the native CLI` |
| `unknown.txt` / `.err` | 1 | `Unknown command "nope"` and `anyr --help` |

Complementary suite: `help_lists_login_claude_account_and_spawn_targets` passed.

No `anyr claude` without `--dry-run`. No paid tokens.

## Cleanup

`control-anyr cleanup` removed the isolated workdir and left this `artifacts/help-and-version/` tree in place.
