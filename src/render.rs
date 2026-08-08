//! Pure renderer: payload + pre-gathered externals in, ANSI text out.
//! No filesystem or process access here — everything it needs is passed in,
//! which is what keeps it unit-testable and the hot path predictable.

use crate::git::GitInfo;
use crate::inventory::Inventory;
use crate::payload::{Payload, RateWindow};
use crate::settings::CompactLimit;
use crate::timefmt;

pub const RST: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
const GRAY: &str = "\x1b[38;5;240m";
const PURPLE: &str = "\x1b[38;5;183m";
const CYAN: &str = "\x1b[38;5;81m";
const BLUE: &str = "\x1b[38;5;111m";
const PINK: &str = "\x1b[38;5;212m";
const ORANGE: &str = "\x1b[38;5;209m";
const GREEN: &str = "\x1b[38;5;120m";
const YELLOW: &str = "\x1b[38;5;221m";
const RED: &str = "\x1b[38;5;204m";
const GOLD: &str = "\x1b[38;5;178m";
const LAVENDER: &str = "\x1b[38;5;147m";
const TEAL: &str = "\x1b[38;5;73m";

pub struct External {
    pub git: Option<GitInfo>,
    pub compact_limit: CompactLimit,
    /// Loaded MCP/skill inventory from the transcript. None when unavailable.
    pub inventory: Option<Inventory>,
    /// Active sessions on this machine. None when unavailable.
    pub active_sessions: Option<u64>,
}

impl Default for External {
    fn default() -> Self {
        External {
            git: None,
            compact_limit: CompactLimit::Known(200_000),
            inventory: None,
            active_sessions: None,
        }
    }
}

fn sep() -> String {
    format!(" {GRAY}·{RST} ")
}

/// OSC 8 hyperlink (BEL-terminated — widest terminal support).
fn osc8(url: &str, text: &str) -> String {
    format!("\x1b]8;;{url}\x07{text}\x1b]8;;\x07")
}

pub fn fmt_tokens(t: u64) -> String {
    if t >= 1_000_000 {
        format!("{:.1}M", t as f64 / 1_000_000.0)
    } else if t >= 1_000 {
        format!("{}K", t / 1_000)
    } else {
        t.to_string()
    }
}

pub fn render(p: &Payload, ext: &External) -> String {
    let lines: Vec<String> = [line1(p, ext), line2(p, ext), line3(p, ext), line4(ext)]
        .into_iter()
        .filter(|l| !l.is_empty())
        .collect();
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

// ── Line 1: repo · branch [wt] +S~M?U ↑A↓B PR ───────────────────────────
fn line1(p: &Payload, ext: &External) -> String {
    let Some(g) = &ext.git else {
        // Not a repo: just the directory name.
        let dir = p
            .cwd()
            .map(|c| {
                std::path::Path::new(c)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| c.to_string())
            })
            .unwrap_or_default();
        return if dir.is_empty() {
            String::new()
        } else {
            format!("{CYAN}{dir}{RST}")
        };
    };

    let repo_identity = p.workspace.repo.as_ref();
    let display_name = repo_identity
        .and_then(|r| r.name.clone())
        .unwrap_or_else(|| g.repo_name.clone());
    let name_txt = format!("{BOLD}{CYAN}{display_name}{RST}");
    let mut line = match repo_identity.and_then(|r| r.url()) {
        Some(url) => osc8(&url, &name_txt),
        None => name_txt,
    };

    line.push_str(&sep());
    line.push_str(&format!("{LAVENDER}{}{RST}", g.branch));
    if g.is_worktree || p.workspace.git_worktree.is_some() {
        line.push_str(&format!(" {PURPLE}[wt]{RST}"));
    }

    let mut counters = String::new();
    if g.staged > 0 {
        counters.push_str(&format!("{GREEN}+{}{RST}", g.staged));
    }
    if g.modified > 0 {
        counters.push_str(&format!("{ORANGE}~{}{RST}", g.modified));
    }
    if g.untracked > 0 {
        counters.push_str(&format!("{GRAY}?{}{RST}", g.untracked));
    }
    if !counters.is_empty() {
        line.push(' ');
        line.push_str(&counters);
    }

    let mut sync = String::new();
    if g.ahead > 0 {
        sync.push_str(&format!("{PURPLE}↑{}{RST}", g.ahead));
    }
    if g.behind > 0 {
        sync.push_str(&format!("{BLUE}↓{}{RST}", g.behind));
    }
    if !sync.is_empty() {
        line.push(' ');
        line.push_str(&sync);
    }

    // PR state straight from the payload — no `gh` spawn.
    if let Some(pr) = &p.pr {
        let (icon, color) = match pr.review_state.as_deref() {
            Some("approved") => ("✓", GREEN),
            Some("changes_requested") => ("✗", RED),
            Some("draft") => ("◌", GRAY),
            _ => ("○", YELLOW), // open, review pending/unknown
        };
        let label = pr.number.map(|n| format!("#{n}")).unwrap_or_default();
        let txt = format!("{color}{icon}{label}{RST}");
        line.push(' ');
        match &pr.url {
            Some(url) => line.push_str(&osc8(url, &txt)),
            None => line.push_str(&txt),
        }
    }

    line
}

