//! Count of active Claude Code sessions on this machine.
//!
//! A live session appends to its transcript continuously, so "active" is
//! observable as a recent mtime on `~/.claude/projects/*/*.jsonl`. This is
//! a pure filesystem walk: readdir + stat, no process spawns. The result
//! is machine-global, so the cache entry is too (10s TTL).

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::cache;

const TTL: Duration = Duration::from_secs(10);
/// A transcript touched within this window counts as an active session.
const ACTIVE_WINDOW: Duration = Duration::from_secs(60);

pub fn active_count() -> Option<u64> {
    let cache_path = cache::dir().join("sessions");
    if let Some(raw) = cache::read_fresh(&cache_path, TTL) {
        return raw.trim().parse().ok();
    }
    let n = count(projects_dir()?, SystemTime::now())?;
    cache::write(&cache_path, &n.to_string());
    Some(n)
}

fn projects_dir() -> Option<PathBuf> {
    cache::home_dir().map(|h| h.join(".claude").join("projects"))
}

fn count(projects: PathBuf, now: SystemTime) -> Option<u64> {
    let mut n = 0;
    for project in std::fs::read_dir(projects).ok()? {
        let Ok(project) = project else { continue };
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            if now
                .duration_since(modified)
                .is_ok_and(|age| age <= ACTIVE_WINDOW)
            {
                n += 1;
            }
        }
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use filetime_shim::set_old_mtime;

    mod filetime_shim {
        use std::path::Path;
        /// Set the file mtime to a moment in the past with
        /// `File::set_times`, without a process spawn.
        pub fn set_old_mtime(path: &Path, secs_ago: u64) {
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago);
            let f = std::fs::File::options().write(true).open(path).unwrap();
            let times = std::fs::FileTimes::new().set_modified(t);
            f.set_times(times).unwrap();
        }
    }

    #[test]
    fn counts_only_recent_jsonl_files() {
        let base = std::env::temp_dir().join(format!("csl-sessions-{}", std::process::id()));
        let proj = base.join("-Users-x-code");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("fresh.jsonl"), "x").unwrap();
        std::fs::write(proj.join("stale.jsonl"), "x").unwrap();
        std::fs::write(proj.join("not-a-transcript.txt"), "x").unwrap();
        set_old_mtime(&proj.join("stale.jsonl"), 3600);

        assert_eq!(count(base.clone(), SystemTime::now()), Some(1));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_dir_is_none() {
        assert_eq!(
            count(PathBuf::from("/nonexistent-csl-test"), SystemTime::now()),
            None
        );
    }
}
