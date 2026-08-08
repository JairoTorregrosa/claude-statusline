//! Typed model of the JSON Claude Code pipes to the statusline on stdin.
//!
//! Every field is optional: the docs guarantee several objects are absent or
//! null depending on auth type, session phase, and version (`rate_limits`
//! only for subscribers, `current_usage` null right after /compact, `pr`
//! only while a PR is open, ...). A missing field must degrade the render,
//! never abort it.

use serde::{Deserialize, Deserializer};

/// Accept `123176` or `123176.0`. A count that arrives as a float must not
/// abort the whole payload parse (serde would otherwise reject the entire
/// document and blank every segment). A fractional or out-of-range value
/// is not a count and becomes None.
fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|x| {
        x.as_u64().or_else(|| {
            x.as_f64()
                // Strict bound: `u64::MAX as f64` rounds up to 2^64,
                // which is one past the last valid value.
                .filter(|f| f.is_finite() && *f >= 0.0 && f.fract() == 0.0 && *f < u64::MAX as f64)
                .map(|f| f as u64)
        })
    }))
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct Payload {
    pub session_name: Option<String>,
    pub cwd: Option<String>,
    pub transcript_path: Option<String>,
    pub model: Model,
    pub workspace: Workspace,
    pub cost: Option<Cost>,
    pub context_window: Option<ContextWindow>,
    pub effort: Option<Effort>,
    pub thinking: Option<Thinking>,
    pub fast_mode: Option<bool>,
    pub rate_limits: Option<RateLimits>,
    pub pr: Option<Pr>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct Model {
    pub id: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct Workspace {
    pub current_dir: Option<String>,
    pub project_dir: Option<String>,
    /// Worktree name when cwd is inside a linked worktree (any worktree,
    /// not just --worktree sessions).
    pub git_worktree: Option<String>,
    /// Present when the repo has an `origin` remote.
    pub repo: Option<RepoIdentity>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct RepoIdentity {
    pub host: Option<String>,
    pub owner: Option<String>,
    pub name: Option<String>,
}

impl RepoIdentity {
    pub fn url(&self) -> Option<String> {
        match (&self.host, &self.owner, &self.name) {
            (Some(h), Some(o), Some(n)) => Some(format!("https://{h}/{o}/{n}")),
            _ => None,
        }
    }
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct Cost {
    pub total_cost_usd: Option<f64>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct ContextWindow {
    /// Tokens currently occupying the context window
    /// (input + cache_creation + cache_read). 0 before the first response.
    #[serde(deserialize_with = "lenient_u64")]
    pub total_input_tokens: Option<u64>,
    #[serde(deserialize_with = "lenient_u64")]
    pub context_window_size: Option<u64>,
    pub used_percentage: Option<f64>,
    pub remaining_percentage: Option<f64>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct Effort {
    pub level: Option<String>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct Thinking {
    pub enabled: Option<bool>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct RateLimits {
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct RateWindow {
    pub used_percentage: Option<f64>,
    /// Unix epoch seconds when the window resets.
    pub resets_at: Option<i64>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct Pr {
    pub number: Option<u64>,
    pub url: Option<String>,
    /// "approved" | "pending" | "changes_requested" | "draft"
    pub review_state: Option<String>,
}

impl Payload {
    pub fn cwd(&self) -> Option<&str> {
        self.workspace
            .current_dir
            .as_deref()
            .or(self.cwd.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_object_parses() {
        let p: Payload = serde_json::from_str("{}").unwrap();
        assert!(p.cwd().is_none());
        assert!(p.model.display_name.is_none());
    }

    #[test]
    fn float_counts_do_not_abort_the_parse() {
        // A count serialized as 123176.0 must not reject the whole payload.
        let p: Payload = serde_json::from_str(
            r#"{"context_window": {"total_input_tokens": 123176.0,
                                   "context_window_size": 1e6},
                "model": {"display_name": "Fable 5"}}"#,
        )
        .unwrap();
        let cw = p.context_window.unwrap();
        assert_eq!(cw.total_input_tokens, Some(123176));
        assert_eq!(cw.context_window_size, Some(1_000_000));
        assert_eq!(p.model.display_name.as_deref(), Some("Fable 5"));
    }

    #[test]
    fn negative_and_bogus_counts_become_none() {
        let p: Payload = serde_json::from_str(
            r#"{"context_window": {"total_input_tokens": -5,
                                   "context_window_size": "1M"}}"#,
        )
        .unwrap();
        let cw = p.context_window.unwrap();
        assert_eq!(cw.total_input_tokens, None);
        assert_eq!(cw.context_window_size, None);
        // Fractional values are not counts.
        let p: Payload =
            serde_json::from_str(r#"{"context_window": {"total_input_tokens": 123176.9}}"#)
                .unwrap();
        assert_eq!(p.context_window.unwrap().total_input_tokens, None);
        // 2^64 is one past u64::MAX and must not saturate into a count.
        let p: Payload = serde_json::from_str(
            r#"{"context_window": {"total_input_tokens": 18446744073709551616.0}}"#,
        )
        .unwrap();
        assert_eq!(p.context_window.unwrap().total_input_tokens, None);
    }

    #[test]
    fn null_fields_parse() {
        // used_percentage / current_usage are documented as nullable.
        let p: Payload = serde_json::from_str(
            r#"{"context_window": {"used_percentage": null, "current_usage": null},
                "session_name": null}"#,
        )
        .unwrap();
        assert!(p.context_window.unwrap().used_percentage.is_none());
    }

    #[test]
    fn full_payload_parses() {
        let p: Payload = serde_json::from_str(
            r#"{
              "session_id": "x", "cwd": "/tmp",
              "session_name": "review",
              "model": {"id": "claude-fable-5", "display_name": "Fable 5"},
              "workspace": {"current_dir": "/tmp/repo", "project_dir": "/tmp/repo",
                            "added_dirs": [], "git_worktree": "feature-x",
                            "repo": {"host": "github.com", "owner": "me", "name": "repo"}},
              "cost": {"total_cost_usd": 1.5, "total_lines_added": 10, "total_lines_removed": 2},
              "context_window": {"total_input_tokens": 123176, "total_output_tokens": 492,
                                 "context_window_size": 1000000,
                                 "used_percentage": 12, "remaining_percentage": 88},
              "effort": {"level": "xhigh"},
              "thinking": {"enabled": true},
              "fast_mode": false,
              "rate_limits": {"five_hour": {"used_percentage": 43, "resets_at": 1786155000},
                              "seven_day": {"used_percentage": 8, "resets_at": 1786734000}},
              "pr": {"number": 12, "url": "https://github.com/me/repo/pull/12",
                     "review_state": "approved"}
            }"#,
        )
        .unwrap();
        assert_eq!(p.cwd(), Some("/tmp/repo"));
        assert_eq!(
            p.workspace.repo.as_ref().unwrap().url().unwrap(),
            "https://github.com/me/repo"
        );
        assert_eq!(p.effort.unwrap().level.as_deref(), Some("xhigh"));
        assert_eq!(p.pr.unwrap().number, Some(12));
    }
}
