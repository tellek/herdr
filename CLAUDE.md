# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

AGENTS.md

## Rules

- **ALWAYS capture a passing test baseline before making changes.** Run the full test suite first (via the `windows-tests` skill on Windows) and record the result, so that after your changes you can re-run and diff against the baseline to tell whether a failure was caused by your change or was pre-existing.

## Project

Herdr is a terminal workspace manager for AI coding agents, built with Rust and ratatui. It runs as a server/client pair communicating over a local socket (Unix socket on Linux/macOS, named pipe on Windows via `interprocess`). Windows support is preview beta.

## Commands

```powershell
cargo build --release --locked            # build release binary
just test                                 # cargo nextest + maintenance script tests
just lint                                 # cargo fmt --check + cargo clippy
just ci                                   # lint + nextest (PR CI subset)
just test-one <filter>                    # run one nextest filter
cargo run --release --locked -- --default-config  # print default config
```

`just check` is the full pre-commit suite but includes `windows-lint` which uses Unix env var syntax and will not run correctly on native Windows. On native Windows, run these separately:

```powershell
just ci                                   # formatting + clippy + nextest
$env:LIBGHOSTTY_VT_SIMD="false"; cargo clippy --bin herdr --locked --target x86_64-pc-windows-msvc -- -D warnings
python3 -m unittest scripts.test_agent_detection_manifest_check scripts.test_changelog scripts.test_preview scripts.test_vendor_libghostty_vt
```

## Architecture

**Server/client split.** `herdr` attaches a client to a background server. The server owns all `PaneRuntime` (real PTY processes). The client handles rendering and input. They communicate over a local socket.

**State separation.** `AppState` (`src/app/state.rs`) is pure data with no PTY or async dependencies — testable with `AppState::test_new()`. `PaneState` is separate from `PaneRuntime`. Core modules must not contain `#[cfg(target_os)]`; all platform code goes in `src/platform/`.

**Platform code.** `src/platform/windows.rs` handles process enumeration (via Windows toolhelp snapshot API), foreground job detection, process CWD (via `NtQueryInformationProcess` + `ReadProcessMemory`), and URL opening (via `ShellExecuteW`). `src/ipc.rs` uses `interprocess::local_socket` with `GenericNamespaced` on Windows (named pipes) and `GenericFilePath` on Unix (domain sockets).

**Render pipeline.** `compute_view()` handles geometry and mutations. `render()` takes `&AppState` and only draws — never mutate state during render.

**Agent detection.** `src/detect/` reads a screen snapshot to detect agent state. Detection is decoupled from the parser and viewport. Manifest hot-reload and overrides are in `src/detect/manifest_update.rs`. Manifests live in `src/detect/manifests/`.

**API layer.** `src/api/` defines the JSON wire protocol. `src/app/api/` implements server-side handlers. Schema types are in `src/api/schema/`.

**Socket API.** Agents and external tools communicate with the running server via the local socket, using the same protocol as the client.

**Sidebar layout.** The left sidebar shows only the agents section — the spaces/workspaces list is hidden. The agent panel spans the full sidebar height minus reserved rows (2 when `mouse_capture=true`, else 1). The menu button sits at the second-to-last row; the collapse toggle is at the last row. `workspace_at_row()` always returns `None`; workspace switching goes through the agent list. `workspace_card_areas` is always set to `Vec::new()` in `compute_view_internal`. The collapsed sidebar shows only agent markers (no workspace dots).

## Agent naming

Agent entries in the left sidebar use a two-row display: primary label (row 1, bold) and agent label (row 2, secondary). The primary label defaults to the CWD folder name (via `derive_label_from_cwd` → `display_name_from`). If `agent_name` is set (via `herdr agent rename`), it overrides the primary label. If `session_title` is set (auto-discovered from Claude's JSONL file), it becomes the primary label when no `agent_name` is set.

`session_title` is populated in `AppEvent::AgentSessionReported` (in `src/app/actions.rs`) when the agent is `claude`: it reads `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`, finds the last `{"type":"ai-title","aiTitle":"..."}` or `{"type":"title","title":"..."}` entry, and stores it via `TerminalState::set_session_title`. The encoded path replaces all non-alphanumeric chars with `-` (e.g. `C:\git\herdr` → `C--git-herdr`). `name_override` in `PaneDetail` carries whichever of `agent_name`/`session_title` wins; `agent_label` (row 2) always shows the detected agent type.

## Statusline panel

A 1-row statusline bar sits at the bottom of the terminal pane area (right of the sidebar, spanning to the right edge). It is desktop-only (not rendered in mobile layout). `compute_view_internal` carves it out of `main_terminal_area` before passing `terminal_area` to the pane renderer; `ViewState::statusline_rect` stores its coordinates.

`render_statusline` in `src/ui/statusline.rs` looks up the focused pane's `TerminalState` (via `app.active → ws.active_tab → tab.layout.focused() → tab.terminal_id() → app.terminals`) and renders `terminal.effective_custom_status()` when present. When `custom_status` is absent it falls back to the CWD folder name. The `custom_status` string is populated by the `statusline-command.ps1` Claude hook, which formats: `[Model] effort:X | ctx:[bar%] | cost:$X | pts:[bar%] | 📁 folder`.

## Paste handling

Paste text is sent to PTY panes via `encode_paste_payload` in `src/pane.rs`. When the pane has bracketed paste mode enabled (`InputState::bracketed_paste`), the text is wrapped in `\x1b[200~...\x1b[201~`. When not, newlines are backslash-escaped so the shell treats the entire paste as a single command continuation rather than executing on each newline.

## Windows-specific notes

- Config and logs: `%APPDATA%\herdr\` (e.g. `C:\Users\<user>\AppData\Roaming\herdr\`)
- Agent detection overrides: `%APPDATA%\herdr\agent-detection\<agent>.toml`
- IPC uses Windows named pipes via `interprocess` — the socket "path" is actually a named pipe name under `GenericNamespaced`
- Keyboard enhancement flags (Kitty protocol) are no-ops on Windows (`src/main.rs`)
- Clipboard image bridging is not yet wired on Windows (`src/platform/windows.rs`)
- Desktop notifications are not yet implemented on Windows
- When running a debug build inside a live herdr session, clear socket overrides in PowerShell: `$env:HERDR_SOCKET_PATH=$null; $env:HERDR_CLIENT_SOCKET_PATH=$null; cargo run -- <command>`
- Git hooks (`just install-hooks`) use `chmod` and won't apply on Windows; configure hooks manually if needed

## Testing

Unit tests live next to the code (`#[cfg(test)] mod tests`). New `AppState` or `Workspace` behavior should use `AppState::test_new()` and `Workspace::test_new()` without PTYs. For identity/state refactors, use `AppState::assert_invariants_for_test()` with `AppState::test_with_adversarial_identity_state()`.
