# Instructions for coding agents

This file tells a coding agent how to install, verify, and modify this
project safely. If you are a human, read [README.md](README.md).

## Task: install the statusline for the user

### Preconditions — verify, do not assume

1. Run `cargo --version`. Require 1.88 or later. If Rust is missing, stop
   and tell the user to install it from https://rustup.rs.
2. Run `git --version`. Require any version.
3. Confirm `~/.claude/settings.json` parses as JSON if it exists. If it
   does not parse, STOP. Report the parse error to the user. Do not edit a
   broken file.

### Steps — idempotent, safe to re-run

1. `cargo build --release`
2. `install -d ~/.local/bin && install -m 755 target/release/claude-statusline ~/.local/bin/claude-statusline`
3. Read `~/.claude/settings.json`. Record the current `statusLine` value.
   You will report it to the user as rollback information.
4. Set only this key. Preserve every other key in the file:

```json
{
  "statusLine": {
    "type": "command",
    "command": "/absolute/path/to/home/.local/bin/claude-statusline"
  }
}
```

Use the absolute home path. Do not write `~` inside the JSON value.

### Postconditions — verify before you report success

1. `echo '{}' | ~/.local/bin/claude-statusline` exits 0 and prints `ctx:--`.
2. `~/.local/bin/claude-statusline < docs/sample-payload.json` prints 3
   lines. Line 2 contains `Fable 5` and `ctx:`.
3. `python3 -c "import json; json.load(open('$HOME/.claude/settings.json'))"`
   exits 0.

### Report to the user

- The previous `statusLine` value (or "none"), so the user can roll back.
- The one-line rollback instruction: restore that value in settings.json.

### Prohibitions

- Do not use sudo.
- Do not modify any settings key other than `statusLine`.
- Do not overwrite `settings.json` wholesale. Merge.
- Do not delete or rename an existing statusline script. It is the
  rollback.

## Task: contribute a change

1. Read [agm.json](agm.json). Compute the risk zone: for each changed
   file, take the highest-severity zone whose pattern matches it; the
   change's zone is the highest across all files.
2. Prepare the evidence package in the pull-request body with the
   sections agm.json requires for that zone. Start from
   `.github/PULL_REQUEST_TEMPLATE.md`.
3. State every external assumption (payload shape, transcript schema,
   settings keys, git output) and how you verified it against real data.
   Declare what you could not verify. An unverified assumption stated as
   fact is a governance failure, not a shortcut.
4. For high and critical zones: STOP before you submit. Show the human
   the diff and the package. Ask the human to check the confirmation
   box. Do not check it yourself.
5. Never claim maintainer approval. The `AGM` check passing is not
   approval; the maintainer's review is.
6. Disclose your tool and model in the PR body. Keep the
   `Co-Authored-By` trailer on commits.

## Task: modify the code

- Obey the rules in [DESIGN.md](DESIGN.md).
- Run `cargo test` and `cargo clippy --all-targets -- -D warnings` before
  you report done. CI enforces both, plus `cargo fmt --check`.
- The renderer (`src/render.rs`) is pure: no filesystem access, no process
  access. Gather external data in `main.rs` and pass it in.
- Every new payload field must be an `Option` and must have a test with the
  field absent.
- The hot path must not add process spawns or network calls. Cache external
  reads under `~/.cache/claude-statusline/` with a TTL. Key each cache
  entry to prevent cross-session contamination.