// ── Line 2: model · effort [badges] · ctx · $cost · session ─────────────
fn line2(p: &Payload, ext: &External) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(model) = p.model.display_name.as_deref().or(p.model.id.as_deref()) {
        parts.push(format!("{PINK}{model}{RST}"));
    }

    if let Some(level) = p.effort.as_ref().and_then(|e| e.level.as_deref()) {
        let color = match level {
            "low" => GRAY,
            "medium" => BLUE,
            "high" => CYAN,
            "xhigh" => PURPLE,
            "max" => RED,
            _ => GRAY,
        };
        let mut s = format!("{color}{level}{RST}");
        // Deviation badges only — a badge that is always present is noise.
        if p.fast_mode == Some(true) {
            s.push_str(&format!(" {ORANGE}⚡fast{RST}"));
        }
        if p.thinking.as_ref().and_then(|t| t.enabled) == Some(false) {
            s.push_str(&format!(" {YELLOW}¬think{RST}"));
        }
        parts.push(s);
    }

    parts.push(ctx_part(p, ext.compact_limit));

    if let Some(cost) = p.cost.as_ref().and_then(|c| c.total_cost_usd) {
        parts.push(format!("{GOLD}${cost:.2}{RST}"));
    }

    if let Some(name) = p.session_name.as_deref() {
        parts.push(format!(
            "{TEAL}{}{RST}",
            truncate_ellipsis(name, SESSION_MAX_CHARS)
        ));
    }

    parts.join(&sep())
}

const SESSION_MAX_CHARS: usize = 36;

/// Control characters would corrupt the line-oriented output (a `\n`
/// injects a line, an ESC opens an escape sequence), so they are dropped
/// before the length check.
fn truncate_ellipsis(s: &str, max: usize) -> String {
    let clean: String = s.chars().filter(|c| !c.is_control()).collect();
    if clean.chars().count() <= max {
        clean
    } else {
        let mut t: String = clean.chars().take(max).collect();
        t.push('…');
        t
    }
}

/// ctx measured against the auto-compact window (distance to compact,
/// not to the model ceiling). If settings.json is corrupt we measure against
/// the real model ceiling and show a loud `cfg!` marker — never a silently
/// invented denominator.
fn ctx_part(p: &Payload, limit: CompactLimit) -> String {
    let cw = p.context_window.as_ref();
    let used = cw.and_then(|c| c.total_input_tokens).unwrap_or(0);
    if used == 0 {
        return format!("{GRAY}ctx:--{RST}");
    }

    let (denom, cfg_broken) = match limit {
        CompactLimit::Known(n) if n > 0 => (Some(n), false),
        _ => (
            cw.and_then(|c| c.context_window_size).filter(|&n| n > 0),
            true,
        ),
    };

    let colored = match denom {
        Some(d) => {
            let pct = used.saturating_mul(100) / d;
            let remaining = 100_i64.saturating_sub(pct as i64);
            let body = format!("ctx:{}/{} ({pct}%)", fmt_tokens(used), fmt_tokens(d));
            if remaining <= 15 {
                format!("{RED}{body}{RST} {RED}{BOLD}⚠compact{RST}")
            } else if remaining <= 30 {
                format!("{YELLOW}{body}{RST}")
            } else {
                format!("{GREEN}{body}{RST}")
            }
        }
        None => format!("{GREEN}ctx:{}{RST}", fmt_tokens(used)),
    };

    if cfg_broken {
        format!("{colored} {RED}{BOLD}cfg!{RST}")
    } else {
        colored
    }
}

