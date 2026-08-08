//! Git segment data.
//!
//! Hot-path cost: repo discovery is a pure-filesystem walk (zero process
//! spawns), then a per-repo cache read. Only when the cache entry is older
//! than `TTL` do we refresh with ONE `git status --porcelain=v2 --branch`
//! spawn (branch + ahead/behind + staged/modified/untracked in a single
//! invocation — the gitstatusd lesson: the cost is spawning git eight
//! times, not computing status once) plus one `git log -1` for the subject.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::cache;

const TTL: Duration = Duration::from_secs(4);
const COMMIT_MAX_CHARS: usize = 36;

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct GitInfo {
    pub repo_name: String,
    pub is_worktree: bool,
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub last_commit: String,
}

/// Result of the spawn-free `.git` walk.
struct Discovery {
    root: PathBuf,
    /// Contents of `.git` when it is a file (linked worktree), None when dir.
    dot_git_file: Option<String>,
}

fn discover(cwd: &Path) -> Option<Discovery> {
    for dir in cwd.ancestors() {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some(Discovery {
                root: dir.to_path_buf(),
                dot_git_file: None,
            });
        }
        if dot_git.is_file() {
            let contents = std::fs::read_to_string(&dot_git).ok()?;
            return Some(Discovery {
                root: dir.to_path_buf(),
                dot_git_file: Some(contents),
            });
        }
    }
    None
}

/// Main-repo display name. In a linked worktree the `.git` file points at
/// `<main>/.git/worktrees/<name>` — show `<main>`'s basename, not the
/// worktree folder.
fn repo_name(d: &Discovery) -> (String, bool) {
    let fallback = || basename(&d.root);
    match &d.dot_git_file {
        None => (fallback(), false),
        Some(contents) => {
            let gitdir = contents
                .lines()
                .find_map(|l| l.strip_prefix("gitdir:"))
                .map(str::trim);
            // A submodule also has a `.git` FILE, but it points into the
            // superproject's `.git/modules/` — that is not a worktree.
            if gitdir.is_some_and(|g| g.contains("/modules/")) {
                return (fallback(), false);
            }
            let name = gitdir
                .and_then(|gitdir| {
                    let main = gitdir.split("/worktrees/").next()?;
                    let main = main.strip_suffix("/.git").unwrap_or(main);
                    let base = basename(Path::new(main));
                    let base = base.strip_suffix(".git").unwrap_or(&base).to_string();
                    (!base.is_empty()).then_some(base)
                })
                .unwrap_or_else(fallback);
            (name, true)
        }
    }
}

fn basename(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn collect(cwd: &Path) -> Option<GitInfo> {
    let d = discover(cwd)?;
    let cache_path = cache::dir().join(format!(
        "git-{}.json",
        cache::key_hash(&d.root.to_string_lossy())
    ));

    if let Some(raw) = cache::read_fresh(&cache_path, TTL)
        && let Ok(info) = serde_json::from_str::<GitInfo>(&raw)
    {
        return Some(info);
    }

    let info = refresh(cwd, &d)?;
    if let Ok(raw) = serde_json::to_string(&info) {
        cache::write(&cache_path, &raw);
    }
    Some(info)
}

fn refresh(cwd: &Path, d: &Discovery) -> Option<GitInfo> {
    let status_out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
        ])
        .output()
        .ok()?;
    if !status_out.status.success() {
        return None;
    }
    let mut info = parse_porcelain_v2(&String::from_utf8_lossy(&status_out.stdout));

    let (name, is_worktree) = repo_name(d);
    info.repo_name = name;
    info.is_worktree = is_worktree;

    // Empty repos (no HEAD yet) make `git log` fail — subject just stays empty.
    if let Ok(log_out) = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["log", "-1", "--format=%s"])
        .output()
        && log_out.status.success()
    {
        info.last_commit = truncate_chars(
            String::from_utf8_lossy(&log_out.stdout).trim(),
            COMMIT_MAX_CHARS,
        );
    }
    Some(info)
}

/// Drops control characters (a subject can contain ESC or tabs) before
/// the length check — the statusline is line-oriented ANSI output.
fn truncate_chars(s: &str, max: usize) -> String {
    let clean: String = s.chars().filter(|c| !c.is_control()).collect();
    if clean.chars().count() <= max {
        clean
    } else {
        let mut t: String = clean.chars().take(max).collect();
        t.push('…');
        t
    }
}

