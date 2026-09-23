# claude-statusline

[![CI](https://github.com/JairoTorregrosa/claude-statusline/actions/workflows/ci.yml/badge.svg)](https://github.com/JairoTorregrosa/claude-statusline/actions/workflows/ci.yml)

A fast statusline for [Claude Code](https://code.claude.com). Written in Rust.

<img src="assets/statusline.svg" alt="claude-statusline: three lines with repo state, model and context, and rate limits" width="675">

## Anatomy

Each segment helps you make one decision about the current session. The
numbers below match the image.

<img src="assets/anatomy.svg" alt="the same render with numbered callouts under each segment" width="675">

| # | Segment | Purpose |
|---|---|---|
| 1 | Repo name | Shows the repository of this session. Click the name to open the repository. |
| 2 | Branch | Shows the current branch. Shows `[wt]` when the session runs in a linked worktree. |
| 3 | Git counts | Shows `+staged ~modified ?untracked` and `↑ahead ↓behind`. |
| 4 | PR state | Shows `✓` approved, `✗` changes requested, `○` pending, `◌` draft. Click the number to open the pull request. |
| 5 | Model | Shows the model of this session. |
| 6 | Effort | Shows the reasoning effort. Effort changes cost and quality. The `⚡fast` and `¬think` badges appear when these modes differ from the default. |
| 7 | Context | Shows reported input tokens and a window derived from the reported ceiling and settings when both are available. `⚠compact` warns near that displayed window; actual compaction can differ. |
| 8 | Cost | Shows the session cost in USD. |
| 9 | Session name | Identifies this session when many sessions run in parallel. A name longer than 36 characters truncates. |
| 10 | Rate limits | Shows the 5-hour and the 7-day windows. The percentage shows the usage. The `↻` time shows when the window opens again. |
| 11 | Tokens | Sums fresh input, cache writes, cache reads, and output recorded in this session's transcript. |
| 12 | Last commit | Shows the subject of `HEAD`. |
| 13 | MCP count | Counts servers with tools or instructions loaded in this session's transcript. |
| 14 | Skill count | Shows the latest full skill listing in this session's transcript. |
| 15 | Sessions | Counts top-level session transcripts written in the last 60 seconds on this machine. This is a recent-activity estimate. |

Line 4 shows deviations only. MCP and skill counts come from this session's
transcript; the session count is machine-wide. Pending MCP servers do not
count until tools or instructions are loaded.
A zero value does not render. The full line does not render when the counts
are zero and fewer than two recent session transcripts are found.

## Design

The statusline renders from the JSON payload that Claude Code writes to
stdin. The hot path starts no processes and makes no network calls.

Git refresh runs `git status --porcelain=v2 --branch` and `git log -1` for
the last commit subject, at most once each per 4 seconds per repository.
Each repository has its own cache entry.

The pull-request state, the repository identity, and the worktree name come
from the payload. The statusline does not call `gh`.

When `~/.claude/settings.json` is not valid JSON, the context segment
shows a red `cfg!` marker. If the payload omits `context_window_size`,
the segment shows only its reported token count, without a denominator
or percentage.

The transcript scan is incremental. The statusline stores a byte offset for
each session and reads only the new lines. One pass reads at most 4 MB, so a
large backlog does not block a render. The scan catches up across renders.
The same scan counts the loaded MCP servers and skills and sums the API
usage records into the session token total. The total renders only after
the scan catches up and all four usage counters are available; repeated
message IDs use their latest complete usage. The machine-wide session
count is a directory walk with no process spawns.

[DESIGN.md](DESIGN.md) lists the full design rules. [ROADMAP.md](ROADMAP.md)
lists the known limits and the planned work.

## Configuration

Configuration is optional. The file is
`~/.config/claude-statusline/config.json`. Only the ambient segments have
switches. The core lines render for everyone.

```json
{
  "inventory": true,
  "sessions": true
}
```

`inventory` controls the transcript segments: the MCP count, the skill
count, and the token total. `sessions` controls the session count. A
missing file selects the defaults (all on). An invalid file selects the
defaults and writes a warning to stderr.

## Performance

Measured on an M-series MacBook (macOS, warm filesystem cache), release
build:

| Path | Time |
|---|---|
| Warm (cache hit) | ~10 ms |
| Cold (git refresh) | ~33 ms |

Claude Code debounces statusline renders at 300 ms. Both paths are much
faster than that interval.

## Install

Requirements: `git` on `PATH`. A build from source requires Rust 1.88 or
later.

### From a release

1. Download the archive for your platform from the
   [releases page](https://github.com/JairoTorregrosa/claude-statusline/releases).
   macOS and Linux ship a `.tar.gz`; Windows ships a `.zip`.
2. Extract the binary to `~/.local/bin`, or `%USERPROFILE%\.local\bin` on
   Windows.
3. Set `statusLine` in `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "/absolute/path/to/.local/bin/claude-statusline"
  }
}
```

On Windows the value is the absolute path to `claude-statusline.exe`, written
with forward slashes. Claude Code runs the command through Git Bash when Git
for Windows is installed and through PowerShell otherwise; in Git Bash a
backslash is an escape character, and forward slashes are accepted by both:

```json
{
  "statusLine": {
    "type": "command",
    "command": "C:/Users/you/.local/bin/claude-statusline.exe"
  }
}
```

If your user name contains a space or another character a shell interprets
(`'`, `&`, `(`, `$`, …), the path must be quoted, and the two shells quote
differently. `install.ps1` detects which one applies and writes the right
form; when editing by hand, use single quotes — they are literal in both
shells — and the shell's own escape for a quote inside the name:

| Claude Code runs commands through | `command` value |
|---|---|
| Git Bash (Git for Windows installed) | `'C:/Users/Jane Doe/.local/bin/claude-statusline.exe'` |
| PowerShell (no Git for Windows) | `& 'C:/Users/Jane Doe/.local/bin/claude-statusline.exe'` |

Claude Code keeps its Windows settings under `%USERPROFILE%\.claude`, and
`install.ps1` writes there. The binary, however, resolves its home directory
from `HOME` first and `USERPROFILE` second, so if you set `HOME` to something
other than your profile directory the binary reads `autoCompactWindow` and
counts sessions from a `.claude` directory Claude Code never writes. Leave
`HOME` unset, or set it to `%USERPROFILE%`.

### From source

```sh
git clone https://github.com/JairoTorregrosa/claude-statusline
cd claude-statusline
./install.sh
```

On Windows, run the PowerShell installer instead:

```powershell
git clone https://github.com/JairoTorregrosa/claude-statusline
cd claude-statusline
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

Either script builds the binary and installs it to `~/.local/bin`. The script
writes a backup of `~/.claude/settings.json` and points `statusLine` at the
binary. The script prints the previous value so you can roll back. Both
scripts refuse to touch a `settings.json` that does not parse.

### With an agent

Give a coding agent this prompt:

> Clone https://github.com/JairoTorregrosa/claude-statusline and read AGENTS.md.
> Follow the install task in that file: verify the preconditions, run the steps, check the postconditions, and report the rollback value.

[AGENTS.md](AGENTS.md) gives the agent preconditions to verify, idempotent
steps, postconditions to check, and rollback rules.

## Development

```sh
cargo test          # parser, worktree and submodule resolution,
                    # payload nullability, render output
cargo clippy --all-targets -- -D warnings
```

To capture the payload that your Claude Code version sends, add one line to
any statusline script:

```sh
tee -a /tmp/statusline-payload.jsonl > /dev/null
```

A captured example lives in
[docs/sample-payload.json](docs/sample-payload.json).

## Governance

A pull request declares what produced it: the models, the harness, the
tokens, the cost. Four lines, checked by the `AGM` workflow.
[GOVERNANCE.md](GOVERNANCE.md) explains why that is the whole rule;
[agm.json](agm.json) is the machine-readable form. Correctness is CI's
job, review is Codex's, and the decision to merge is the maintainer's.

## Releases

A push of a tag that matches `v*` builds the binaries for macOS and Linux,
computes checksums, and publishes a GitHub release.

## License

MIT or Apache-2.0, at your option.