// ── Line 3: 5h/7d limits ↻reset · session tokens · last commit ──────────
fn line3(p: &Payload, ext: &External) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(rl) = &p.rate_limits {
        let mut rate = String::new();
        if let Some(s) = rate_part("5h", rl.five_hour.as_ref()) {
            rate.push_str(&s);
        }
        if let Some(s) = rate_part("7d", rl.seven_day.as_ref()) {
            if !rate.is_empty() {
                rate.push(' ');
            }
            rate.push_str(&s);
        }
        if !rate.is_empty() {
            parts.push(rate);
        }
    }

    // Session-total tokens from the transcript (all API usage: fresh
    // input, cache writes, cache reads, output). A partial sum is not a
    // total: the segment waits for the scan to reach the end of the file.
    if let Some(inv) = &ext.inventory
        && inv.caught_up
        && !inv.lossy
        && inv.total_tokens > 0
    {
        parts.push(format!(
            "{GRAY}tok:{RST}{BLUE}{}{RST}",
            fmt_tokens(inv.total_tokens)
        ));
    }

    if let Some(g) = &ext.git
        && !g.last_commit.is_empty()
    {
        parts.push(format!("{GRAY}{}{RST}", g.last_commit));
    }

    parts.join(&sep())
}

// ── Line 4 (ambient, deviation-only): mcp · skills · sessions ───────────
// Loaded-inventory counts: what this session pays context for, whether or
// not it is used. Absent entirely when nothing is loaded and only one
// session is active. Ambient state must not shout.
fn line4(ext: &External) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(inv) = &ext.inventory {
        let mcp = inv.mcp_count();
        if mcp > 0 {
            parts.push(format!("{GRAY}mcp:{RST}{BLUE}{mcp}{RST}"));
        }
        if let Some(n) = inv.skill_count.filter(|&n| n > 0) {
            parts.push(format!("{GRAY}skills:{RST}{LAVENDER}{n}{RST}"));
        }
    }

    // One session is the default state — only a plurality is signal.
    if let Some(n) = ext.active_sessions
        && n >= 2
    {
        parts.push(format!("{GRAY}sessions:{RST}{TEAL}{n}{RST}"));
    }

    parts.join(&sep())
}

