# AGENTS.md

Native Rust CLI (`anyr`) for the AnyRouter gateway. One crate, lib plus bin. Stay on 0.1.x.

## Commands

| Task | Command |
| --- | --- |
| Test | `cargo test --locked --all-targets` |
| Clippy | `cargo clippy --locked --all-targets` |
| Format | `cargo fmt --check` |
| Size | `python scripts/bench.py measure --bin target/release/anyr --asset anyr-linux-x86_64 --kind native --out /tmp/anyr-bench.json` (stripped linux x86_64 ≤ `LINUX_X86_64_BUDGET_BYTES` in `scripts/bench.py`, now 3,584,500 bytes; each timed command ≤ 5 ms median) |

Version lock is also enforced in `tests/release_lock.rs`.

## Invariants (do not break)

- Keep `Cargo.toml`, `package.json`, and `.release-please-manifest.json` on 0.1.x. `release-please-config.json` uses `"versioning": "always-bump-patch"`, so `feat!` still bumps patch only.
- Never auto-merge release-please PRs. A human merges them.
- Do not rename fields in `~/.anyrouter/config.yaml`. The TypeScript CLI reads the same file (`relay_token`, `relay_device_id`, and the rest).
- Never print full API keys (`sk-ar-v1-…`) or relay tokens (`rk_…`). Mask with `mask_api_key`. Tests such as `upgrade_does_not_print_full_sk_ar_key` assert redaction.
- Keep wasm building with `--no-default-features`. New deps on the non-native path must compile there.

## Conventions

- Conventional commits. `.githooks/prepare-commit-msg` appends two Co-authored-by trailers. Opt out with `ANYR_SKIP_COAUTHORS=1`. Enable with `./scripts/install-hooks.sh`.
- `.githooks/pre-push` runs `cargo fmt --check` and clippy. Skip with `ANYR_SKIP_PREPUSH=1`.
- CI runs tests with `cargo nextest run` (profile in `.config/nextest.toml`). `cargo test` still works locally.
- Error strings are human sentences. Use `map_err` plus `format!` and name the failing subject.
- Unit tests live in `#[cfg(test)]` next to the module. Integration tests spawn `env!("CARGO_BIN_EXE_anyr")` with a fresh `ANYROUTER_HOME`.

## Gotchas

- `ratatui = "=0.29.0"` and `crossterm = "=0.28.1"` are exact-pinned on purpose.
- npm `@anyr/cli@latest` is still the old JS CLI. Native publishes use `--tag next` until a human retags.
- Beta prereleases come from the `ci.yml` matrix on every push to `main`.
