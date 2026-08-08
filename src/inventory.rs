//! Loaded MCP servers, skills, and total tokens for this session, from
//! the transcript.
//!
//! Why "loaded" and not "used": every connected MCP server and every
//! available skill injects definitions into the context on every turn.
//! The count is a context-weight and capability inventory — it explains
//! the ctx number and surfaces bloat. A used-but-not-loaded state cannot
//! exist; a loaded-but-never-used server is exactly the waste you want
//! to see.
//!
//! Token totals: each assistant record carries `message.usage` with the
//! API token counts for one response (fresh input, cache writes, cache
//! reads, output; an absent counter counts as zero). One response writes
//! several consecutive records — one per content block — with the same
//! `message.id` and identical usage, so the first record for an id
//! counts and repeats are skipped. A record without an id cannot be
//! deduplicated and is not counted. The sum over distinct ids is the
//! session total; it renders only once the scan has reached the end of
//! the transcript (`caught_up`), because a partial sum is not a total.
//! A line longer than one pass budget is discarded to its newline; when
//! its head classifies as relevant, the total is marked `lossy` and does
//! not render — a lower bound is not a total either.
//!
//! The transcript path embeds the session id, so a path is never reused
//! by another session; offset state cannot leak across sessions.
//!
//! Sources (attachment records in the transcript JSONL):
//! - `skill_listing` with `isInitial: true` → full listing; `skillCount`
//!   is the loaded-skill count. Later full listings override earlier ones
//!   (plugin reloads change the inventory mid-session).
//! - `deferred_tools_delta` → `addedNames`/`readdedNames`/`removedNames`
//!   applied in order; `mcp__<server>__<tool>` names yield the loaded
//!   server set. `pendingMcpServers` names servers awaiting schemas.
//! - `mcp_instructions_delta` → servers whose instructions are injected.
//!
//! Known limit: a server whose tools are neither deferred nor carrying
//! instructions never appears in a delta. The count can undercount such
//! minimal setups; the segment then stays absent rather than guessing.
//!
//! Cost model: a per-transcript state file stores the processed byte
//! offset, one pass caps at `MAX_SCAN_BYTES` with progress saved,
//! truncation resets the state, and a partial trailing line is never
//! consumed.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use crate::cache;

const TTL: Duration = Duration::from_secs(5);
const MAX_SCAN_BYTES: usize = 4 * 1024 * 1024;

/// All fields are required on purpose: a state file from an older schema
/// fails the parse and the scan self-heals from offset 0.
#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct Inventory {
    pub offset: u64,
    /// Inside a line longer than one pass budget: discard bytes until the
    /// next newline before parsing resumes.
    pub skipping: bool,
    /// The scan reached the end of the transcript on the last pass. Token
    /// totals render only when true.
    pub caught_up: bool,
    /// A discarded oversized line classified as relevant: the token total
    /// is a lower bound, not a total, and must not render.
    pub lossy: bool,
    /// Latest full-listing skill count. None until a listing is seen.
    pub skill_count: Option<u64>,
    /// Live deferred `mcp__*` tool names (adds minus removes).
    pub mcp_tools: HashSet<String>,
    /// Server names from instruction deltas and pending-schema lists.
    pub mcp_servers: HashSet<String>,
    /// Session-total tokens: fresh input + cache writes + cache reads +
    /// output, summed over distinct assistant message ids.
    pub total_tokens: u64,
    /// Id of the last counted usage record. Records repeat per content
    /// block with identical usage: the first one counts.
    pub last_usage_id: Option<String>,
}

impl Inventory {
    pub fn mcp_count(&self) -> u64 {
        let mut servers: HashSet<&str> = self
            .mcp_tools
            .iter()
            .filter_map(|t| mcp_server_name(t))
            .collect();
        servers.extend(self.mcp_servers.iter().map(String::as_str));
        servers.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.mcp_count() == 0 && self.skill_count.is_none() && self.total_tokens == 0
    }
}

pub fn collect(transcript: &Path) -> Option<Inventory> {
    let state_path = cache::dir().join(format!(
        "inventory-{}.json",
        cache::key_hash(&transcript.to_string_lossy())
    ));

    if let Some(raw) = cache::read_fresh(&state_path, TTL)
        && let Ok(state) = serde_json::from_str::<Inventory>(&raw)
    {
        return Some(state);
    }

    let mut state = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Inventory>(&raw).ok())
        .unwrap_or_default();

    scan(transcript, &mut state, MAX_SCAN_BYTES).ok()?;

    if let Ok(raw) = serde_json::to_string(&state) {
        cache::write(&state_path, &raw);
    }
    Some(state)
}