fn rate_part(label: &str, w: Option<&RateWindow>) -> Option<String> {
    let w = w?;
    // A percentage outside 0..=100 is invalid input, not a rate.
    let raw = w.used_percentage?;
    if !raw.is_finite() || !(0.0..=100.0).contains(&raw) {
        return None;
    }
    let pct = raw.round() as i64;
    let color = if pct >= 80 {
        RED
    } else if pct >= 50 {
        YELLOW
    } else {
        TEAL
    };
    let mut s = format!("{color}{label} {pct}%{RST}");
    if let Some(lbl) = w.resets_at.and_then(timefmt::reset_label) {
        s.push_str(&format!(" {GRAY}↻{lbl}{RST}"));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::Payload;

    fn full_payload() -> Payload {
        serde_json::from_str(
            r#"{
              "cwd": "/tmp/repo",
              "session_name": "review",
              "model": {"id": "claude-fable-5", "display_name": "Fable 5"},
              "workspace": {"current_dir": "/tmp/repo",
                            "repo": {"host": "github.com", "owner": "me", "name": "repo"}},
              "cost": {"total_cost_usd": 14.45, "total_lines_added": 12, "total_lines_removed": 3},
              "context_window": {"total_input_tokens": 123176, "context_window_size": 1000000},
              "effort": {"level": "xhigh"},
              "thinking": {"enabled": true},
              "fast_mode": false,
              "rate_limits": {"five_hour": {"used_percentage": 43},
                              "seven_day": {"used_percentage": 81}},
              "pr": {"number": 7, "url": "https://github.com/me/repo/pull/7",
                     "review_state": "approved"}
            }"#,
        )
        .unwrap()
    }

    fn git_fixture() -> GitInfo {
        GitInfo {
            repo_name: "repo".into(),
            branch: "main".into(),
            staged: 1,
            modified: 2,
            untracked: 3,
            ahead: 1,
            last_commit: "fix: something".into(),
            ..Default::default()
        }
    }

    #[test]
    fn full_render_contains_all_segments() {
        let p = full_payload();
        let ext = External {
            git: Some(git_fixture()),
            compact_limit: CompactLimit::Known(350_000),
            inventory: Some(Inventory {
                total_tokens: 2_400_000,
                caught_up: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let out = render(&p, &ext);
        for needle in [
            "repo",
            "main",
            "+1",
            "~2",
            "?3",
            "↑1",
            "✓#7", // line 1
            "Fable 5",
            "xhigh",
            "ctx:123K/350K (35%)",
            "$14.45",
            "review", // line 2
            "5h 43%",
            "7d 81%",
            "tok:",
            "2.4M",
            "fix: something", // line 3
        ] {
            assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
        }
        // 43% teal, 81% red
        assert!(out.contains(&format!("{TEAL}5h 43%{RST}")));
        assert!(out.contains(&format!("{RED}7d 81%{RST}")));
        // thinking on + fast off ⇒ no deviation badges
        assert!(!out.contains("⚡fast"));
        assert!(!out.contains("¬think"));
    }

    #[test]
    fn partial_token_total_does_not_render() {
        let p = full_payload();
        let ext = External {
            compact_limit: CompactLimit::Known(350_000),
            inventory: Some(Inventory {
                total_tokens: 2_400_000,
                caught_up: false,
                ..Default::default()
            }),
            ..Default::default()
        };
        let out = render(&p, &ext);
        assert!(
            !out.contains("tok:"),
            "a partial sum is not a total:\n{out}"
        );
    }

    #[test]
    fn out_of_range_percentage_does_not_render() {
        let mut p = full_payload();
        p.rate_limits
            .as_mut()
            .unwrap()
            .five_hour
            .as_mut()
            .unwrap()
            .used_percentage = Some(-20.0);
        let ext = External {
            compact_limit: CompactLimit::Known(350_000),
            ..Default::default()
        };
        let out = render(&p, &ext);
        assert!(
            !out.contains("5h"),
            "invalid percentage must not render:\n{out}"
        );
        assert!(out.contains("7d 81%"), "valid window stays:\n{out}");
    }

    #[test]
    fn control_characters_in_session_name_are_dropped() {
        let mut p = full_payload();
        p.session_name = Some("re\x1b[31mview\nx".into());
        let ext = External {
            compact_limit: CompactLimit::Known(350_000),
            ..Default::default()
        };
        let out = render(&p, &ext);
        assert!(out.contains("re[31mviewx"), "controls dropped:\n{out}");
        assert_eq!(out.lines().count(), 3, "no injected line:\n{out}");
    }

    #[test]
    fn empty_payload_renders_placeholder() {
        let p: Payload = serde_json::from_str("{}").unwrap();
        let ext = External {
            git: None,
            compact_limit: CompactLimit::Known(200_000),
            ..Default::default()
        };
        let out = render(&p, &ext);
        assert!(out.contains("ctx:--"));
    }

    #[test]
    fn broken_settings_measures_against_ceiling_and_flags() {
        let p = full_payload();
        let ext = External {
            git: None,
            compact_limit: CompactLimit::Unavailable,
            ..Default::default()
        };
        let out = render(&p, &ext);
        assert!(
            out.contains("cfg!"),
            "must flag corrupt settings loudly:\n{out}"
        );
        assert!(
            out.contains("123K/1.0M"),
            "must use real ceiling, not invented default:\n{out}"
        );
    }

    #[test]
    fn context_over_compact_window_clamps_bar_shows_true_pct() {
        // Real case: tokens can pass autoCompactWindow before compaction fires.
        let mut p = full_payload();
        p.context_window.as_mut().unwrap().total_input_tokens = Some(420_000);
        let ext = External {
            git: None,
            compact_limit: CompactLimit::Known(350_000),
            ..Default::default()
        };
        let out = render(&p, &ext);
        assert!(out.contains("120%"), "number must tell the truth:\n{out}");
        assert!(out.contains("⚠compact"), "over-limit must warn:\n{out}");
    }

    #[test]
    fn long_session_name_truncates_multibyte_safely() {
        let mut p = full_payload();
        p.session_name = Some("Revisar configuración de Claude y computer use".into());
        let ext = External {
            git: None,
            compact_limit: CompactLimit::Known(350_000),
            ..Default::default()
        };
        let out = render(&p, &ext);
        assert!(
            out.contains("Revisar configuración de Claude y co…"),
            "truncate on char boundary:\n{out}"
        );
        assert!(!out.contains("computer use"));
    }

    #[test]
    fn root_cwd_without_git_renders_path() {
        let p: Payload = serde_json::from_str(r#"{"cwd": "/"}"#).unwrap();
        let ext = External {
            git: None,
            compact_limit: CompactLimit::Known(200_000),
            ..Default::default()
        };
        // Must not panic on a path with no file_name component.
        assert!(render(&p, &ext).contains("/"));
    }

    #[test]
    fn low_context_shows_low_marker() {
        let mut p = full_payload();
        p.context_window.as_mut().unwrap().total_input_tokens = Some(320_000);
        let ext = External {
            git: None,
            compact_limit: CompactLimit::Known(350_000),
            ..Default::default()
        };
        let out = render(&p, &ext);
        assert!(out.contains("⚠compact"), "91% used must warn:\n{out}");
    }

    #[test]
    fn deviation_badges_appear_when_deviating() {
        let mut p = full_payload();
        p.fast_mode = Some(true);
        p.thinking.as_mut().unwrap().enabled = Some(false);
        let ext = External {
            git: None,
            compact_limit: CompactLimit::Known(350_000),
            ..Default::default()
        };
        let out = render(&p, &ext);
        assert!(out.contains("⚡fast"));
        assert!(out.contains("¬think"));
    }

    #[test]
    fn pr_states_map_to_icons() {
        for (state, icon) in [
            ("changes_requested", "✗#7"),
            ("draft", "◌#7"),
            ("pending", "○#7"),
        ] {
            let mut p = full_payload();
            p.pr.as_mut().unwrap().review_state = Some(state.into());
            let ext = External {
                git: Some(git_fixture()),
                compact_limit: CompactLimit::Known(350_000),
                ..Default::default()
            };
            assert!(
                render(&p, &ext).contains(icon),
                "state {state} should render {icon}"
            );
        }
    }

    #[test]
    fn line4_absent_without_ambient_data() {
        let p = full_payload();
        let ext = External {
            git: None,
            compact_limit: CompactLimit::Known(350_000),
            ..Default::default()
        };
        let out = render(&p, &ext);
        assert!(!out.contains("mcp:"));
        assert!(!out.contains("sessions:"));
        assert_eq!(out.trim_end().lines().count(), 3);
    }

    #[test]
    fn line4_shows_loaded_counts() {
        let mut inv = Inventory::default();
        for t in ["mcp__chrome__navigate", "mcp__codex__run"] {
            inv.mcp_tools.insert(t.into());
        }
        inv.mcp_servers.insert("chrome".into()); // dup with tools: one server
        inv.mcp_servers.insert("exa".into());
        inv.skill_count = Some(56);
        let ext = External {
            git: None,
            compact_limit: CompactLimit::Known(350_000),
            inventory: Some(inv),
            active_sessions: Some(3),
        };
        let out = render(&full_payload(), &ext);
        assert!(
            out.contains(&format!("{GRAY}mcp:{RST}{BLUE}3{RST}")),
            "{out}"
        );
        assert!(
            out.contains(&format!("{GRAY}skills:{RST}{LAVENDER}56{RST}")),
            "{out}"
        );
        assert!(
            out.contains(&format!("{GRAY}sessions:{RST}{TEAL}3{RST}")),
            "{out}"
        );
        assert!(!out.contains("chrome"), "names must not render: {out}");
    }

    #[test]
    fn single_session_is_not_signal() {
        let ext = External {
            git: None,
            compact_limit: CompactLimit::Known(350_000),
            active_sessions: Some(1),
            ..Default::default()
        };
        assert!(!render(&full_payload(), &ext).contains("sessions:"));
    }

    #[test]
    fn zero_skill_listing_renders_no_segment() {
        let mut inv = Inventory::default();
        inv.mcp_tools.insert("mcp__codex__run".into());
        inv.skill_count = Some(0); // a listing arrived, but nothing is loaded
        let ext = External {
            git: None,
            compact_limit: CompactLimit::Known(350_000),
            inventory: Some(inv),
            ..Default::default()
        };
        let out = render(&full_payload(), &ext);
        assert!(
            out.contains(&format!("{GRAY}mcp:{RST}{BLUE}1{RST}")),
            "{out}"
        );
        assert!(!out.contains("skills:"), "zero is not signal: {out}");
    }

    #[test]
    fn token_formatting() {
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(1_000), "1K");
        assert_eq!(fmt_tokens(123_176), "123K");
        assert_eq!(fmt_tokens(1_500_000), "1.5M");
    }
}
