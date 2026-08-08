//! Optional user configuration: `~/.config/claude-statusline/config.json`.
//!
//! Only the opinionated ambient segments are configurable — the core lines
//! render for everyone. A missing file means defaults. An invalid file
//! falls back to defaults with a warning on stderr (stderr is the
//! diagnostics surface; stdout is the statusline).

use serde::Deserialize;

use crate::cache;

#[derive(Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Line 4: loaded MCP server and skill counts for this session.
    pub inventory: bool,
    /// Line 4: count of active sessions on this machine.
    pub sessions: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            inventory: true,
            sessions: true,
        }
    }
}

pub fn load() -> Config {
    let Some(home) = cache::home_dir() else {
        return Config::default();
    };
    let path = home
        .join(".config")
        .join("claude-statusline")
        .join("config.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Config::default(); // no file: defaults
    };
    match serde_json::from_str(&raw) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "claude-statusline: invalid {}: {e}; using defaults",
                path.display()
            );
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_all_on() {
        let c = Config::default();
        assert!(c.inventory && c.sessions);
    }

    #[test]
    fn partial_config_keeps_other_defaults() {
        let c: Config = serde_json::from_str(r#"{"sessions": false}"#).unwrap();
        assert!(c.inventory);
        assert!(!c.sessions);
    }
}
