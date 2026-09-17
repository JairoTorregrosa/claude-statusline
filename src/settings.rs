//! Reads `autoCompactWindow` from ~/.claude/settings.json — the denominator
//! for the ctx gauge ("distance to auto-compact", not distance to the model
//! ceiling).
//!
//! Failure policy (deliberate): a *missing* file or key selects the
//! documented default (200k) — that is real semantics, not a mask. A file
//! that cannot be parsed, or a key whose value is not one Claude Code
//! itself accepts, is a broken config and must degrade LOUDLY: we return
//! `Unavailable` and the renderer shows a visible `cfg!` marker. A
//! denominator is never invented.
//!
//! Claude Code's `/autocompact` command persists the value as a *string* in
//! the forms the command takes: a token count (`"200000"`), a `k`/`M`
//! suffix (`"500k"`, `"1M"`), a bare number from 100 to 1000 meaning
//! thousands (`"200"`), or `"auto"` for the model-tuned window. A JSON
//! integer is accepted as a token count.

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
            Some(x) => match parse_window(x) {
                Some(n) => CompactLimit::Known(n),
                // Present but unparseable, zero, negative, or fractional:
                // broken config, not a value to default away.
                None => CompactLimit::Unavailable,
            },
        },
        Err(_) => CompactLimit::Unavailable,
    }
}

/// One `autoCompactWindow` value, in tokens. Accepts what Claude Code
/// accepts (see the module docs); rejects everything else with `None`.
fn parse_window(x: &serde_json::Value) -> Option<u64> {
    if let Some(n) = x.as_u64() {
        return (n > 0).then_some(n);
    }
    let s = x.as_str()?.trim();
    if s.eq_ignore_ascii_case("auto") {
        return Some(DEFAULT_COMPACT_WINDOW);
    }
    let (digits, unit) = match s.char_indices().next_back() {
        Some((i, 'k' | 'K')) => (&s[..i], 1_000),
        Some((i, 'm' | 'M')) => (&s[..i], 1_000_000),
        _ => (s, 1),
    };
    let n: u64 = digits.parse().ok()?;
    if n == 0 {
        return None;
    }
    if unit == 1 && (100..=1000).contains(&n) {
        return Some(n * 1_000);
    }
    n.checked_mul(unit)
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
    fn string_values_written_by_autocompact_are_accepted() {
        for (raw, want) in [
            (r#"{"autoCompactWindow": "350000"}"#, 350_000),
            (r#"{"autoCompactWindow": "500k"}"#, 500_000),
            (r#"{"autoCompactWindow": "500K"}"#, 500_000),
            (r#"{"autoCompactWindow": "1M"}"#, 1_000_000),
            (r#"{"autoCompactWindow": "200"}"#, 200_000),
            (r#"{"autoCompactWindow": " 750k "}"#, 750_000),
            (r#"{"autoCompactWindow": "auto"}"#, DEFAULT_COMPACT_WINDOW),
        ] {
            assert_eq!(parse_limit(raw), CompactLimit::Known(want), "raw: {raw}");
        }
    }

    #[test]
    fn present_but_invalid_value_degrades_loudly() {
        for raw in [
            r#"{"autoCompactWindow": null}"#,
            r#"{"autoCompactWindow": ""}"#,
            r#"{"autoCompactWindow": "0"}"#,
            r#"{"autoCompactWindow": "abc"}"#,
            r#"{"autoCompactWindow": "5x"}"#,
            r#"{"autoCompactWindow": "k"}"#,
            r#"{"autoCompactWindow": "-1"}"#,
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
