use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::app::AppState;

/// Return the effective custom_status for the currently focused pane, if any.
fn focused_custom_status(app: &AppState) -> Option<String> {
    let ws_idx = app.active?;
    let ws = app.workspaces.get(ws_idx)?;
    let tab = ws.tabs.get(ws.active_tab)?;
    let pane_id = tab.layout.focused();
    let terminal_id = tab.terminal_id(pane_id)?;
    let terminal = app.terminals.get(terminal_id)?;
    terminal.effective_custom_status()
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

    let line = if let Some(status) = focused_custom_status(app) {
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

    frame.render_widget(Paragraph::new(line), inner);
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
