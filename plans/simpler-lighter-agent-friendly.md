# anyr: simpler, lighter, agent-friendly — measurements and plan

Status: shipped. Recorded 2026-11-08 from `origin/main` (`ffcb4a9`, release
`v0.1.38`). This is the durable record for AnyRouterHQ/cli#192; the issue
itself holds the original measurements and design.

## Goal

Make `anyr` simpler to use, lighter, and easy for AI agents to drive. The
work landed in three parts: size (#191), agent mode, and help/UX.

## Measurements (original, v0.1.30-beta.486 → now, v0.1.38)

**Size** (stripped linux x86_64, budget 4,194,304 bytes / 4.0 MiB):

| | bytes | share of budget |
|---|---:|---:|
| v0.1.30-beta.486 (thin LTO, opt s) | 4,027,144 | 96% |
| now (fat LTO, opt z) | 3,600,232 | 85.8% |

Fat LTO with `opt-level = "z"` (the #191 size change) bought back ~11% of
the budget. Feature-gating was measured at ~2.6% and declined, so no
command is feature-gated.

**Startup** (median, release binary): `--version` 1.21 ms, `--help` 1.19 ms,
`commands --json` 2.24 ms. Startup is negligible; there is nothing to gain
here, so no cold-start work is planned.

**Code size of hot paths** (lines, `wc -l`):

| file | lines |
|---|---:|
| src/http.rs | 1438 |
| src/help.rs | 1184 |
| src/artifact.rs | 870 |
| src/publish.rs | 630 |
| src/registry.rs | 379 |
| src/parse.rs | 373 |
| src/commands.rs | 364 |
| src/exit.rs | 281 |
| src/lib.rs | 47 |
| src/main.rs | 13 |

The big files are help text registry and the HTTP layer, not startup or
dispatch. No hot-path refactor is planned; if one of these grows past the
point of hurting review, split the module then.

## What shipped, mapped to the design

1. **Simpler (help PR).** `anyr --help` is a short page (an everyday group
   plus an agents pointer); `help commands` is generated from one registry
   (`dispatch::COMMANDS`, enforced complete by tests) and grouped: sign in ·
   chat and models · launch · relay · publish · account · setup, with
   onboard shortcuts listed once under `onboard`. The full machine map is
   `anyr commands --json` (schema `anyr.commands/v1`), which lists 49
   commands including `chat`, `relay`, `task`, `delegate` and `upgrade`
   (the old map left those five out). No command was removed.

2. **Lightweight (#191).** Fat LTO, `opt-level = "z"`, `strip = true`,
   `panic = "abort"` in the release profile (see Cargo.toml). `--version`
   and HTTPS commands show no measurable change.

3. **Agent mode (agent PR).**
   - `--json` / `ANYR_OUTPUT=json` and `--quiet` / `ANYR_QUIET=1` work on
     every command; in JSON mode each error is one
     `{"error":{code,exit,message,hint}}` line on stderr.
   - One exit table in `src/exit.rs`: **0 ok, 1 error, 2 usage, 3 not found,
     4 auth, 5 rate limited.** `tests/json_contract.rs` runs every command
     and subcommand from the registry with `--json` against a local server.
   - `chat --json` / `--no-stream`, `-` for stdin, and stdin when piped.
   - `help agents` prints `docs/agents.md` (the agent guide README points
     at too).
   - No update notices under `--json`, `--quiet`, or without a TTY;
     `NO_COLOR` honoured, colour off when stdout is not a TTY.
   - Verified 2026-11-08 against the release binary: `account --json`,
     `help --json`, `apps --json`, `chat "…" --json`, `chat --json` on
     piped stdin all behave per the contract; `completion`, `delegate`,
     `chat` and `artifact` without arguments return JSON usage errors with
     exit 2 (not a `--json` rejection). `byok add --donate` without a TTY
     now says what to do ("re-run on a terminal or pass --yes") instead of
     reading EOF and printing "Cancelled".

## Decision recorded: exit codes 3 and 4

The original spec asked for 3 = auth, 4 = not found. The CLI had already
shipped and documented the reverse (3 = not found, 4 = auth) in help text
and ~8 tests. The agent-mode PR kept the shipped mapping and added
5 = rate limited. No owner request to swap ever arrived, and the mapping
is now load-bearing in the JSON contract, so **3 = not found, 4 = auth
stands**. Swapping would be a one-line change in `src/exit.rs` plus tests
and help text if an owner explicitly wants it.

## Deliberate leftovers (not bugs)

- `pool` still means the Poolside launcher, `pool stats`, `relay pool` and
  `byok --donate`. Renaming would break scripts; the registry groups them.
- `api` still duplicates `keys`, `usage`, `logs`, `presets`,
  `transactions` and `mcp connections` as a raw escape hatch.
- Three publish paths remain: `artifact publish`, `publish`, `apps`.
- Delete/list verbs still differ per command (`keys revoke` vs `byok rm`;
  `list` vs `ls`). The design removed nothing, and these stay as aliases
  in the registry rather than being renamed.

## Re-measuring

`python scripts/bench.py measure --bin target/release/anyr --asset
anyr-linux-x86_64 --kind native --out /tmp/anyr-bench.json`, or simply
check the stripped size against the 4,194,304-byte budget in AGENTS.md.
