# Roadmap

## Context segment: session-effective auto-compact window

A session resolves its auto-compact window when it starts. The context
segment reads `~/.claude/settings.json` on each render. When the file
changes while a session runs, the segment and the session disagree. The
segment can show more than 100% while the session does not compact.

Candidate work, ordered by preference:

1. Render a `⚠over` marker above 100%. The marker reports the overflow and
   does not promise a compaction.
2. Detect a settings change that is newer than the session start. Mark the
   denominator as possibly stale. The transcript start time and the
   settings file mtime make this detectable locally.
3. Request an `effective_auto_compact_window` field in the statusline
   payload upstream. Use the field when it is present.
4. Detect compaction events in the transcript and reconcile the gauge. This
   option requires verification of the transcript record shape.

## Inventory: token weight

The MCP count shows distinct servers. Servers differ in tool count and in
context cost. Candidate work: show the deferred tool count for each server,
or estimate the token weight of the injected definitions.

## Inventory: minimal servers

A server whose tools are not deferred and that injects no instructions does
not appear in the transcript deltas. The count can miss such a server.
Candidate work: find a second transcript signal for these servers.

## Distribution

- Publish the crate to crates.io. The package metadata is ready.
