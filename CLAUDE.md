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
$env:LIBGHOSTTY_VT_PREBUILT="true"; cargo clippy --bin herdr --locked --target x86_64-pc-windows-msvc -- -D warnings
python3 -m unittest scripts.test_agent_detection_manifest_check scripts.test_changelog scripts.test_preview scripts.test_vendor_libghostty_vt
```

**Zig / libghostty-vt build note.** The vendored `libghostty-vt` requires Zig 0.15.2 to rebuild from source. On machines with a newer Zig, set `LIBGHOSTTY_VT_PREBUILT=true` (or `1`/`yes`/`on`) to skip the Zig build step and use the pre-built artifacts already committed to `vendor/libghostty-vt/zig-out/lib/`. Never set `LIBGHOSTTY_VT_SIMD` as an environment variable unless you have Zig 0.15.2 installed — doing so invalidates the cargo build cache and triggers a Zig rebuild that will fail on any other Zig version.

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

`session_title` is populated and kept fresh via three paths:
1. **`AppEvent::AgentSessionReported`** — fired at `SessionStart`; reads `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl` and stores the last `{"type":"ai-title","aiTitle":"..."}` or `{"type":"title","title":"..."}` entry via `TerminalState::set_session_title`. Prefers `project_cwd` from the event over `agent_session_project_cwd` over `terminal.cwd` to find the correct JSONL.
2. **`AppEvent::HookMetadataReported`** — fired by the statusline hook on `PreToolUse`, `PostToolUse`, and `Stop`; when the source is `"herdr:claude"`, re-reads the same JSONL using the stored `persisted_agent_session.session_ref` and `agent_session_project_cwd`. This ensures that a `/rename` applied during a session is reflected in the sidebar on the next tool use, without requiring a restart.
3. **Snapshot persistence** — `PaneSnapshot.session_title` (added to `src/persist/snapshot.rs`) captures the value at snapshot time. On restore, `persist/restore.rs` calls `terminal.set_session_title(Some(title))` in both the pending-agent-resume path and the normal spawn path, so the correct label is shown immediately on startup before any hook fires.

The encoded path replaces all non-alphanumeric chars with `-` (e.g. `C:\git\herdr` → `C--git-herdr`). `name_override` in `PaneDetail` carries whichever of `agent_name`/`session_title` wins; `agent_label` (row 2) always shows the detected agent type. `read_claude_session_ai_title_from(home, cwd, session_id)` is the inner testable helper; `read_claude_session_ai_title(cwd, session_id)` resolves home from env vars.

## Statusline panel

A 1-row statusline bar sits at the bottom of the terminal pane area (right of the sidebar, spanning to the right edge). It is desktop-only (not rendered in mobile layout). `compute_view_internal` carves it out of `main_terminal_area` before passing `terminal_area` to the pane renderer; `ViewState::statusline_rect` stores its coordinates.

`render_statusline` in `src/ui/statusline.rs` looks up the focused pane's `TerminalState` (via `app.active → ws.active_tab → tab.layout.focused() → tab.terminal_id() → app.terminals`) and renders `terminal.effective_custom_status()` when present. When `custom_status` is absent it falls back to the CWD folder name. The `custom_status` string is populated by the Claude integration hook (`herdr-agent-state.ps1`/`.sh`) on `PreToolUse`, `PostToolUse`, and `Stop` events with the `statusline` action. The hook computes: `[Model] effort:X | [bar] pct% | 💰 $cost | 📁 folder` from the Claude hook payload (model, context window, CWD). The status persists across tool calls (no TTL) so the last-known info stays visible between events. The integration is installed via `herdr integrate install claude` and is at `CLAUDE_INTEGRATION_VERSION=9`. `normalize_custom_status` in `src/app/api_helpers.rs` strips control characters and caps at 512 chars.

## Claude session resume CWD

When herdr auto-resumes a Claude session it runs `claude --resume <id>` inside a shell spawned at the pane's saved CWD. If that CWD is a subdirectory Claude navigated into, Claude may not find the session (sessions are keyed to the project root where `claude` was originally launched).

`TerminalState.agent_session_project_cwd: Option<PathBuf>` stores the project root CWD, captured at `SessionStart` from the hook payload (`workspace.current_dir` or `cwd`). The hook sends it via `herdr pane report-agent-session --project-cwd <dir>` (PS1) or as `project_cwd` JSON param (SH). `PaneAgentSessionSnapshot` persists it. On restore, `persist/restore.rs` uses this path (if present and exists on disk) as the shell's spawn directory for the resume terminal, falling back to the snapshot CWD otherwise.

## Paste handling

Paste text is sent to PTY panes via `encode_paste_payload` in `src/pane.rs`. When the pane has bracketed paste mode enabled (`InputState::bracketed_paste`), the text is wrapped in `\x1b[200~...\x1b[201~`. When not, newlines are backslash-escaped so the shell treats the entire paste as a single command continuation rather than executing on each newline.

## Dynamic agent label CWD (Windows)

On Windows, `PaneRuntime` tracks the foreground subprocess PID in `foreground_pid: Arc<AtomicU32>` (shared with the detection task). The detection loop sets `foreground_pid` to the agent (e.g. Claude) subprocess PID whenever it identifies a foreground agent, and to 0/shell-PID when the shell is foreground. `PaneRuntime.cwd()` on Windows first checks `foreground_pid` — if it differs from `child_pid` (the shell), it calls `platform::process_cwd(foreground_pid)` to read the agent's actual CWD, so the sidebar label dynamically reflects where Claude is working rather than where the shell started.

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
