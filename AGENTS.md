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

   On Windows the binary is `target\release\claude-statusline.exe`. Copy it to
   `%USERPROFILE%\.local\bin\claude-statusline.exe`, or run `install.ps1`,
   which performs every step in this section.
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

On Windows, write the path with forward slashes
(`C:/Users/you/.local/bin/claude-statusline.exe`). Claude Code hands the
value to Git Bash when Git for Windows is installed and to PowerShell
otherwise; a backslash is an escape character in Git Bash.

If the path contains a space or any character outside `[A-Za-z0-9_./:-]`, it
must be quoted, and the two shells disagree on how. Determine which shell
applies — Git for Windows present means Git Bash — and write:

- Git Bash: `'C:/Users/Jane Doe/.local/bin/claude-statusline.exe'`, with a
  quote inside the name written as `'\''`.
- PowerShell: `& 'C:/Users/Jane Doe/.local/bin/claude-statusline.exe'`, with a
  quote inside the name written as `''`.

An unquoted path with a space fails silently in both shells. `install.ps1`
performs this detection; prefer running it.

Two Windows-specific hazards apply when a tool other than `install.ps1`
rewrites the file:

- `ConvertTo-Json` defaults to `-Depth 2` and silently replaces deeper
  structures with a placeholder string. `settings.json` nests further than
  that. Pass `-Depth 100`.
- Windows PowerShell 5.1 writes UTF-8 with a BOM through `Set-Content` and
  `Out-File`. A BOM makes strict JSON parsers reject the file. Write the
  bytes with a BOM-less encoder.

### Postconditions — verify before you report success

1. `echo '{}' | ~/.local/bin/claude-statusline` exits 0 and prints `ctx:--`.
2. `~/.local/bin/claude-statusline < docs/sample-payload.json` prints 3
   lines. Line 2 contains `Fable 5` and `ctx:`.
3. `python3 -c "import json; json.load(open('$HOME/.claude/settings.json'))"`
   exits 0.

On Windows, use `claude-statusline.exe` in the paths above and `python`
instead of `python3`. The expected output is identical on all platforms.

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

1. Describe the change in the pull-request body, then declare what
   produced it. Four lines, from
   `.github/PULL_REQUEST_TEMPLATE.md`:

   ```
   - Model: every model that wrote part of this, comma-separated
   - Harness: the tool they ran in, with a version when you have one
   - Tokens: total for the session, rounded is fine
   - Cost: USD; a flat-rate subscription with no metered spend is 0
   ```

   Declare the numbers your harness actually reports. A number it does
   not report is `unknown`. Never invent one — a fabricated cost is the
   only way to fail this rule.
2. State an external assumption (payload shape, transcript schema,
   settings keys, git output) where you relied on one, and say how you
   checked it. This is not a gate; it is the context a reviewer cannot
   reconstruct from the diff.
3. Keep the `Co-Authored-By` trailer on commits.
4. Wait for the Codex review before you ask for a merge. Its findings
   are not checks and do not appear in `gh pr checks`.
5. Never claim maintainer approval. A green `AGM` check is not approval,
   and neither is a Codex review with no findings.

## Code Review Rules

These rules govern the automated review. Review as the person who has
this statusline on screen all day, not as a style checker. A finding
names a payload, terminal, settings or repository state that produces a
wrong or missing segment, and says what the user sees when it happens.
Without that state, there is no finding.

### A wrong number is worse than no number

The worst outcome this project has is an invented number rendered as
fact. A segment that declines to render is acceptable. Flag any path
where a missing or malformed input becomes a number on screen: a
denominator that falls back to a constant, a partial sum presented as a
total, a percentage derived from an absent field, an arithmetic
saturation standing in for a real count.

Safe path: return `None`, drop the segment, or show the loud marker
(`cfg!`) that [DESIGN.md](DESIGN.md) prescribes.

### The payload belongs to Claude Code, not to us

`src/payload.rs` models JSON this project does not own and cannot
version. Flag a new field that is not an `Option`, a field read without
its absent case covered, a `deny_unknown_fields`, and any statement a
comment makes about the payload that the repository cannot back —
`docs/sample-payload.json` is the only captured evidence of the shape.
A comment's phrasing is not a finding; a comment's false claim is.

Safe path: model the field as `Option`, add a test with it absent, and
declare what was not verified.

### The hot path renders every 300 ms

Flag a process spawn, a network call, an unbounded read or an uncached
filesystem walk added to the render path, and any cache key that does
not include the repository or transcript path — one session reading
another's numbers is a wrong number with extra steps.

Safe path: gather in `main.rs`, cache under
`~/.cache/claude-statusline/` with a TTL, keep `src/render.rs` pure.

### What not to report

CI already enforces formatting, clippy with `-D warnings`, and the test
suite on Linux, macOS and Windows. Do not spend a comment on formatting,
lint, naming, comment wording, test names, doc phrasing, or a refactor
that changes nothing the user sees. Do not report an input this project
cannot receive.

One finding that costs a user a wrong number beats five that cost a
reviewer their attention. When nothing meets that bar, say so and
approve.

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
