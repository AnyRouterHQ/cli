---
name: capture-bug
description: File a redacted anyr bug report when anyr fails in a way the task did not expect. Use when an `anyr` command exits non-zero unexpectedly, crashes, or prints output that contradicts `anyr schema`. Runs `anyr report --last`, checks it holds no secrets, and files it once per fingerprint.
---

# Capture an anyr bug

Use this when `anyr` misbehaves, not when the task itself failed.

File a report when:

- `anyr` exited non-zero and the task did not expect it. Expected failures are not bugs: a missing key (exit 4), a 429 (exit 5), a confirmation that needs `--yes` (exit 6), or a model that does not exist (exit 3).
- `anyr` crashed (`anyr crashed at ...` on stderr).
- The output contradicts `anyr schema <command>`: a missing field, a wrong type, an exit code that is not listed, or JSON where text was promised.

A launched agent's own exit code (`anyr claude`, `anyr codex`, ...) is the agent's, not anyr's. Do not report it.

## Steps

1. Read the bundle:

   ```bash
   anyr report --last --json
   ```

2. Check it holds no secrets. It must not contain any of these: a key or token (`sk-ar-`, `ak_`, `rk_`), a path, a file name, a username, a hostname, a prompt, a model id, a repo name, or file contents. The report is redacted, so this should hold. If anything slipped through, stop. Do not file the report. Tell the user what leaked instead.

3. Note the `fingerprint` field. File at most one report per fingerprint per session. If you already filed this fingerprint, stop.

4. Write a title of a few words that says what broke, for example `keys list --json missing name`. Never put a key, a prompt, a path or file contents in the title.

5. File it:

   ```bash
   anyr report --last --file --yes --title "<short title>"
   ```

   With `gh` installed, this comments on an open issue that has the same fingerprint, or opens a new issue labelled `captured`. Without `gh`, it prints a prefilled issue link. Give that link to the user to open.

6. Tell the user in one sentence what you were doing when `anyr` failed, and give them the issue URL. Keep that sentence free of secrets as well.

## Do not

- Do not file without `--file`. Plain `anyr report` only prints.
- Do not paste the report, logs, prompts or file contents into the title or into extra comments.
- Do not retry a failing `--file` in a loop. One attempt per fingerprint.
