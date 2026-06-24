//! Stdin input reading for the thin client.
//!
//! On Unix, reads stdin bytes and forwards framed input to the main event loop.
//! The server handles semantic parsing. On Windows, crossterm may surface
//! terminal control strings as character key events, so the reader re-frames
//! those control bytes before forwarding semantic client input events.
//!
//! This is simpler and more reliable because:
//! - The server has the same input parsing code
//! - We avoid duplicating parsing logic in the client
//! - Host terminal control replies can be buffered or discarded before they leak

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(unix)]
use std::io::{self, Read};
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(windows)]
use std::time::Duration;
use tokio::sync::mpsc;

use super::ClientLoopEvent;

// ---------------------------------------------------------------------------
// Stdin reader thread
// ---------------------------------------------------------------------------

/// Reads raw bytes from stdin and sends them to the main event loop.
///
/// This runs on a dedicated thread because stdin reading is blocking.
/// The main loop receives the raw bytes and forwards them as
/// `ClientMessage::Input` to the server.
pub fn stdin_reader_loop(
    event_tx: mpsc::Sender<ClientLoopEvent>,
    should_quit: &Arc<AtomicBool>,
    host_color_query_sent: bool,
) {
    #[cfg(windows)]
    {
        let _ = host_color_query_sent;
        windows_stdin_reader_loop(event_tx, should_quit);
    }

    #[cfg(unix)]
    unix_stdin_reader_loop(event_tx, should_quit, host_color_query_sent);
}