fn scan(path: &Path, state: &mut Inventory, max_bytes: usize) -> std::io::Result<()> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    if state.offset > len {
        *state = Inventory::default();
    }
    if state.offset == len {
        state.caught_up = !state.skipping;
        return Ok(());
    }
    state.caught_up = false;

    f.seek(SeekFrom::Start(state.offset))?;
    let mut buf = Vec::with_capacity(max_bytes.min((len - state.offset) as usize));
    f.take(max_bytes as u64).read_to_end(&mut buf)?;

    // Finish discarding an oversized line before parsing resumes.
    let mut start = 0usize;
    if state.skipping {
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => {
                start = i + 1;
                state.offset += start as u64;
                state.skipping = false;
            }
            None => {
                state.offset += buf.len() as u64;
                return Ok(());
            }
        }
    }

    let chunk = &buf[start..];
    match chunk.iter().rposition(|&b| b == b'\n') {
        Some(idx) => {
            let complete = idx + 1;
            for line in String::from_utf8_lossy(&chunk[..complete]).lines() {
                apply_line(line, state);
            }
            state.offset += complete as u64;
        }
        None => {
            // A line that fills the whole pass budget without a newline is
            // longer than the budget: consume it unparsed instead of
            // stalling forever. Anything shorter is a partial trailing
            // line — wait for the rest.
            if chunk.len() == max_bytes {
                // Head-classify before discarding: dropping a relevant
                // record turns the total into a lower bound.
                if contains_seq(head_of(chunk), br#""role":"assistant""#)
                    || contains_seq(head_of(chunk), br#""attachment":"#)
                {
                    state.lossy = true;
                }
                state.offset += chunk.len() as u64;
                state.skipping = true;
            }
        }
    }
    state.caught_up = state.offset == len && !state.skipping;
    Ok(())
}

/// The record keys that classify a line sit near its start (measured
/// maximum: byte 163); tool output quoted inside `content` sits later and
/// arrives with escaped quotes, so it cannot spoof these tokens.
const HEAD_BYTES: usize = 512;

fn head_of(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.len().min(HEAD_BYTES)]
}

