//! File-based TTL cache under ~/.cache/claude-statusline/.
//!
//! Writes are atomic (temp file + rename) because several Claude Code
//! sessions render concurrently and may share a cache entry. Entries are
//! keyed by the caller (e.g. per-repo hash) — never a single global file,
//! so state cannot leak across sessions.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Cross-platform home: `HOME` (unix) with a `USERPROFILE` fallback (windows).
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn dir() -> PathBuf {
    let home = home_dir().unwrap_or_else(std::env::temp_dir);
    let d = home.join(".cache").join("claude-statusline");
    let _ = fs::create_dir_all(&d);
    d
}

/// Contents of `path` if it exists and is younger than `ttl`.
pub fn read_fresh(path: &Path, ttl: Duration) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    (age <= ttl).then(|| fs::read_to_string(path).ok())?
}

pub fn write(path: &Path, contents: &str) {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    if fs::write(&tmp, contents).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// FNV-1a — tiny, dependency-free stable hash for cache keys.
pub fn key_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_distinct() {
        assert_eq!(key_hash("/a/b"), key_hash("/a/b"));
        assert_ne!(key_hash("/a/b"), key_hash("/a/c"));
    }

    #[test]
    fn fresh_roundtrip_and_expiry() {
        let p = std::env::temp_dir().join(format!("csl-test-{}", std::process::id()));
        write(&p, "hello");
        assert_eq!(
            read_fresh(&p, Duration::from_secs(60)).as_deref(),
            Some("hello")
        );
        assert_eq!(read_fresh(&p, Duration::ZERO), None);
        let _ = fs::remove_file(&p);
    }
}