#[cfg(unix)]
fn unix_stdin_reader_loop(
    event_tx: mpsc::Sender<ClientLoopEvent>,
    should_quit: &Arc<AtomicBool>,
    host_color_query_sent: bool,
) {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut scratch = [0u8; 4096];
    let mut framer = crate::raw_input::RawInputByteFramer::default();
    if host_color_query_sent {
        framer.host_color_query_sent();
        framer.enable_host_color_scheme_change_tracking();
    }

    while !should_quit.load(Ordering::Acquire) {
        match reader.read(&mut scratch) {
            Ok(0) => break,
            Ok(n) => {
                for data in framer.push(&scratch[..n]) {
                    if event_tx
                        .blocking_send(ClientLoopEvent::StdinInput(data))
                        .is_err()
                    {
                        return;
                    }
                }

                if stdin_read_ready(&reader, 10) == Some(false) {
                    let had_pending = framer.has_pending_input();
                    let chunks = framer.flush_timeout();
                    let held_escape = had_pending && chunks.is_empty();
                    for data in chunks {
                        if event_tx
                            .blocking_send(ClientLoopEvent::StdinInput(data))
                            .is_err()
                        {
                            return;
                        }
                    }
                    if held_escape && stdin_read_ready(&reader, 10) == Some(false) {
                        for data in framer.flush_timeout() {
                            if event_tx
                                .blocking_send(ClientLoopEvent::StdinInput(data))
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            }
            Err(err) => {
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
        }
    }
}

#[cfg(windows)]
fn windows_stdin_reader_loop(
    event_tx: mpsc::Sender<ClientLoopEvent>,
    should_quit: &Arc<AtomicBool>,
) {
    let mut framer = crate::raw_input::RawInputFramer::default();
    let mut raw_sequence_pending = false;
    let mut pending_ticks: u32 = 0;
    // After ~5 s with no end marker, abandon the incomplete bracketed paste.
    const MAX_PENDING_TICKS: u32 = 500;
    // On Windows a paste is delivered as a rapid burst of individual Char/Enter key
    // events (no bracketed-paste markers), so an embedded newline is an Enter key
    // that submits in the inner app. This accumulator coalesces such a burst into a
    // single Paste event the server brackets, so newlines stay text.
    let mut paste_burst = PasteBurstAccumulator::default();

    while !should_quit.load(Ordering::Acquire) {
        match crossterm::event::poll(Duration::from_millis(10)) {
            Ok(true) => {
                pending_ticks = 0;
            }
            Ok(false) => {
                // No event is queued: the burst (if any) has ended. Finalize it.
                let flushed = paste_burst.finish();
                if !flushed.is_empty()
                    && event_tx
                        .blocking_send(ClientLoopEvent::StdinEvents(flushed))
                        .is_err()
                {
                    return;
                }
                if raw_sequence_pending {
                    let flushed = framer.flush_timeout();
                    let still_pending = framer.has_pending_input();
                    if !send_windows_raw_events(flushed, &event_tx) {
                        return;
                    }
                    if still_pending {
                        // Framer is still waiting for the rest of a sequence (e.g. a large
                        // bracketed paste delivered in multiple chunks).  Keep routing input
                        // through the framer so the sequence can complete.
                        pending_ticks += 1;
                        if pending_ticks >= MAX_PENDING_TICKS {
                            tracing::warn!("windows input bracketed paste timed out; abandoning");
                            framer.clear_pending_input();
                            raw_sequence_pending = false;
                            pending_ticks = 0;
                        }
                    } else {
                        tracing::debug!("windows input raw sequence timed out; flushed");
                        raw_sequence_pending = false;
                        pending_ticks = 0;
                    }
                }
                continue;
            }
            Err(_) => break,
        }

        let event = match crossterm::event::read() {
            Ok(event) => event,
            Err(_) => break,
        };

        if let Some(bytes) = windows_key_raw_bytes(&event, raw_sequence_pending) {
            tracing::debug!(
                bytes = ?bytes,
                pending_before = raw_sequence_pending,
                "windows input routed through raw framer"
            );
            let events = framer.push(&bytes);
            raw_sequence_pending = events.is_empty();
            if !send_windows_raw_events(events, &event_tx) {
                return;
            }
            continue;
        }

        if raw_sequence_pending {
            tracing::debug!("windows input raw sequence interrupted by semantic event; flushing");
            if !send_windows_raw_events(framer.flush_timeout(), &event_tx) {
                return;
            }
            raw_sequence_pending = false;
        }

        // Feed the event to the paste-burst accumulator. It may flush a finalized
        // burst (events to forward now) and tells us whether the current event was
        // absorbed into a new burst or should fall through to normal handling.
        let outcome = paste_burst.observe(&event);
        if !outcome.flush.is_empty()
            && event_tx
                .blocking_send(ClientLoopEvent::StdinEvents(outcome.flush))
                .is_err()
        {
            return;
        }
        if outcome.absorbed {
            continue;
        }

        if windows_event_is_control_key(&event) {
            tracing::debug!(event = ?event, "windows control key forwarded as semantic input");
        }

        let Some(event) = crate::protocol::ClientInputEvent::from_crossterm(event) else {
            continue;
        };
        if event_tx
            .blocking_send(ClientLoopEvent::StdinEvents(vec![event]))
            .is_err()
        {
            return;
        }
    }

    if raw_sequence_pending {
        let _ = send_windows_raw_events(framer.flush_timeout(), &event_tx);
    }
}

#[cfg(windows)]
fn windows_event_is_control_key(event: &crossterm::event::Event) -> bool {
    use crossterm::event::{Event, KeyModifiers};

    matches!(
        event,
        Event::Key(key)
            if key.modifiers.contains(KeyModifiers::CONTROL)
                || matches!(key.code, crossterm::event::KeyCode::Char(ch) if ch.is_control())
    )
}

/// Result of feeding one event to the [`PasteBurstAccumulator`].
#[cfg(windows)]
#[derive(Default)]
struct PasteBurstOutcome {
    /// Finalized events to forward to the server right now (a coalesced paste,
    /// or buffered keystrokes replayed because the run was just normal typing).
    flush: Vec<crate::protocol::ClientInputEvent>,
    /// Whether the current event was absorbed into a building burst (so the
    /// caller must not also forward it through the normal semantic path).
    absorbed: bool,
}

/// Coalesces the rapid burst of individual key events Windows produces for a
/// paste into a single `Paste` event.
///
/// On Windows there are no bracketed-paste markers in the input stream: a paste
/// arrives as many `Char`/`Enter`/`Tab` key presses delivered back-to-back, so an
/// embedded newline is an `Enter` key that submits in the inner app (e.g. Claude
/// submits the first line). This buffers consecutive printable key presses and,
/// when the run ends (an idle poll or a non-text key), emits them as one `Paste`
/// when the run is paste-shaped (multiple characters spanning a newline). Short
/// runs and single keys replay unchanged, so normal typing is unaffected.
#[cfg(windows)]
#[derive(Default)]
struct PasteBurstAccumulator {
    buffer: String,
}

#[cfg(windows)]
impl PasteBurstAccumulator {
    fn observe(&mut self, event: &crossterm::event::Event) -> PasteBurstOutcome {
        use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

        if let Event::Key(key) = event {
            // Releases never carry text; ignore them while a burst builds so they
            // do not split or end it.
            if key.kind == KeyEventKind::Release {
                return PasteBurstOutcome {
                    flush: Vec::new(),
                    absorbed: !self.buffer.is_empty(),
                };
            }
            let unmodified = (key.modifiers & !KeyModifiers::SHIFT).is_empty();
            let ch = match key.code {
                KeyCode::Char(ch) if unmodified && !ch.is_control() => Some(ch),
                KeyCode::Enter if unmodified => Some('\n'),
                KeyCode::Tab if unmodified => Some('\t'),
                _ => None,
            };
            if let Some(ch) = ch {
                self.buffer.push(ch);
                return PasteBurstOutcome {
                    flush: Vec::new(),
                    absorbed: true,
                };
            }
        }

        // A non-text event ends any building run; finalize it, then let the
        // current event fall through to normal handling.
        PasteBurstOutcome {
            flush: self.finish(),
            absorbed: false,
        }
    }

    /// Finalize the buffered run, returning the events to forward.
    fn finish(&mut self) -> Vec<crate::protocol::ClientInputEvent> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let text = std::mem::take(&mut self.buffer);
        if text.chars().count() >= 2 && text.contains('\n') {
            return vec![crate::protocol::ClientInputEvent::Paste { text }];
        }
        // Not paste-shaped: replay the buffered characters as the individual key
        // events they would have been, so normal typing is unchanged.
        text.chars().map(client_key_for_char).collect()
    }
}

/// Build a semantic key event for a buffered character (used when a short run is
/// replayed as normal typing rather than coalesced into a paste).
#[cfg(windows)]
fn client_key_for_char(ch: char) -> crate::protocol::ClientInputEvent {
    use crossterm::event::KeyCode;

    let code = match ch {
        '\n' => KeyCode::Enter,
        '\t' => KeyCode::Tab,
        other => KeyCode::Char(other),
    };
    crate::protocol::ClientInputEvent::Key {
        code: crate::protocol::ClientKeyCode::from_crossterm(code)
            .unwrap_or(crate::protocol::ClientKeyCode::Char(ch)),
        modifiers: 0,
        kind: crate::protocol::ClientKeyKind::Press,
    }
}

#[cfg(windows)]
fn windows_key_raw_bytes(
    event: &crossterm::event::Event,
    raw_sequence_pending: bool,
) -> Option<Vec<u8>> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

    let Event::Key(key) = event else {
        return None;
    };
    if key.kind == KeyEventKind::Release {
        return None;
    }

    match key.code {
        KeyCode::Esc if key.modifiers.is_empty() => Some(vec![0x1b]),
        KeyCode::Char(ch)
            if !raw_sequence_pending
                && matches!(ch, 'i' | 'I')
                && key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            let mut buf = [0; 4];
            Some(ch.encode_utf8(&mut buf).as_bytes().to_vec())
        }
        KeyCode::Char(ch) if raw_sequence_pending || ch.is_control() => {
            let mut bytes = Vec::new();
            if key.modifiers.contains(KeyModifiers::ALT) {
                bytes.push(0x1b);
            }
            let mut buf = [0; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            Some(bytes)
        }
        // Inside an in-flight raw sequence (e.g. a bracketed paste being
        // reassembled from char-by-char console events), newlines and tabs
        // surface as Enter/Tab key codes. Feed them back as raw bytes so they
        // stay part of the paste payload instead of interrupting it and
        // reaching the pane as a literal Enter (submitting in Claude).
        KeyCode::Enter if raw_sequence_pending => Some(vec![b'\r']),
        KeyCode::Tab if raw_sequence_pending => Some(vec![b'\t']),
        _ => None,
    }
}

#[cfg(windows)]
fn send_windows_raw_events(
    events: Vec<crate::raw_input::RawInputEvent>,
    event_tx: &mpsc::Sender<ClientLoopEvent>,
) -> bool {
    let raw_event_count = events.len();
    let events = events
        .into_iter()
        .filter_map(windows_client_input_event_from_raw)
        .collect::<Vec<_>>();
    if events.is_empty() {
        return true;
    }

    tracing::debug!(
        raw_event_count,
        forwarded_event_count = events.len(),
        "windows raw-framed input events forwarded"
    );
    event_tx
        .blocking_send(ClientLoopEvent::StdinEvents(events))
        .is_ok()
}

#[cfg(windows)]
fn windows_client_input_event_from_raw(
    event: crate::raw_input::RawInputEvent,
) -> Option<crate::protocol::ClientInputEvent> {
    match event {
        crate::raw_input::RawInputEvent::Key(key) => Some(crate::protocol::ClientInputEvent::Key {
            code: crate::protocol::ClientKeyCode::from_crossterm(key.code)?,
            modifiers: key.modifiers.bits(),
            kind: crate::protocol::ClientKeyKind::from_crossterm(key.kind),
        }),
        crate::raw_input::RawInputEvent::Mouse(mouse) => {
            Some(crate::protocol::ClientInputEvent::Mouse {
                kind: crate::protocol::ClientMouseKind::from_crossterm(mouse.kind)?,
                column: mouse.column,
                row: mouse.row,
                modifiers: mouse.modifiers.bits(),
            })
        }
        crate::raw_input::RawInputEvent::Paste(text) => {
            Some(crate::protocol::ClientInputEvent::Paste { text })
        }
        crate::raw_input::RawInputEvent::OuterFocusGained => {
            Some(crate::protocol::ClientInputEvent::FocusGained)
        }
        crate::raw_input::RawInputEvent::OuterFocusLost => {
            Some(crate::protocol::ClientInputEvent::FocusLost)
        }
        crate::raw_input::RawInputEvent::HostDefaultColor { .. }
        | crate::raw_input::RawInputEvent::HostColorSchemeChanged(_)
        | crate::raw_input::RawInputEvent::Unsupported => None,
    }
}

#[cfg(unix)]
fn stdin_read_ready<R: AsRawFd>(reader: &R, timeout_ms: i32) -> Option<bool> {
    poll_read_ready(reader.as_raw_fd(), timeout_ms)
}

#[cfg(unix)]
fn poll_read_ready(fd: i32, timeout_ms: i32) -> Option<bool> {
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    unsafe extern "C" {
        fn poll(fds: *mut PollFd, nfds: usize, timeout: i32) -> i32;
    }

    const POLLIN: i16 = 0x0001;

    let mut pfd = PollFd {
        fd,
        events: POLLIN,
        revents: 0,
    };

    let result = unsafe { poll(&mut pfd as *mut PollFd, 1, timeout_ms) };
    if result < 0 {
        None
    } else {
        Some(result > 0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, unix))]
mod tests {
    // The stdin reader thread is hard to unit test since it reads from actual stdin.
    // Integration tests will verify the full client→server input flow.
    // Here we test the event type construction.

    use super::*;

    #[cfg(unix)]
    #[test]
    fn stdin_input_event_carries_raw_bytes() {
        let data = vec![0x1b, b'[', b'A']; // Up arrow escape sequence
        let event = ClientLoopEvent::StdinInput(data.clone());
        match event {
            ClientLoopEvent::StdinInput(d) => assert_eq!(d, data),
            _ => panic!("expected StdinInput event"),
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn windows_control_chars_are_reframed_as_raw_bytes() {
        let escape = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert_eq!(
            windows_key_raw_bytes(&escape, false).as_deref(),
            Some(b"\x1b".as_slice())
        );

        let enter = Event::Key(KeyEvent::new(KeyCode::Char('\r'), KeyModifiers::empty()));
        assert_eq!(
            windows_key_raw_bytes(&enter, false).as_deref(),
            Some(b"\r".as_slice())
        );

        let printable = Event::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty()));
        assert_eq!(windows_key_raw_bytes(&printable, false), None);

        let pending_arrow_tail =
            Event::Key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::empty()));
        assert_eq!(
            windows_key_raw_bytes(&pending_arrow_tail, true).as_deref(),
            Some(b"[".as_slice())
        );
    }

    #[test]
    fn windows_ctrl_d_semantic_event_encodes_to_eot() {
        let event = Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(windows_key_raw_bytes(&event, false), None);

        let event =
            crate::protocol::ClientInputEvent::from_crossterm(event).expect("ctrl-d converts");
        let raw = event.to_raw_input_event();
        let crate::raw_input::RawInputEvent::Key(key) = raw else {
            panic!("expected key");
        };
        assert_eq!(key.code, KeyCode::Char('d'));
        assert_eq!(key.modifiers, KeyModifiers::CONTROL);
        assert_eq!(
            crate::input::encode_terminal_key(key, crate::input::KeyboardProtocol::Legacy),
            b"\x04"
        );
    }

    #[test]
    fn windows_pasted_printable_ctrl_i_routes_as_literal_i() {
        let event = Event::Key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL));
        assert_eq!(
            windows_key_raw_bytes(&event, false).as_deref(),
            Some(b"i".as_slice())
        );

        let event = Event::Key(KeyEvent::new(
            KeyCode::Char('I'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert_eq!(
            windows_key_raw_bytes(&event, false).as_deref(),
            Some(b"I".as_slice())
        );
    }

    #[test]
    fn windows_enter_inside_pending_sequence_stays_in_paste() {
        // While a bracketed paste is being reassembled char-by-char, a newline
        // surfaces as KeyCode::Enter and must be fed back as a raw byte instead
        // of interrupting the sequence (which would submit in Claude).
        let enter = Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(
            windows_key_raw_bytes(&enter, true).as_deref(),
            Some(b"\r".as_slice())
        );
        // Outside a pending sequence, Enter is a normal semantic key, not raw.
        assert_eq!(windows_key_raw_bytes(&enter, false), None);

        let tab = Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
        assert_eq!(
            windows_key_raw_bytes(&tab, true).as_deref(),
            Some(b"\t".as_slice())
        );
        assert_eq!(windows_key_raw_bytes(&tab, false), None);
    }

    #[test]
    fn windows_bracketed_paste_with_newline_reassembles_to_single_paste() {
        // Simulate the console delivering "\x1b[200~ab<Enter>cd\x1b[201~" as the
        // discrete key events herdr sees, and assert the framer yields one Paste.
        let mut framer = crate::raw_input::RawInputFramer::default();
        let mut pending = false;
        let mut events = Vec::new();

        let sequence = [
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('~'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('~'), KeyModifiers::empty())),
        ];

        for event in sequence {
            let bytes =
                windows_key_raw_bytes(&event, pending).expect("paste bytes route through framer");
            let produced = framer.push(&bytes);
            pending = produced.is_empty();
            events.extend(produced);
        }

        assert_eq!(events.len(), 1);
        let crate::raw_input::RawInputEvent::Paste(text) = &events[0] else {
            panic!("expected a single Paste event");
        };
        assert_eq!(text, "ab\rcd");
    }

    #[test]
    fn windows_multiline_paste_reassembles_with_no_enter_keys_escaping() {
        // With ENABLE_VIRTUAL_TERMINAL_INPUT enabled the host delivers the
        // bracketed-paste markers as a discrete key-event stream. A paste with
        // MULTIPLE newlines must reassemble into exactly one Paste carrying every
        // newline, and NO event may fall through as a semantic Enter (which would
        // submit). This guards the "a newline mid-block submits" report.
        let mut framer = crate::raw_input::RawInputFramer::default();
        let mut pending = false;
        let mut events = Vec::new();

        // "\x1b[200~a<NL>b<NL>c\x1b[201~"
        let sequence = [
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('~'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::empty())),
            Event::Key(KeyEvent::new(KeyCode::Char('~'), KeyModifiers::empty())),
        ];

        for event in sequence {
            // Every event in the marker stream must route through the framer; none
            // may escape to the semantic-key path (which is where a bare Enter
            // would submit).
            let bytes = windows_key_raw_bytes(&event, pending)
                .expect("every paste-stream event routes through the framer");
            let produced = framer.push(&bytes);
            pending = produced.is_empty();
            events.extend(produced);
        }

        assert_eq!(events.len(), 1, "multi-line paste must be one event");
        let raw = events.into_iter().next().unwrap();
        let crate::raw_input::RawInputEvent::Paste(text) = &raw else {
            panic!("expected a single Paste event");
        };
        assert_eq!(text, "a\rb\rc");

        // And it converts to a wire Paste, not a Key, so the server brackets it.
        let wire = windows_client_input_event_from_raw(raw).expect("paste converts");
        assert_eq!(
            wire,
            crate::protocol::ClientInputEvent::Paste {
                text: "a\rb\rc".to_string()
            }
        );
    }

    #[test]
    fn windows_eot_control_char_normalizes_to_ctrl_d() {
        let event = Event::Key(KeyEvent::new(KeyCode::Char('\u{4}'), KeyModifiers::empty()));
        let bytes = windows_key_raw_bytes(&event, false).expect("eot routes through raw framer");
        assert_eq!(bytes, b"\x04");

        let mut framer = crate::raw_input::RawInputFramer::default();
        let events = framer.push(&bytes);
        assert_eq!(events.len(), 1);

        let event = windows_client_input_event_from_raw(events.into_iter().next().unwrap())
            .expect("raw eot converts");
        assert_eq!(
            event,
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Char('d'),
                modifiers: KeyModifiers::CONTROL.bits(),
                kind: crate::protocol::ClientKeyKind::Press,
            }
        );
    }

    #[test]
    fn windows_pending_escape_sequence_converts_to_semantic_arrow() {
        let mut framer = crate::raw_input::RawInputFramer::default();
        assert!(framer.push(b"\x1b").is_empty());
        assert!(framer.push(b"[").is_empty());
        let events = framer.push(b"A");
        assert_eq!(events.len(), 1);

        let event = windows_client_input_event_from_raw(events.into_iter().next().unwrap())
            .expect("raw arrow converts");
        assert_eq!(
            event,
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Up,
                modifiers: 0,
                kind: crate::protocol::ClientKeyKind::Press,
            }
        );
    }

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::empty()))
    }

    fn feed(
        acc: &mut PasteBurstAccumulator,
        events: &[Event],
    ) -> Vec<crate::protocol::ClientInputEvent> {
        let mut out = Vec::new();
        for event in events {
            let outcome = acc.observe(event);
            out.extend(outcome.flush);
        }
        out.extend(acc.finish());
        out
    }

    #[test]
    fn multiline_burst_coalesces_into_single_paste() {
        let mut acc = PasteBurstAccumulator::default();
        let out = feed(
            &mut acc,
            &[
                press(KeyCode::Char('a')),
                press(KeyCode::Char('b')),
                press(KeyCode::Enter),
                press(KeyCode::Char('c')),
            ],
        );
        assert_eq!(
            out,
            vec![crate::protocol::ClientInputEvent::Paste {
                text: "ab\nc".to_string()
            }]
        );
    }

    #[test]
    fn lone_enter_is_not_coalesced_and_still_submits() {
        let mut acc = PasteBurstAccumulator::default();
        let out = feed(&mut acc, &[press(KeyCode::Enter)]);
        assert_eq!(
            out,
            vec![crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Enter,
                modifiers: 0,
                kind: crate::protocol::ClientKeyKind::Press,
            }]
        );
    }

    #[test]
    fn single_line_run_without_newline_replays_as_keys() {
        let mut acc = PasteBurstAccumulator::default();
        let out = feed(
            &mut acc,
            &[press(KeyCode::Char('h')), press(KeyCode::Char('i'))],
        );
        assert_eq!(out.len(), 2);
        assert!(out
            .iter()
            .all(|e| matches!(e, crate::protocol::ClientInputEvent::Key { .. })));
    }

    #[test]
    fn non_text_key_ends_burst_and_passes_through() {
        let mut acc = PasteBurstAccumulator::default();
        assert!(acc.observe(&press(KeyCode::Char('x'))).absorbed);
        assert!(acc.observe(&press(KeyCode::Enter)).absorbed);
        let outcome = acc.observe(&press(KeyCode::Up));
        assert!(!outcome.absorbed);
        assert_eq!(
            outcome.flush,
            vec![crate::protocol::ClientInputEvent::Paste {
                text: "x\n".to_string()
            }]
        );
    }

    #[test]
    fn release_events_do_not_split_a_burst() {
        let mut acc = PasteBurstAccumulator::default();
        let out = feed(
            &mut acc,
            &[
                press(KeyCode::Char('a')),
                Event::Key(KeyEvent::new_with_kind(
                    KeyCode::Char('a'),
                    KeyModifiers::empty(),
                    crossterm::event::KeyEventKind::Release,
                )),
                press(KeyCode::Enter),
            ],
        );
        assert_eq!(
            out,
            vec![crate::protocol::ClientInputEvent::Paste {
                text: "a\n".to_string()
            }]
        );
    }

    #[test]
    fn windows_bare_escape_flushes_to_semantic_escape() {
        let mut framer = crate::raw_input::RawInputFramer::default();
        assert!(framer.push(b"\x1b").is_empty());
        let events = framer.flush_timeout();
        assert_eq!(events.len(), 1);

        let event = windows_client_input_event_from_raw(events.into_iter().next().unwrap())
            .expect("raw escape converts");
        assert_eq!(
            event,
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Esc,
                modifiers: 0,
                kind: crate::protocol::ClientKeyKind::Press,
            }
        );
    }
}
