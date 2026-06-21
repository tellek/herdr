use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::app::live_status::LiveStatus;
use crate::app::state::Palette;
use crate::app::AppState;

const CONTEXT_BAR_WIDTH: usize = 10;

/// Return the focused pane's TerminalState, if any.
fn focused_terminal(app: &AppState) -> Option<&crate::terminal::TerminalState> {
    let ws_idx = app.active?;
    let ws = app.workspaces.get(ws_idx)?;
    let tab = ws.tabs.get(ws.active_tab)?;
    let pane_id = tab.layout.focused();
    let terminal_id = tab.terminal_id(pane_id)?;
    app.terminals.get(terminal_id)
}

/// Color for the model name based on its family.
fn model_color(model: &str, p: &Palette) -> Color {
    let m = model.to_ascii_lowercase();
    if m.contains("opus") {
        p.peach // orange
    } else if m.contains("sonnet") {
        p.teal // cyan
    } else if m.contains("haiku") {
        p.yellow // light yellow
    } else if m.contains("fable") {
        p.red
    } else {
        p.text
    }
}

/// Color for the effort level.
fn effort_color(effort: &str, p: &Palette) -> Color {
    match effort {
        "low" => p.teal,
        "medium" => p.green,
        "high" => p.yellow,
        "xhigh" => p.red,
        _ => p.text,
    }
}

/// Build the styled statusline spans from parsed live status.
fn live_status_spans(s: &LiveStatus, p: &Palette) -> Vec<Span<'static>> {
    let dim = Style::default().fg(p.text);
    let mut spans: Vec<Span<'static>> = Vec::new();

    // [model] at <effort> effort  <fast> <thinking>
    spans.push(Span::styled(
        format!("[{}]", s.model),
        Style::default().fg(model_color(&s.model, p)),
    ));
    spans.push(Span::styled(" at ", dim));
    spans.push(Span::styled(
        s.effort.clone(),
        Style::default().fg(effort_color(&s.effort, p)),
    ));
    spans.push(Span::styled(" effort", dim));
    if s.fast_mode {
        spans.push(Span::styled(" \u{26A1}", dim)); // lightning
    }
    if s.thinking {
        spans.push(Span::styled(" \u{1F9E0}", dim)); // brain
    }

    // | [bar] pct%  ->in  <-out   (context in light purple)
    spans.push(Span::styled(" | ", dim));
    let mauve = Style::default().fg(p.mauve);
    let (bar, pct) = match s.context_pct {
        Some(pct) => {
            let filled = (((pct / 100.0) * CONTEXT_BAR_WIDTH as f64).floor() as usize)
                .min(CONTEXT_BAR_WIDTH);
            (
                "\u{2588}".repeat(filled) + &"\u{2591}".repeat(CONTEXT_BAR_WIDTH - filled),
                format!("{}%", pct.round() as i64),
            )
        }
        None => ("\u{2591}".repeat(CONTEXT_BAR_WIDTH), "--%".to_string()),
    };
    spans.push(Span::styled(format!("[{bar}] {pct}"), mauve));
    spans.push(Span::styled(
        format!(" \u{2192}{} \u{2190}{}", s.input_tokens, s.output_tokens),
        dim,
    ));

    // | money $cost
    spans.push(Span::styled(" | ", dim));
    spans.push(Span::styled(format!("\u{1F4B0} ${:.2}", s.cost), dim));

    // | up added (green)  down removed (red)
    spans.push(Span::styled(" | ", dim));
    spans.push(Span::styled(
        format!("\u{2191}{}", s.lines_added),
        Style::default().fg(p.green),
    ));
    spans.push(Span::styled(
        format!(" \u{2193}{}", s.lines_removed),
        Style::default().fg(p.red),
    ));

    // | folder dir +N
    spans.push(Span::styled(" | ", dim));
    spans.push(Span::styled(format!("\u{1F4C1} {}", s.current_dir), dim));
    if s.added_dirs > 0 {
        spans.push(Span::styled(format!(" +{}", s.added_dirs), dim));
    }

    // | alarm 200K alarm (red) when over 200k
    if s.exceeds_200k {
        spans.push(Span::styled(" | ", dim));
        spans.push(Span::styled(
            "\u{23F0} 200K \u{23F0}",
            Style::default().fg(p.red),
        ));
    }

    // | session id
    if let Some(id) = &s.session_id {
        spans.push(Span::styled(" | ", dim));
        spans.push(Span::styled(id.clone(), dim));
    }

    spans
}

/// Return the CWD folder name of the focused pane for the fallback display.
fn focused_cwd_label(app: &AppState) -> Option<String> {
    let ws_idx = app.active?;
    let ws = app.workspaces.get(ws_idx)?;
    let tab = ws.tabs.get(ws.active_tab)?;
    let pane_id = tab.layout.focused();
    let terminal_id = tab.terminal_id(pane_id)?;
    let terminal = app.terminals.get(terminal_id)?;
    terminal
        .cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .or_else(|| Some(terminal.cwd.display().to_string()))
}

pub(super) fn render_statusline(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let p = &app.palette;

    // Background bar
    let block = Block::default().style(Style::default().bg(p.panel_bg));
    frame.render_widget(block, area);

    let inner = Rect::new(
        area.x + 1,
        area.y,
        area.width.saturating_sub(2),
        area.height,
    );
    if inner.width == 0 {
        return;
    }

    let line = if let Some(status) = focused_terminal(app).and_then(|t| t.live_status.as_ref()) {
        Line::from(live_status_spans(status, p))
    } else if let Some(status) = focused_terminal(app).and_then(|t| t.effective_custom_status()) {
        Line::from(Span::styled(status, Style::default().fg(p.text)))
    } else if let Some(cwd) = focused_cwd_label(app) {
        Line::from(vec![
            Span::styled("\u{1F4C1} ", Style::default().fg(p.overlay0)),
            Span::styled(
                cwd,
                Style::default().fg(p.subtext0).add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        Line::from(Span::styled(
            "no active session",
            Style::default().fg(p.overlay0),
        ))
    };

    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppState;
    use crate::workspace::Workspace;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn statusline_renders_without_panic_when_no_workspace() {
        let app = AppState::test_new();
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_statusline(&app, frame, Rect::new(0, 0, 80, 1)))
            .unwrap();
    }

    #[tokio::test]
    async fn statusline_renders_cwd_fallback_when_no_custom_status() {
        let mut app = AppState::test_new();
        let ws = Workspace::test_new("test");
        app.workspaces = vec![ws];
        app.active = Some(0);

        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_statusline(&app, frame, Rect::new(0, 0, 80, 1)))
            .unwrap();

        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        // Should contain the folder name from the test workspace CWD
        assert!(!rendered.trim().is_empty());
    }

    #[test]
    fn statusline_renders_empty_area_without_panic() {
        let app = AppState::test_new();
        let backend = TestBackend::new(1, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_statusline(&app, frame, Rect::new(0, 0, 0, 0)))
            .unwrap();
    }
}
