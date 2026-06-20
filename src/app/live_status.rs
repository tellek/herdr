//! Live statusline data for Claude panes.
//!
//! The Claude `statusLine` command (installed via the user's integration) dumps
//! its full stdin payload to `~/.claude/projects/<encoded-cwd>/<session_id>.yaml`
//! on every render. herdr polls that file (keyed by the pane's Claude session
//! id) and stores the parsed fields so the bottom statusline panel can render a
//! styled summary.

use crate::app::state::AppState;

/// Parsed Claude statusLine payload for one pane. Rendering (colors/icons) lives
/// in the UI layer; this struct only carries data. Not persisted.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveStatus {
    pub model: String,
    pub effort: String,
    pub fast_mode: bool,
    pub thinking: bool,
    pub context_pct: Option<f64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub current_dir: String,
    pub added_dirs: usize,
    pub exceeds_200k: bool,
}

/// Parse a Claude statusLine payload (JSON, which is also valid YAML — the writer
/// dumps raw JSON into the `.yaml` file). Returns `None` if it can't be parsed or
/// has no usable model field.
fn parse_live_status(payload: &str) -> Option<LiveStatus> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;

    let model = v
        .get("model")
        .and_then(|m| m.get("display_name"))
        .and_then(|n| n.as_str())?
        .to_string();

    let effort = v
        .get("effort")
        .and_then(|e| e.get("level"))
        .and_then(|l| l.as_str())
        .unwrap_or("default")
        .to_string();

    let cw = v.get("context_window");
    let u64_at = |obj: Option<&serde_json::Value>, key: &str| -> u64 {
        obj.and_then(|o| o.get(key))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let cost = v.get("cost");

    Some(LiveStatus {
        model,
        effort,
        fast_mode: v
            .get("fast_mode")
            .and_then(|f| f.as_bool())
            .unwrap_or(false),
        thinking: v
            .get("thinking")
            .and_then(|t| t.get("enabled"))
            .and_then(|e| e.as_bool())
            .unwrap_or(false),
        context_pct: cw
            .and_then(|c| c.get("used_percentage"))
            .and_then(serde_json::Value::as_f64),
        input_tokens: u64_at(cw, "total_input_tokens"),
        output_tokens: u64_at(cw, "total_output_tokens"),
        cost: cost
            .and_then(|c| c.get("total_cost_usd"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0),
        lines_added: u64_at(cost, "total_lines_added"),
        lines_removed: u64_at(cost, "total_lines_removed"),
        current_dir: v
            .get("workspace")
            .and_then(|w| w.get("current_dir"))
            .and_then(|d| d.as_str())
            .or_else(|| v.get("cwd").and_then(|d| d.as_str()))
            .unwrap_or_default()
            .to_string(),
        added_dirs: v
            .get("workspace")
            .and_then(|w| w.get("added_dirs"))
            .and_then(|a| a.as_array())
            .map(|a| a.len())
            .unwrap_or(0),
        exceeds_200k: v
            .get("exceeds_200k_tokens")
            .and_then(|e| e.as_bool())
            .unwrap_or(false),
    })
}

/// Locate `<home>/.claude/projects/*/<session_id>.yaml` and parse its payload.
fn read_live_status_for_session(home: &std::path::Path, session_id: &str) -> Option<LiveStatus> {
    let projects = home.join(".claude").join("projects");
    let file = format!("{session_id}.yaml");
    for entry in std::fs::read_dir(&projects).ok()?.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let candidate = entry.path().join(&file);
        if let Ok(payload) = std::fs::read_to_string(&candidate) {
            return parse_live_status(&payload);
        }
    }
    None
}

fn claude_home() -> Option<std::path::PathBuf> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(std::path::PathBuf::from)
}

impl AppState {
    /// Refresh each Claude pane's `live_status` from its statusLine payload yaml.
    pub fn refresh_agent_live_statuses(&mut self) {
        let Some(home) = claude_home() else {
            return;
        };
        for terminal in self.terminals.values_mut() {
            terminal.live_status = terminal
                .claude_session_id()
                .and_then(|id| read_live_status_for_session(&home, id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "session_id": "abc",
        "model": { "id": "claude-opus-4-8", "display_name": "Opus 4.8" },
        "effort": { "level": "high" },
        "context_window": {
            "used_percentage": 12,
            "total_input_tokens": 117315,
            "total_output_tokens": 2032
        },
        "cost": {
            "total_cost_usd": 17.72,
            "total_lines_added": 498,
            "total_lines_removed": 107
        },
        "workspace": { "current_dir": "C:\\git\\herdr", "added_dirs": ["a", "b"] },
        "exceeds_200k_tokens": false,
        "fast_mode": true,
        "thinking": { "enabled": true }
    }"#;

    #[test]
    fn parses_full_payload() {
        let s = parse_live_status(SAMPLE).unwrap();
        assert_eq!(s.model, "Opus 4.8");
        assert_eq!(s.effort, "high");
        assert!(s.fast_mode);
        assert!(s.thinking);
        assert_eq!(s.context_pct, Some(12.0));
        assert_eq!(s.input_tokens, 117315);
        assert_eq!(s.output_tokens, 2032);
        assert!((s.cost - 17.72).abs() < 1e-9);
        assert_eq!(s.lines_added, 498);
        assert_eq!(s.lines_removed, 107);
        assert_eq!(s.current_dir, "C:\\git\\herdr");
        assert_eq!(s.added_dirs, 2);
        assert!(!s.exceeds_200k);
    }

    #[test]
    fn defaults_when_fields_missing() {
        let s = parse_live_status(r#"{ "model": { "display_name": "Sonnet 4.6" } }"#).unwrap();
        assert_eq!(s.model, "Sonnet 4.6");
        assert_eq!(s.effort, "default");
        assert!(!s.fast_mode);
        assert!(!s.thinking);
        assert_eq!(s.context_pct, None);
        assert_eq!(s.input_tokens, 0);
        assert_eq!(s.cost, 0.0);
        assert_eq!(s.added_dirs, 0);
    }

    #[test]
    fn no_model_returns_none() {
        assert!(parse_live_status(r#"{ "effort": { "level": "low" } }"#).is_none());
    }

    #[test]
    fn invalid_json_returns_none() {
        assert!(parse_live_status("not json").is_none());
    }

    #[test]
    fn reads_payload_by_session_id_glob() {
        let home = std::env::temp_dir().join(format!(
            "herdr-live-status-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let proj = home.join(".claude").join("projects").join("C--git-herdr");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("abc.yaml"), SAMPLE).unwrap();

        let s = read_live_status_for_session(&home, "abc").unwrap();
        assert_eq!(s.model, "Opus 4.8");
        assert!(read_live_status_for_session(&home, "missing").is_none());

        let _ = std::fs::remove_dir_all(&home);
    }
}