/// Parse `git status --porcelain=v2 --branch`.
///
/// Header lines: `# branch.head <name>`, `# branch.ab +A -B`.
/// Entries: `1 XY ...` / `2 XY ...` (X = index state, Y = worktree state),
/// `u ...` (unmerged), `? <path>` (untracked).
fn parse_porcelain_v2(text: &str) -> GitInfo {
    let mut info = GitInfo::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            info.branch = if rest == "(detached)" {
                "detached".into()
            } else {
                rest.into()
            };
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            for tok in rest.split_whitespace() {
                if let Some(n) = tok.strip_prefix('+') {
                    info.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = tok.strip_prefix('-') {
                    info.behind = n.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            let mut chars = line.chars().skip(2);
            let x = chars.next().unwrap_or('.');
            let y = chars.next().unwrap_or('.');
            if x != '.' {
                info.staged += 1;
            }
            if y != '.' {
                info.modified += 1;
            }
        } else if line.starts_with("u ") {
            info.modified += 1;
        } else if line.starts_with("? ") {
            info.untracked += 1;
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_repo_with_upstream() {
        let out = "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +0 -0\n";
        let i = parse_porcelain_v2(out);
        assert_eq!(i.branch, "main");
        assert_eq!(
            (i.ahead, i.behind, i.staged, i.modified, i.untracked),
            (0, 0, 0, 0, 0)
        );
    }

    #[test]
    fn parses_dirty_repo() {
        let out = concat!(
            "# branch.head feat/x\n",
            "# branch.ab +2 -1\n",
            "1 M. N... 100644 100644 100644 a b staged.rs\n",
            "1 .M N... 100644 100644 100644 a b modified.rs\n",
            "1 MM N... 100644 100644 100644 a b both.rs\n",
            "2 R. N... 100644 100644 100644 a b R100 new.rs\told.rs\n",
            "u UU N... 100644 100644 100644 100644 a b c conflicted.rs\n",
            "? untracked-1.txt\n",
            "? untracked-2.txt\n",
        );
        let i = parse_porcelain_v2(out);
        assert_eq!(i.branch, "feat/x");
        assert_eq!(i.ahead, 2);
        assert_eq!(i.behind, 1);
        assert_eq!(i.staged, 3); // staged.rs + both.rs + rename
        assert_eq!(i.modified, 3); // modified.rs + both.rs + conflicted
        assert_eq!(i.untracked, 2);
    }

    #[test]
    fn detached_head_is_labelled() {
        let i = parse_porcelain_v2("# branch.oid abc\n# branch.head (detached)\n");
        assert_eq!(i.branch, "detached");
    }

    #[test]
    fn no_upstream_means_zero_ab() {
        let i = parse_porcelain_v2("# branch.head main\n");
        assert_eq!((i.ahead, i.behind), (0, 0));
    }

    #[test]
    fn worktree_repo_name_resolves_to_main_repo() {
        let d = Discovery {
            root: PathBuf::from("/w/myrepo-wt1"),
            dot_git_file: Some("gitdir: /home/u/code/myrepo/.git/worktrees/wt1\n".into()),
        };
        let (name, wt) = repo_name(&d);
        assert_eq!(name, "myrepo");
        assert!(wt);
    }

    #[test]
    fn submodule_is_not_a_worktree() {
        // Submodules also have a `.git` FILE, pointing into the
        // superproject's `.git/modules/` — must not render as [wt].
        let d = Discovery {
            root: PathBuf::from("/home/u/code/super/libs/inner"),
            dot_git_file: Some("gitdir: ../../.git/modules/libs/inner\n".into()),
        };
        let (name, wt) = repo_name(&d);
        assert_eq!(name, "inner");
        assert!(!wt);
    }

    #[test]
    fn unborn_branch_parses() {
        // Fresh `git init`, before the first commit.
        let i = parse_porcelain_v2("# branch.oid (initial)\n# branch.head main\n");
        assert_eq!(i.branch, "main");
        assert_eq!((i.staged, i.modified, i.untracked), (0, 0, 0));
    }

    #[test]
    fn unicode_branch_name_parses() {
        let i = parse_porcelain_v2("# branch.head feat/añadir-configuración\n");
        assert_eq!(i.branch, "feat/añadir-configuración");
    }

    #[test]
    fn short_malformed_entry_lines_do_not_panic() {
        // Defensive: entry lines shorter than the XY field.
        let i = parse_porcelain_v2("1 \n2\nu\n? \n#\n");
        assert_eq!(i.staged, 0);
    }

    #[test]
    fn plain_repo_name_is_root_basename() {
        let d = Discovery {
            root: PathBuf::from("/home/u/code/proj"),
            dot_git_file: None,
        };
        let (name, wt) = repo_name(&d);
        assert_eq!(name, "proj");
        assert!(!wt);
    }

    #[test]
    fn commit_subject_truncation() {
        assert_eq!(truncate_chars("short", COMMIT_MAX_CHARS), "short");
        assert_eq!(
            truncate_chars(
                "a very long commit subject line that keeps going",
                COMMIT_MAX_CHARS
            ),
            "a very long commit subject line that…"
        );
    }
}
