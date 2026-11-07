# Accounts and config

Accounts and config let a user see which profile is active, switch between saved profiles, find the config file, and log out — all against this machine's AnyRouter config, not the operator's home, when driven through the helper.

## Sub-features

- `whoami` shows who is signed in (name and email, or why they are unavailable), the account, a masked key and the default model.
- `config-path` prints the config file path.
- `account-use` switches `active_profile` to a named account.
- `logout` clears the stored key for the active account without printing it.

## How to get to it (user POV)

- Run `anyr whoami` or `anyr auth status`.
- Run `anyr config path`.
- Run `anyr account use <name>` (README: `anyr account use work`).
- Run `anyr logout` or `anyr auth logout`.
- Open `anyr config` on a TTY for the settings screen (see [TUI launcher](./tui-launcher.md)).

## Driving it with control-anyr

Preconditions:

- `control-anyr doctor` is clean.
- Seed config has profiles `default` (active) and `work`.
- Isolated `CONFIG_FILE` from `control-anyr path CONFIG_FILE`.

- **Whoami.** Run `control-anyr cli --out artifacts/accounts-and-config/whoami.txt -- whoami`. Exit code `0`. Stdout starts with `Signed in` and has an `account` row with `default` (the identity lookup is offline-safe: it times out after 3s and prints why name and email are missing). Stdout does not contain `sk-ar-v1-verify-fixture-not-a-real-key`.
- **Config path.** Run `control-anyr cli --out artifacts/accounts-and-config/config-path.txt -- config path`. Exit code `0`. Stdout contains the isolated path from `control-anyr path CONFIG_FILE`.
- **Switch account.** Run `control-anyr cli --out artifacts/accounts-and-config/account-use.txt -- account use work`. Exit code `0`. Stdout contains `work`.
- **Confirm switch.** Run `control-anyr cli --out artifacts/accounts-and-config/whoami-work.txt -- whoami`. Stdout contains `work` and does not contain the full work fixture key.
- **Confirm file.** Copy `$(control-anyr path CONFIG_FILE)` to `artifacts/accounts-and-config/config-after-use.yaml`. The file has `active_profile: work` (or equivalent).
- **Restore default.** Run `control-anyr cli -- account use default`. Follow with `whoami` showing `default`.
- **Logout (optional last).** Only if no later recipe needs the seed key: `control-anyr cli --out artifacts/accounts-and-config/logout.txt -- logout`. Combined output does not contain `sk-ar-v1-verify-fixture-not-a-real-key`. Relogin is not required for help/onboard/dry-run-with-`--key`.
- **Proof.** Keep `whoami.txt`, `account-use.txt`, `whoami-work.txt`, and `config-after-use.yaml`. They show the same switch from the command output and from the file.

## Gotchas

- `login --key` validates the key against the gateway. Do not use it in this recipe; the seed file already has fixture keys.
- Dummy keys `sk-ar-v1-test` / `sk-ar-v1-testkey` / `sk-ar-v1-test-key` are treated as absent. Seed keys must not be those literals.
- `whoami --json` is the stable parse form if human columns shift. Keys: `account`, `user{name,email}`, `key_masked`, `default_model`, `config_path`, `base_url`, plus `identity_error` only when the lookup failed. A missing key exits 4.
- Non-TTY `anyr config` (no `--dump-tui`) prints status, not a picker. Do not expect `Pick 1-`.
- Never run these commands without `ANYROUTER_HOME` / `control-anyr`; they would rewrite `~/.anyrouter/config.yaml`.
