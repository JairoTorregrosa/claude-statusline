//! Reads `autoCompactWindow` from ~/.claude/settings.json — the denominator
//! for the ctx gauge ("distance to auto-compact", not distance to the model
//! ceiling).
//!
//! Failure policy (deliberate): a *missing* file or key selects the
//! documented default (200k) — that is real semantics, not a mask. A file
//! that cannot be parsed, or a key that is present with a non-integer or
//! zero value, is a broken config and must degrade LOUDLY: we return
//! `Unavailable` and the renderer shows a visible `cfg!` marker. A
//! denominator is never invented.

const DEFAULT_COMPACT_WINDOW: u64 = 200_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompactLimit {
    /// Explicit or default-by-absence value, usable as denominator.
    Known(u64),
    /// settings.json exists but is unreadable/corrupt — render must flag it.
    Unavailable,
}

pub fn compact_limit() -> CompactLimit {
    let Some(home) = crate::cache::home_dir() else {
        return CompactLimit::Unavailable;
    };
    let path = home.join(".claude").join("settings.json");
    if !path.exists() {
        return CompactLimit::Known(DEFAULT_COMPACT_WINDOW);
    }
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return CompactLimit::Unavailable;
    };
    parse_limit(&raw)
}

fn parse_limit(raw: &str) -> CompactLimit {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => match v.get("autoCompactWindow") {
            None => CompactLimit::Known(DEFAULT_COMPACT_WINDOW),
            Some(x) => match x.as_u64() {
                Some(n) if n > 0 => CompactLimit::Known(n),
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
            CompactLimit::Known(350_000)
        );
    }

    #[test]
    fn absent_key_selects_the_documented_default() {
        assert_eq!(
            parse_limit(r#"{"model": "opus"}"#),
            CompactLimit::Known(DEFAULT_COMPACT_WINDOW)
        );
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
}
