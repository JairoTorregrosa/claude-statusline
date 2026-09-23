//! Reads `autoCompactWindow` from ~/.claude/settings.json — the configured
//! setting used by the ctx gauge. The model ceiling in the payload caps
//! the setting; `CompactLimit::denominator` combines the two. Claude Code
//! can also apply session or environment overrides that this file cannot see.
//!
//! A missing file or key leaves the window unconfigured. A usable
//! denominator still needs the model ceiling from the payload, since the
//! effective window may be capped by that ceiling.
//! A file that cannot be parsed, or a key that is present with a non-integer
//! or zero value, is a broken config and must degrade LOUDLY: we return
//! `Unavailable` and the renderer shows a visible `cfg!` marker. A
//! denominator is never invented.

/// Window used by this gauge for reported ceilings below
/// `LARGE_CONTEXT_CEILING` when no setting is configured.
pub const DEFAULT_COMPACT_WINDOW: u64 = 200_000;

/// At or above this reported ceiling the gauge uses the model ceiling.
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
    /// Resolve a window from the setting and a reported model ceiling.
    /// Without the ceiling, neither the default nor a configured value can
    /// be confirmed as the effective limit.
    ///
    /// `ceiling` is `context_window.context_window_size` from the payload,
    /// already filtered to a non-zero value. `None` means the payload did
    /// not report one.
    pub fn denominator(self, ceiling: Option<u64>) -> Option<u64> {
        match self {
            CompactLimit::Configured(n) => ceiling.map(|g| n.min(g)),
            CompactLimit::Default => ceiling.map(|g| {
                if g >= LARGE_CONTEXT_CEILING {
                    g
                } else {
                    g.min(DEFAULT_COMPACT_WINDOW)
                }
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
    fn missing_ceiling_has_no_verified_denominator() {
        assert_eq!(CompactLimit::Default.denominator(None), None);
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
        assert_eq!(CompactLimit::Configured(350_000).denominator(None), None);
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
