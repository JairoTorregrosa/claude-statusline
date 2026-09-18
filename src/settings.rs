//! Reads `autoCompactWindow` from ~/.claude/settings.json — the configured
//! half of the denominator for the ctx gauge ("distance to auto-compact",
//! not distance to the model ceiling). The other half is the model ceiling
//! the payload reports; `CompactLimit::denominator` combines the two.
//!
//! Failure policy (deliberate): a *missing* file or key means the user
//! configured nothing, so the denominator is derived from the model ceiling
//! the same way Claude Code derives it — that is real semantics, not a mask.
//! A file that cannot be parsed, or a key that is present with a non-integer
//! or zero value, is a broken config and must degrade LOUDLY: we return
//! `Unavailable` and the renderer shows a visible `cfg!` marker. A
//! denominator is never invented.

/// The window Claude Code falls back to when the model ceiling is below
/// `LARGE_CONTEXT_CEILING` and nothing is configured.
pub const DEFAULT_COMPACT_WINDOW: u64 = 200_000;

/// At or above this ceiling Claude Code stops applying
/// `DEFAULT_COMPACT_WINDOW` and lets the session run to the model ceiling.
pub const LARGE_CONTEXT_CEILING: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompactLimit {
    /// `autoCompactWindow` is present and is a usable value.
    Configured(u64),
    /// No settings file, or no `autoCompactWindow` key: nothing is
    /// configured and the ceiling decides.
    Default,
    /// settings.json exists but is unreadable/corrupt — render must flag it.
    Unavailable,
}

impl CompactLimit {
    /// The effective auto-compact window, resolved the way Claude Code
    /// resolves it: a configured value is clamped to the model ceiling, and
    /// with nothing configured a ceiling at or above `LARGE_CONTEXT_CEILING`
    /// is its own window while a smaller ceiling takes the 200k default.
    ///
    /// `ceiling` is `context_window.context_window_size` from the payload,
    /// already filtered to a non-zero value. `None` means the payload did
    /// not report one.
    pub fn denominator(self, ceiling: Option<u64>) -> Option<u64> {
        match self {
            CompactLimit::Configured(n) => Some(match ceiling {
                Some(g) => n.min(g),
                None => n,
            }),
            CompactLimit::Default => Some(match ceiling {
                Some(g) if g >= LARGE_CONTEXT_CEILING => g,
                Some(g) => g.min(DEFAULT_COMPACT_WINDOW),
                None => DEFAULT_COMPACT_WINDOW,
            }),
            // Nothing trustworthy to configure with: fall back to the real
            // ceiling and let the renderer show `cfg!`.
            CompactLimit::Unavailable => ceiling,
        }
    }
}

pub fn compact_limit() -> CompactLimit {
    let Some(home) = crate::cache::home_dir() else {
        return CompactLimit::Unavailable;
    };
    let path = home.join(".claude").join("settings.json");
    if !path.exists() {
        return CompactLimit::Default;
    }
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return CompactLimit::Unavailable;
    };
    parse_limit(&raw)
}

fn parse_limit(raw: &str) -> CompactLimit {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => match v.get("autoCompactWindow") {
            None => CompactLimit::Default,
            Some(x) => match x.as_u64() {
                Some(n) if n > 0 => CompactLimit::Configured(n),
                // Present but wrong type, zero, negative, or fractional:
                // broken config, not a value to default away.
                _ => CompactLimit::Unavailable,
            },
        },
        Err(_) => CompactLimit::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_value_is_used() {
        assert_eq!(
            parse_limit(r#"{"autoCompactWindow": 350000}"#),
            CompactLimit::Configured(350_000)
        );
    }

    #[test]
    fn absent_key_leaves_the_window_unconfigured() {
        assert_eq!(parse_limit(r#"{"model": "opus"}"#), CompactLimit::Default);
    }

    #[test]
    fn present_but_invalid_value_degrades_loudly() {
        for raw in [
            r#"{"autoCompactWindow": "350000"}"#,
            r#"{"autoCompactWindow": null}"#,
            r#"{"autoCompactWindow": -1}"#,
            r#"{"autoCompactWindow": 350000.5}"#,
            r#"{"autoCompactWindow": 0}"#,
            r#"{"autoCompactWindow": {}}"#,
        ] {
            assert_eq!(parse_limit(raw), CompactLimit::Unavailable, "raw: {raw}");
        }
    }

    #[test]
    fn corrupt_json_degrades_loudly() {
        assert_eq!(parse_limit("{not json"), CompactLimit::Unavailable);
    }

    #[test]
    fn unconfigured_million_token_ceiling_is_its_own_window() {
        assert_eq!(
            CompactLimit::Default.denominator(Some(1_000_000)),
            Some(1_000_000)
        );
    }

    #[test]
    fn unconfigured_small_ceiling_takes_the_documented_default() {
        assert_eq!(
            CompactLimit::Default.denominator(Some(200_000)),
            Some(DEFAULT_COMPACT_WINDOW)
        );
        // A ceiling between the default and 1M still compacts at the default.
        assert_eq!(
            CompactLimit::Default.denominator(Some(500_000)),
            Some(DEFAULT_COMPACT_WINDOW)
        );
    }

    #[test]
    fn unconfigured_ceiling_below_the_default_clamps_to_the_ceiling() {
        assert_eq!(
            CompactLimit::Default.denominator(Some(120_000)),
            Some(120_000)
        );
    }

    #[test]
    fn unconfigured_without_a_ceiling_keeps_the_documented_default() {
        assert_eq!(
            CompactLimit::Default.denominator(None),
            Some(DEFAULT_COMPACT_WINDOW)
        );
    }

    #[test]
    fn configured_value_is_clamped_to_the_ceiling() {
        assert_eq!(
            CompactLimit::Configured(1_000_000).denominator(Some(200_000)),
            Some(200_000)
        );
        assert_eq!(
            CompactLimit::Configured(350_000).denominator(Some(1_000_000)),
            Some(350_000)
        );
        assert_eq!(
            CompactLimit::Configured(350_000).denominator(None),
            Some(350_000)
        );
    }

    #[test]
    fn broken_config_measures_against_the_ceiling() {
        assert_eq!(
            CompactLimit::Unavailable.denominator(Some(1_000_000)),
            Some(1_000_000)
        );
        assert_eq!(CompactLimit::Unavailable.denominator(None), None);
    }
}
