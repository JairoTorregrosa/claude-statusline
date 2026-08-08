//! Claude Code statusline.
//!
//! Architecture (payload-first):
//!   hot path     — parse stdin JSON, render. No spawns, no network.
//!   warm path    — git via per-repo TTL cache; refresh = one porcelain-v2
//!                  spawn + one `git log -1`, at most every 4s per repo.
//!   ambient path — loaded MCP/skill inventory and session token totals
//!                  from the transcript (incremental, offset-tracked scan)
//!                  and active-session count (mtime walk). Both cached,
//!                  both optional.
//!
//! Stdout is the product surface; diagnostics go to stderr.

mod cache;
mod config;
mod git;
mod inventory;
mod payload;
mod render;
mod sessions;
mod settings;
mod timefmt;

use std::io::Read;
use std::path::Path;

fn main() {
    let mut input = String::new();
    let _ = std::io::stdin().lock().read_to_string(&mut input);

    let p: payload::Payload = match serde_json::from_str(&input) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("claude-statusline: unparseable payload: {e}");
            payload::Payload::default()
        }
    };

    let cfg = config::load();
    let ext = render::External {
        git: p.cwd().and_then(|c| git::collect(Path::new(c))),
        compact_limit: settings::compact_limit(),
        inventory: cfg
            .inventory
            .then(|| {
                p.transcript_path
                    .as_deref()
                    .and_then(|t| inventory::collect(Path::new(t)))
            })
            .flatten()
            .filter(|inv| !inv.is_empty()),
        active_sessions: cfg.sessions.then(sessions::active_count).flatten(),
    };
    print!("{}", render::render(&p, &ext));
}
