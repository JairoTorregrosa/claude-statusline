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
//!   is the loaded-skill count and `names` the loaded skills. Later full
//!   listings override earlier ones. A listing without `isInitial` lists
//!   skills that became available mid-session (synced skills arrive this
//!   way); measured over local transcripts, some of its names repeat the
//!   full listing and some are new, so it is merged as a set union.
//! - `system` records with `subtype: "local_command"` for `/reload-skills`
//!   and `/reload-plugins`. Neither writes a new listing. `/reload-skills`
//!   prints the new total ("Reloaded skills: N skills available"), which
//!   replaces the count; the names are then unknown. `/reload-plugins`
//!   prints only the plugin share, so the count becomes unknown and the
//!   segment hides until the next full listing or `/reload-skills`.
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
use std::path::{Path, PathBuf};
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
    /// Latest full-listing skill count. None until a listing is seen, and
    /// None again after a reload that leaves the total unknown.
    pub skill_count: Option<u64>,
    /// Loaded skill names: the latest full listing plus the partial
    /// listings after it. None when a listing's names do not match its
    /// count, or after a reload (the new set is not in the transcript).
    /// `deserialize_with` makes the key required: serde would otherwise
    /// read a state file without it as None and skip the rescan.
    #[serde(deserialize_with = "Option::deserialize")]
    pub skill_names: Option<Vec<String>>,
    /// How many of `skill_names` are project skills, decided when the scan
    /// read each listing and kept with it: a later edit on disk does not
    /// move a loaded skill between local and global. None when the scope
    /// could not be verified or the names are unknown.
    #[serde(deserialize_with = "Option::deserialize")]
    pub skill_local: Option<u64>,
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