fn contains_seq(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn head_contains(line: &str, needle: &[u8]) -> bool {
    contains_seq(head_of(line.as_bytes()), needle)
}

/// Cheap prefilter — most transcript lines carry neither inventory
/// records nor usage. Multi-megabyte tool-output records fail the head
/// test and are never JSON-parsed. The prefilter only saves parses; the
/// full parse still validates the record shape.
fn interesting(line: &str) -> bool {
    (head_contains(line, br#""role":"assistant""#) && line.contains(r#""usage""#))
        || (head_contains(line, br#""attachment":"#)
            && (line.contains("skill_listing")
                || line.contains("deferred_tools_delta")
                || line.contains("mcp_instructions_delta")))
}

fn apply_line(line: &str, state: &mut Inventory) {
    if !interesting(line) {
        return;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return; // malformed line: skip, never abort the scan
    };
    if v.get("type").and_then(|t| t.as_str()) == Some("assistant")
        && let Some(msg) = v.get("message")
        && let Some(usage) = msg.get("usage")
    {
        apply_usage(msg.get("id").and_then(|i| i.as_str()), usage, state);
        return;
    }
    let Some(att) = v.get("attachment") else {
        return;
    };
    match att.get("type").and_then(|t| t.as_str()) {
        Some("skill_listing") => {
            // Only full listings carry the whole inventory; partial updates
            // list changed entries and must not overwrite the count.
            if att.get("isInitial").and_then(|b| b.as_bool()) == Some(true)
                && let Some(n) = att.get("skillCount").and_then(|n| n.as_u64())
            {
                state.skill_count = Some(n);
            }
        }
        Some("deferred_tools_delta") => {
            for key in ["addedNames", "readdedNames"] {
                for name in str_items(att, key) {
                    if name.starts_with("mcp__") {
                        state.mcp_tools.insert(name.to_string());
                    }
                }
            }
            for name in str_items(att, "removedNames") {
                state.mcp_tools.remove(name);
            }
            for server in str_items(att, "pendingMcpServers") {
                state.mcp_servers.insert(server.to_string());
            }
        }
        Some("mcp_instructions_delta") => {
            for server in str_items(att, "addedNames") {
                state.mcp_servers.insert(server.to_string());
            }
            for server in str_items(att, "removedNames") {
                state.mcp_servers.remove(server);
            }
        }
        _ => {}
    }
}

fn apply_usage(id: Option<&str>, usage: &serde_json::Value, state: &mut Inventory) {
    // First record for an id wins; a record without an id cannot be
    // deduplicated and is not counted.
    let Some(id) = id.filter(|i| !i.is_empty()) else {
        return;
    };
    if state.last_usage_id.as_deref() == Some(id) {
        return;
    }
    let n = [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "output_tokens",
    ]
    .iter()
    .filter_map(|k| usage.get(k).and_then(|x| x.as_u64()))
    .fold(0u64, u64::saturating_add);
    state.total_tokens = state.total_tokens.saturating_add(n);
    state.last_usage_id = Some(id.to_string());
}

fn str_items<'a>(att: &'a serde_json::Value, key: &str) -> impl Iterator<Item = &'a str> {
    att.get(key)
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str())
}

/// `mcp__claude-in-chrome__navigate` → `claude-in-chrome`.
fn mcp_server_name(tool: &str) -> Option<&str> {
    let rest = tool.strip_prefix("mcp__")?;
    let server = rest.split("__").next()?;
    (!server.is_empty()).then_some(server)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn skill_listing(count: u64, initial: bool) -> String {
        format!(
            r#"{{"type":"attachment","attachment":{{"type":"skill_listing","isInitial":{initial},"skillCount":{count},"names":[]}}}}"#
        )
    }

    fn tools_delta(added: &[&str], removed: &[&str], pending: &[&str]) -> String {
        let j = |v: &[&str]| serde_json::to_string(v).unwrap();
        format!(
            r#"{{"type":"attachment","attachment":{{"type":"deferred_tools_delta","addedNames":{},"readdedNames":[],"removedNames":{},"pendingMcpServers":{}}}}}"#,
            j(added),
            j(removed),
            j(pending)
        )
    }

    fn instr_delta(added: &[&str], removed: &[&str]) -> String {
        let j = |v: &[&str]| serde_json::to_string(v).unwrap();
        format!(
            r#"{{"type":"attachment","attachment":{{"type":"mcp_instructions_delta","addedNames":{},"removedNames":{}}}}}"#,
            j(added),
            j(removed)
        )
    }

    fn tmpfile(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("csl-inv-{}-{name}", std::process::id()))
    }

    #[test]
    fn latest_full_skill_listing_wins() {
        let mut s = Inventory::default();
        apply_line(&skill_listing(65, true), &mut s);
        apply_line(&skill_listing(15, false), &mut s); // partial update: ignored
        assert_eq!(s.skill_count, Some(65));
        apply_line(&skill_listing(56, true), &mut s); // plugin reload
        assert_eq!(s.skill_count, Some(56));
    }

    #[test]
    fn mcp_servers_from_tools_instructions_and_pending_dedup() {
        let mut s = Inventory::default();
        apply_line(
            &tools_delta(
                &[
                    "mcp__chrome__navigate",
                    "mcp__chrome__find",
                    "mcp__codex__run",
                    "WebSearch",
                ],
                &[],
                &["exa"],
            ),
            &mut s,
        );
        // chrome appears in instructions too — one server, not two.
        apply_line(&instr_delta(&["chrome"], &[]), &mut s);
        assert_eq!(s.mcp_count(), 3, "chrome, codex, exa: {s:?}");
    }

    #[test]
    fn removed_tools_remove_their_server() {
        let mut s = Inventory::default();
        apply_line(
            &tools_delta(&["mcp__linear__create", "mcp__linear__list"], &[], &[]),
            &mut s,
        );
        assert_eq!(s.mcp_count(), 1);
        apply_line(&tools_delta(&[], &["mcp__linear__create"], &[]), &mut s);
        assert_eq!(s.mcp_count(), 1, "one tool left, server still loaded");
        apply_line(&tools_delta(&[], &["mcp__linear__list"], &[]), &mut s);
        assert_eq!(s.mcp_count(), 0, "all tools gone, server unloaded");
    }

    #[test]
    fn instruction_removal_unloads_server() {
        let mut s = Inventory::default();
        apply_line(&instr_delta(&["metavr"], &[]), &mut s);
        assert_eq!(s.mcp_count(), 1);
        apply_line(&instr_delta(&[], &["metavr"]), &mut s);
        assert_eq!(s.mcp_count(), 0);
    }

    #[test]
    fn malformed_and_irrelevant_lines_are_skipped() {
        let mut s = Inventory::default();
        apply_line("skill_listing but not json", &mut s);
        apply_line(
            r#"{"type":"user","message":"mentions skill_listing"}"#,
            &mut s,
        );
        apply_line(
            r#"{"type":"attachment","attachment":{"type":"hook_success"}}"#,
            &mut s,
        );
        assert!(s.is_empty());
    }

    #[test]
    fn old_schema_state_fails_parse_and_self_heals() {
        let old = r#"{"offset":123,"mcp":["chrome"],"skills":["imagegen"]}"#;
        assert!(serde_json::from_str::<Inventory>(old).is_err());
        // A state file without token fields must also fail and self-heal.
        let prior = r#"{"offset":9,"skill_count":3,"mcp_tools":[],"mcp_servers":[]}"#;
        assert!(serde_json::from_str::<Inventory>(prior).is_err());
        // The immediate predecessor schema (token fields, no scan flags)
        // must fail too.
        let prior = r#"{"offset":9,"skill_count":3,"mcp_tools":[],"mcp_servers":[],"total_tokens":7,"last_usage":["m1",7]}"#;
        assert!(serde_json::from_str::<Inventory>(prior).is_err());
    }

    fn usage_line(id: &str, inp: u64, cw: u64, cr: u64, out: u64) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"id":"{id}","role":"assistant","usage":{{"input_tokens":{inp},"cache_creation_input_tokens":{cw},"cache_read_input_tokens":{cr},"output_tokens":{out}}}}}}}"#
        )
    }

    #[test]
    fn usage_accumulates_across_messages() {
        let mut s = Inventory::default();
        apply_line(&usage_line("m1", 2, 100, 900, 40), &mut s);
        apply_line(&usage_line("m2", 3, 50, 1000, 60), &mut s);
        assert_eq!(s.total_tokens, 1042 + 1113);
        assert!(!s.is_empty());
    }

    #[test]
    fn repeated_message_id_counts_once() {
        // One API response writes one record per content block, all with
        // the same message id and identical usage.
        let mut s = Inventory::default();
        for _ in 0..3 {
            apply_line(&usage_line("m1", 2, 100, 900, 40), &mut s);
        }
        assert_eq!(s.total_tokens, 1042);
        apply_line(&usage_line("m2", 0, 0, 0, 8), &mut s);
        assert_eq!(s.total_tokens, 1050);
    }

    #[test]
    fn first_record_for_an_id_wins() {
        // A malformed or truncated repeat must not disturb the first
        // record's contribution.
        let mut s = Inventory::default();
        apply_line(&usage_line("m1", 2, 100, 900, 10), &mut s);
        apply_line(
            r#"{"type":"assistant","message":{"id":"m1","role":"assistant","usage":{"output_tokens":40}}}"#,
            &mut s,
        );
        assert_eq!(s.total_tokens, 1012);
    }

    #[test]
    fn record_without_id_is_not_counted() {
        let mut s = Inventory::default();
        apply_line(
            r#"{"type":"assistant","message":{"role":"assistant","usage":{"output_tokens":40}}}"#,
            &mut s,
        );
        assert_eq!(s.total_tokens, 0, "no id, no dedup, no count");
    }

    #[test]
    fn usage_outside_assistant_records_is_ignored() {
        let mut s = Inventory::default();
        apply_line(
            r#"{"type":"user","message":{"content":"tool output quoting \"usage\""}}"#,
            &mut s,
        );
        apply_line(r#"{"type":"summary","usage":{"output_tokens":5}}"#, &mut s);
        // Quoted keys arrive escaped; a spoofed head plus real top-level
        // type still fails the envelope check in the full parse.
        apply_line(
            r#"{"role":"assistant","type":"user","message":{"usage":{"output_tokens":9}}}"#,
            &mut s,
        );
        assert!(s.is_empty());
    }

    #[test]
    fn oversized_line_is_skipped_and_scan_recovers() {
        let p = tmpfile("oversized");
        let good1 = usage_line("m1", 0, 0, 0, 100);
        let giant = format!("{{\"type\":\"noise\",\"pad\":\"{}\"}}", "x".repeat(4096));
        let good2 = usage_line("m2", 0, 0, 0, 11);
        std::fs::write(&p, format!("{good1}\n{giant}\n{good2}\n")).unwrap();

        let cap = 1024; // smaller than the giant line
        let mut s = Inventory::default();
        for _ in 0..12 {
            scan(&p, &mut s, cap).unwrap();
        }
        assert_eq!(s.total_tokens, 111, "usage on both sides of the giant");
        assert!(s.caught_up, "scan must not wedge on an oversized line");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn oversized_relevant_line_marks_the_total_lossy() {
        let p = tmpfile("lossy");
        let good = usage_line("m1", 0, 0, 0, 100);
        let giant = format!(
            "{{\"type\":\"assistant\",\"message\":{{\"id\":\"mx\",\"role\":\"assistant\",\"content\":\"{}\",\"usage\":{{\"output_tokens\":9}}}}}}",
            "x".repeat(4096)
        );
        std::fs::write(&p, format!("{good}\n{giant}\n")).unwrap();

        let cap = 1024;
        let mut s = Inventory::default();
        for _ in 0..12 {
            scan(&p, &mut s, cap).unwrap();
        }
        assert!(s.caught_up);
        assert!(s.lossy, "a dropped relevant record is a lost contribution");
        assert_eq!(s.total_tokens, 100, "the giant's usage is not recoverable");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn backlog_is_not_caught_up_until_scanned_to_the_end() {
        let p = tmpfile("backlog");
        let line = usage_line("m1", 0, 0, 0, 7);
        let mut f = std::fs::File::create(&p).unwrap();
        for _ in 0..10 {
            writeln!(f, "{line}").unwrap();
        }
        f.sync_all().unwrap();

        let mut s = Inventory::default();
        scan(&p, &mut s, line.len() * 2).unwrap();
        assert!(!s.caught_up, "one pass over a backlog is not the total");
        while !s.caught_up {
            scan(&p, &mut s, line.len() * 2).unwrap();
        }
        assert_eq!(s.total_tokens, 7, "same id throughout: one contribution");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn non_mcp_deferred_tools_are_ignored() {
        let mut s = Inventory::default();
        apply_line(
            &tools_delta(&["WebSearch", "TaskCreate", "Monitor"], &[], &[]),
            &mut s,
        );
        assert_eq!(s.mcp_count(), 0);
        assert!(s.is_empty());
    }

    #[test]
    fn incremental_scan_reads_only_the_delta() {
        let p = tmpfile("incremental");
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, "{}", skill_listing(10, true)).unwrap();
        f.sync_all().unwrap();

        let mut s = Inventory::default();
        scan(&p, &mut s, MAX_SCAN_BYTES).unwrap();
        assert_eq!(s.skill_count, Some(10));
        let first = s.offset;

        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        writeln!(f, "{}", skill_listing(12, true)).unwrap();
        scan(&p, &mut s, MAX_SCAN_BYTES).unwrap();
        assert_eq!(s.skill_count, Some(12));
        assert!(s.offset > first);

        let stable = s.offset;
        scan(&p, &mut s, MAX_SCAN_BYTES).unwrap();
        assert_eq!(s.offset, stable, "no growth, no work");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn partial_trailing_line_is_not_consumed() {
        let p = tmpfile("partial");
        let full = skill_listing(7, true);
        let (head, tail) = full.split_at(full.len() / 2);
        std::fs::write(&p, head).unwrap();

        let mut s = Inventory::default();
        scan(&p, &mut s, MAX_SCAN_BYTES).unwrap();
        assert_eq!(s.skill_count, None);
        assert_eq!(s.offset, 0, "offset must not advance past a partial line");

        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        writeln!(f, "{tail}").unwrap();
        scan(&p, &mut s, MAX_SCAN_BYTES).unwrap();
        assert_eq!(s.skill_count, Some(7));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn truncated_file_resets_state() {
        let p = tmpfile("truncate");
        std::fs::write(&p, format!("{}\n", skill_listing(9, true))).unwrap();
        let mut s = Inventory::default();
        scan(&p, &mut s, MAX_SCAN_BYTES).unwrap();
        assert_eq!(s.skill_count, Some(9));

        std::fs::write(&p, "{}\n").unwrap(); // shrank below offset
        scan(&p, &mut s, MAX_SCAN_BYTES).unwrap();
        assert_eq!(s.skill_count, None, "state from the old file must reset");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn chunk_cap_makes_progress_across_passes() {
        let p = tmpfile("chunked");
        let line = skill_listing(3, true);
        let mut f = std::fs::File::create(&p).unwrap();
        for _ in 0..10 {
            writeln!(f, "{line}").unwrap();
        }
        f.sync_all().unwrap();
        let len = std::fs::metadata(&p).unwrap().len();

        let cap = line.len() * 3;
        let mut s = Inventory::default();
        for _ in 0..10 {
            scan(&p, &mut s, cap).unwrap();
        }
        assert_eq!(s.offset, len, "must catch up across capped passes");
        assert_eq!(s.skill_count, Some(3));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn server_name_extraction() {
        assert_eq!(
            mcp_server_name("mcp__claude-in-chrome__navigate"),
            Some("claude-in-chrome")
        );
        assert_eq!(
            mcp_server_name("mcp__claude_ai_Exa__web_search_exa"),
            Some("claude_ai_Exa")
        );
        assert_eq!(mcp_server_name("mcp____x"), None);
        assert_eq!(mcp_server_name("Bash"), None);
    }
}
