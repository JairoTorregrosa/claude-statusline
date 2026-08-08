# Design

This document lists the rules that govern the statusline. Changes must obey
these rules. [AGENTS.md](AGENTS.md) turns them into a checklist for coding
agents.

## Purpose

The statusline helps you make decisions about the current Claude Code
session. Each segment answers one question about the state of the session.
A segment that does not support a decision does not ship.

## Rules

### The payload is the primary source

Claude Code writes a JSON payload to stdin on each render. The hot path
reads this payload and the local caches. The hot path starts no processes
and makes no network calls.
[docs/sample-payload.json](docs/sample-payload.json) shows the payload
shape.

### The renderer is pure

`src/render.rs` has no filesystem access and no process access.
`src/main.rs` gathers the external data and passes it to the renderer in one
value. This split keeps the render logic testable with plain values.

### External reads sit behind caches

| Source | Provides | Cost control |
|---|---|---|
| `git status --porcelain=v2 --branch` | branch, counts, last commit | one call each 4 s for each repository |
| `~/.claude/settings.json` | auto-compact window | read on each render (small file) |
| session transcript | MCP servers, skills, token totals | stored byte offset, 4 MB for each pass, 5 s TTL |
| `~/.claude/projects` | active session count | directory walk, 10 s TTL |

Cache entries live under `~/.cache/claude-statusline/`. Each cache key
includes the repository path or the transcript path, so sessions do not
contaminate each other.

### Degradation is loud

A missing or invalid input never produces an invented value.

- Invalid `~/.claude/settings.json`: the context segment measures against
  the model ceiling and shows a red `cfg!` marker.
- An inventory that cannot be complete: the segment does not render.
- A state file with an unknown schema: the parse fails and the scan
  restarts from offset 0.

A silent default hides a failure far from its cause. A visible marker
reports the failure at its source.

### Render deviations only

A zero count does not render. A badge for a default mode does not render.
An empty line does not render.

### Performance budget

A warm render completes in about 10 ms and must stay under 15 ms. A cold
render (git refresh) completes in about 33 ms. Claude Code debounces
renders at 300 ms.

### Tests are first-class

Each payload field is an `Option`. Each new field has a test with the field
absent. Edge cases have tests: truncated transcripts, partial trailing
lines, schema changes in state files, concurrent repositories.

CI enforces `cargo fmt --check`, `cargo clippy --all-targets -- -D
warnings`, and `cargo test` on Linux and macOS.