/// Where the session runs, for deciding which listed skills are project
/// skills. Read once per scan pass, not per render.
#[derive(Default)]
pub struct Scope {
    pub cwd: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

impl Scope {
    fn local_count(&self, names: &[String]) -> Option<u64> {
        local_skill_count(names, self.cwd.as_deref()?, self.home.as_deref())
    }
}

pub fn collect(transcript: &Path, scope: &Scope) -> Option<Inventory> {
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

    scan_in(transcript, &mut state, MAX_SCAN_BYTES, scope).ok()?;

    if let Ok(raw) = serde_json::to_string(&state) {
        cache::write(&state_path, &raw);
    }
    Some(state)
}

#[cfg(test)]
fn scan(path: &Path, state: &mut Inventory, max_bytes: usize) -> std::io::Result<()> {
    scan_in(path, state, max_bytes, &Scope::default())
}

fn scan_in(
    path: &Path,
    state: &mut Inventory,
    max_bytes: usize,
    scope: &Scope,
) -> std::io::Result<()> {
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
                apply_line_in(line, state, scope);
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
        || (head_contains(line, br#""subtype":"local_command""#)
            && (line.contains(r#""reload-skills""#) || line.contains(r#""reload-plugins""#)))
}

#[cfg(test)]
fn apply_line(line: &str, state: &mut Inventory) {
    apply_line_in(line, state, &Scope::default());
}

fn apply_line_in(line: &str, state: &mut Inventory, scope: &Scope) {
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
    if v.get("subtype").and_then(|t| t.as_str()) == Some("local_command") {
        apply_reload(&v, state);
        return;
    }
    let Some(att) = v.get("attachment") else {
        return;
    };
    match att.get("type").and_then(|t| t.as_str()) {
        Some("skill_listing") => {
            let names: Option<Vec<String>> = att
                .get("names")
                .and_then(|v| v.as_array())
                .map(|_| str_items(att, "names").map(str::to_string).collect());
            if att.get("isInitial").and_then(|b| b.as_bool()) == Some(true) {
                if let Some(n) = att.get("skillCount").and_then(|n| n.as_u64()) {
                    state.skill_count = Some(n);
                    state.skill_names = names.filter(|v| v.len() as u64 == n);
                    state.skill_local = state
                        .skill_names
                        .as_deref()
                        .and_then(|v| scope.local_count(v));
                }
            } else {
                // A partial listing adds to a known set. Without the set,
                // the overlap is unknowable and so is the total.
                match (state.skill_names.as_mut(), names) {
                    (Some(known), Some(added)) => {
                        let mut new: Vec<String> = Vec::new();
                        for name in added {
                            if !known.contains(&name) && !new.contains(&name) {
                                new.push(name);
                            }
                        }
                        state.skill_local = state
                            .skill_local
                            .zip(scope.local_count(&new))
                            .map(|(a, b)| a + b);
                        known.extend(new);
                        state.skill_count = Some(known.len() as u64);
                    }
                    _ => {
                        state.skill_names = None;
                        state.skill_local = None;
                        state.skill_count = None;
                    }
                }
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

/// `/reload-skills` and `/reload-plugins` change the loaded skills
/// without writing a listing. Whatever the reload prints, the old names no
/// longer describe the session.
fn apply_reload(v: &serde_json::Value, state: &mut Inventory) {
    let command = v
        .get("commandRun")
        .and_then(|c| c.get("command"))
        .and_then(|c| c.as_str());
    match command {
        Some("reload-skills") => {
            state.skill_names = None;
            state.skill_local = None;
            state.skill_count = v
                .get("content")
                .and_then(|c| c.as_str())
                .and_then(reloaded_skill_count);
        }
        Some("reload-plugins") => {
            state.skill_names = None;
            state.skill_local = None;
            state.skill_count = None;
        }
        _ => {}
    }
}

/// "…Reloaded skills: 52 skills available (12 removed)…" → 52. Any other
/// text is None: an unrecognized message leaves the count unknown.
fn reloaded_skill_count(content: &str) -> Option<u64> {
    let rest = content.split("Reloaded skills: ").nth(1)?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let after = &rest[digits.len()..];
    if !after.starts_with(" skills available") && !after.starts_with(" skill available") {
        return None;
    }
    digits.parse().ok()
}

/// How many of `names` are project skills for a session in `cwd`: a skill
/// directory under `.claude/skills` in `cwd` or an ancestor below `home`,
/// or a plugin skill whose plugin is enabled in that project's
/// `.claude/settings.json` or `.claude/settings.local.json`. Everything
/// else is global (user, plugin at user scope, synced, bundled). A name
/// that also exists in `~/.claude/skills` is global: the user skill takes
/// precedence over the project one, so that is the one loaded.
///
/// None when a project settings file exists but does not parse, or a name
/// is not a plain path segment: the split cannot be verified.
fn local_skill_count(names: &[String], cwd: &Path, home: Option<&Path>) -> Option<u64> {
    let roots: Vec<&Path> = cwd.ancestors().take_while(|d| Some(*d) != home).collect();

    let mut project_plugins: Vec<String> = Vec::new();
    for root in &roots {
        for file in ["settings.json", "settings.local.json"] {
            let path = root.join(".claude").join(file);
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
            if let Some(map) = v.get("enabledPlugins").and_then(|m| m.as_object()) {
                for (key, on) in map {
                    if on.as_bool() == Some(true)
                        && let Some((plugin, _)) = key.split_once('@')
                    {
                        project_plugins.push(plugin.to_string());
                    }
                }
            }
        }
    }

    let mut local = 0u64;
    for name in names {
        let is_local = match name.split_once(':') {
            Some((plugin, _)) => project_plugins.iter().any(|p| p == plugin),
            None => {
                if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
                    return None;
                }
                let has_skill = |r: &Path| {
                    r.join(".claude")
                        .join("skills")
                        .join(name)
                        .join("SKILL.md")
                        .is_file()
                };
                !home.is_some_and(has_skill) && roots.iter().any(|r| has_skill(r))
            }
        };
        if is_local {
            local += 1;
        }
    }
    Some(local)
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
        assert_eq!(s.skill_count, Some(65));
        apply_line(&skill_listing(56, true), &mut s);
        assert_eq!(s.skill_count, Some(56));
    }

    #[test]
    fn partial_listing_merges_into_known_names() {
        let mut s = Inventory::default();
        apply_line(&named_listing(&["a", "b"]), &mut s);
        let partial = |names: &[&str]| {
            format!(
                r#"{{"type":"attachment","attachment":{{"type":"skill_listing","isInitial":false,"skillCount":{},"names":{}}}}}"#,
                names.len(),
                serde_json::to_string(names).unwrap()
            )
        };
        apply_line(&partial(&["b", "c"]), &mut s); // b repeats, c is new
        assert_eq!(s.skill_count, Some(3));

        // Without known names the overlap is unknowable.
        apply_line(&skill_listing(65, true), &mut s); // names [] != 65
        assert_eq!(s.skill_names, None);
        apply_line(&partial(&["d"]), &mut s);
        assert_eq!(s.skill_count, None, "65 + 1 would be a guess");
    }

    fn reload(command: &str, stdout: &str) -> String {
        format!(
            r#"{{"type":"system","subtype":"local_command","content":"<local-command-stdout>{stdout}</local-command-stdout>","level":"info","commandRun":{{"command":"{command}","args":""}}}}"#
        )
    }

    fn named_listing(names: &[&str]) -> String {
        format!(
            r#"{{"type":"attachment","attachment":{{"type":"skill_listing","isInitial":true,"skillCount":{},"names":{}}}}}"#,
            names.len(),
            serde_json::to_string(names).unwrap()
        )
    }

    #[test]
    fn full_listing_keeps_names() {
        let mut s = Inventory::default();
        apply_line(&named_listing(&["a", "p:b"]), &mut s);
        assert_eq!(s.skill_count, Some(2));
        assert_eq!(s.skill_names, Some(vec!["a".into(), "p:b".into()]));
    }

    #[test]
    fn reload_skills_replaces_count_and_drops_names() {
        let mut s = Inventory::default();
        apply_line(&named_listing(&["a", "b", "c"]), &mut s);
        apply_line(
            &reload(
                "reload-skills",
                "Reloaded skills: 52 skills available (12 removed)",
            ),
            &mut s,
        );
        assert_eq!(s.skill_count, Some(52));
        assert_eq!(
            s.skill_names, None,
            "the new names are not in the transcript"
        );
    }

    #[test]
    fn unrecognized_reload_output_leaves_count_unknown() {
        let mut s = Inventory::default();
        apply_line(&named_listing(&["a"]), &mut s);
        apply_line(&reload("reload-skills", "Skills recargadas: 40"), &mut s);
        assert_eq!(s.skill_count, None, "a stale 1 must not survive a reload");
    }

    #[test]
    fn reload_plugins_makes_count_unknown_until_next_listing() {
        let mut s = Inventory::default();
        apply_line(&named_listing(&["a", "p:b"]), &mut s);
        apply_line(
            &reload(
                "reload-plugins",
                "Reloaded: 4 plugins · 24 skills · 10 agents",
            ),
            &mut s,
        );
        assert_eq!(s.skill_count, None);
        apply_line(&named_listing(&["a"]), &mut s);
        assert_eq!(s.skill_count, Some(1));
    }

    #[test]
    fn other_local_commands_are_ignored() {
        let mut s = Inventory::default();
        apply_line(&named_listing(&["a"]), &mut s);
        apply_line(&reload("model", "Set model to reload-skills"), &mut s);
        assert_eq!(s.skill_count, Some(1));
    }

    #[test]
    fn local_skills_are_project_dirs_and_project_plugins() {
        let base = tmpfile("scope");
        let home = base.join("home");
        let proj = home.join("code").join("proj");
        let sub = proj.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let skill = |root: &Path, n: &str| {
            let d = root.join(".claude").join("skills").join(n);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("SKILL.md"), "---\nname: x\n---\n").unwrap();
        };
        skill(&proj, "deploy");
        skill(&home, "global-one"); // ~/.claude/skills is user scope
        std::fs::write(
            proj.join(".claude").join("settings.local.json"),
            r#"{"enabledPlugins":{"team@corp":true,"off@corp":false}}"#,
        )
        .unwrap();
        let names: Vec<String> = [
            "deploy",
            "global-one",
            "team:lint",
            "off:x",
            "jai:y",
            "loop",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        assert_eq!(local_skill_count(&names, &sub, Some(&home)), Some(2));

        skill(&home, "deploy");
        assert_eq!(
            local_skill_count(&names, &sub, Some(&home)),
            Some(1),
            "a user skill with the same name wins over the project one"
        );

        std::fs::write(proj.join(".claude").join("settings.json"), "{broken").unwrap();
        assert_eq!(
            local_skill_count(&names, &sub, Some(&home)),
            None,
            "an unreadable project settings file makes the split unverifiable"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn split_is_kept_from_when_the_listing_was_read() {
        let base = tmpfile("snapshot");
        let home = base.join("home");
        let proj = home.join("proj");
        let d = proj.join(".claude").join("skills").join("deploy");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), "x").unwrap();
        let scope = Scope {
            cwd: Some(proj.clone()),
            home: Some(home.clone()),
        };
        let t = base.join("t.jsonl");
        std::fs::write(&t, format!("{}\n", named_listing(&["deploy", "loop"]))).unwrap();

        let mut s = Inventory::default();
        scan_in(&t, &mut s, MAX_SCAN_BYTES, &scope).unwrap();
        assert_eq!(s.skill_local, Some(1));

        // The skill file goes away while the session still has it loaded.
        std::fs::remove_dir_all(&d).unwrap();
        let mut f = std::fs::OpenOptions::new().append(true).open(&t).unwrap();
        writeln!(f, "{}", reload("model", "Set model")).unwrap();
        scan_in(&t, &mut s, MAX_SCAN_BYTES, &scope).unwrap();
        assert_eq!(
            s.skill_local,
            Some(1),
            "a disk edit must not move a loaded skill"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn partial_listing_adds_local_skills_of_new_names_only() {
        let base = tmpfile("partial-scope");
        let home = base.join("home");
        let proj = home.join("proj");
        for n in ["deploy", "lint"] {
            let d = proj.join(".claude").join("skills").join(n);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("SKILL.md"), "x").unwrap();
        }
        let scope = Scope {
            cwd: Some(proj),
            home: Some(home),
        };
        let mut s = Inventory::default();
        apply_line_in(&named_listing(&["deploy", "loop"]), &mut s, &scope);
        let partial = r#"{"type":"attachment","attachment":{"type":"skill_listing","isInitial":false,"skillCount":2,"names":["deploy","lint"]}}"#;
        apply_line_in(partial, &mut s, &scope);
        assert_eq!(s.skill_count, Some(3));
        assert_eq!(s.skill_local, Some(2), "deploy repeats; only lint is new");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn listing_without_scope_has_no_split() {
        let mut s = Inventory::default();
        apply_line(&named_listing(&["a"]), &mut s);
        assert_eq!(s.skill_count, Some(1));
        assert_eq!(s.skill_local, None);
    }

    #[test]
    fn path_like_skill_name_makes_split_unknown() {
        let names = vec!["../escape".to_string()];
        assert_eq!(local_skill_count(&names, Path::new("/tmp"), None), None);
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
        // A state file from before `skill_names` must fail, or the names
        // never arrive for a transcript already scanned to its end.
        let prior = r#"{"offset":9,"skipping":false,"caught_up":true,"lossy":false,"skill_count":3,"mcp_tools":[],"mcp_servers":[],"total_tokens":7,"last_usage_id":null}"#;
        assert!(serde_json::from_str::<Inventory>(prior).is_err());
        let prior = r#"{"offset":9,"skipping":false,"caught_up":true,"lossy":false,"skill_count":3,"skill_names":["a"],"mcp_tools":[],"mcp_servers":[],"total_tokens":7,"last_usage_id":null}"#;
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
