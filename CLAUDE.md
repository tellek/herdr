# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

AGENTS.md

## Rules

- **ALWAYS capture a passing test baseline before making changes.** Run the full test suite first (via the `windows-tests` skill on Windows) and record the result, so that after your changes you can re-run and diff against the baseline to tell whether a failure was caused by your change or was pre-existing.
- **ALWAYS when starting new work in a fresh session**
    Execute the following in order:
    1. Switch to the master branch
    2. Get latest on the branch
    3. Create a new branch
    4. Run tests to get a baseline
    5. Implement the changes required for the current work
    6. Create/fix unit tests to cover the changes made
    7. Run the unit tests you created, go back to #6 if any failures
    8. Run all test, fix any issues, do not proceed until all tests pass
    9. Update claude.md and agents.md with the appropriate information regarding the changes made in this session
    10. Commit, push, and merge into the master branch (no pull request)
    11. Mark the item as done in the todo.yaml (if you are working from there)

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

**Sidebar layout.** The left sidebar shows only the agents section — the spaces/workspaces list is hidden. The agent panel spans the full sidebar height minus 1 reserved row (always 1, regardless of `mouse_capture`). The last row is a shared footer: the `«` collapse toggle is at column `sidebar.width - 2`, and the `menu` label (rendered only when `mouse_capture=true`) sits directly to its left on the same row. `global_launcher_rect()` and `agent_panel_rect()` use a fixed reservation of 1 row. `workspace_at_row()` always returns `None`; workspace switching goes through the agent list. `workspace_card_areas` is always set to `Vec::new()` in `compute_view_internal`. The collapsed sidebar shows only agent markers (no workspace dots).

## Agent naming

Agent entries in the left sidebar use a two-row display: primary label (row 1, bold) and agent label (row 2, secondary). The primary label is chosen by `TerminalState::primary_display_name()`, in priority order: `manual_label` (herdr pane rename via `pane.rename`) > `agent_name` (`herdr agent rename`) > `session_title` (the Claude session name). When all are `None`, the sidebar falls back to the CWD-derived label (`derive_label_from_cwd`: git repo root name, else folder name).

`session_title` is the live Claude session name and is kept fresh by the live-status poll: `AppState::refresh_agent_live_statuses()` (`src/app/live_status.rs`, every 2 s) reads the statusLine payload yaml for each Claude pane and mirrors its `session_name` field into `set_session_title`. The yaml payload is **authoritative for the current session**: when a payload exists, its `session_name` is written verbatim (`Some` sets it, absent/blank clears it), so a stale title from a closed session is dropped once a new, unnamed session replaces it. Note Claude only re-renders the statusLine (and thus rewrites the yaml) on activity, so a session renamed while idle won't reflect until its next turn. `/rename` is *not* written to the transcript jsonl in current Claude versions — `session_name` in the statusLine payload is the only carrier.

`name_override` in `PaneDetail` carries whatever `primary_display_name()` returns; `agent_label` (row 2) always shows the detected agent type.

## Statusline panel

A 1-row statusline bar sits at the bottom of the terminal pane area (right of the sidebar, spanning to the right edge). It is desktop-only (not rendered in mobile layout). `compute_view_internal` carves it out of `main_terminal_area` before passing `terminal_area` to the pane renderer; `ViewState::statusline_rect` stores its coordinates.

`render_statusline` in `src/ui/statusline.rs` looks up the focused pane's `TerminalState` (via `app.active → ws.active_tab → tab.layout.focused() → tab.terminal_id() → app.terminals`) and renders, in priority order: `terminal.live_status` (preferred), then `terminal.effective_custom_status()`, then the CWD folder name fallback. The `custom_status` string is populated by the Claude integration hook (`herdr-agent-state.ps1`/`.sh`) on `PreToolUse`, `PostToolUse`, and `Stop` events with the `statusline` action. The status persists across tool calls (no TTL) so the last-known info stays visible between events. The integration is installed via `herdr integrate install claude` and is at `CLAUDE_INTEGRATION_VERSION=9`. `normalize_custom_status` in `src/app/api_helpers.rs` strips control characters and caps at 512 chars.

### Live status (statusLine yaml polling)

`live_status: Option<LiveStatus>` on `TerminalState` (not persisted) is the primary statusline source. It is fed by the user's Claude `statusLine` command, which dumps its full stdin payload (raw JSON, valid YAML) to `~/.claude/projects/<encoded-cwd>/<session_id>.yaml` on every render — available immediately, even before the first assistant turn, with fields the transcript JSONL lacks (cost, effort, context %, line counts). herdr polls this on a 2 s timer: `AppState::refresh_agent_live_statuses()` (in `src/app/live_status.rs`) iterates terminals, and for each whose `persisted_agent_session` is a Claude id-session (`TerminalState::claude_session_id()`), globs `~/.claude/projects/*/<session_id>.yaml` (id is the dependable join key — no CWD-encoding guesswork) and parses it into `LiveStatus`. The timer is `App::next_live_status_poll` (`LIVE_STATUS_POLL_INTERVAL = 2 s`), driven from both `runtime.rs` (`run_live_status_poll`) and the headless scheduled-task loop, and included in `next_loop_deadline`.

`LiveStatus` holds only data (model, effort, fast_mode, thinking, context %, input/output tokens, cost, lines added/removed, current_dir, added_dirs count, exceeds_200k); all styling/icons live in `src/ui/statusline.rs::live_status_spans`, which colors via the `Palette` (model: opus→peach, sonnet→teal, haiku→yellow, fable→red; effort: low→teal, medium→green, high→yellow, xhigh→red; context bar→mauve; lines added→green, removed→red; `exceeds_200k`→red). Note `xhigh` effort is Opus-only. The writer half is the user's own `~/.claude/statusline-command.sh` (not in-repo); it renders nothing to stdout — its sole job is the yaml dump for herdr.

**Two external pieces must keep working (both outside this repo, in `~/.claude/`):**

1. **SessionStart hook** — must register the pane's Claude session id with herdr (`herdr pane report-agent-session --agent-session-id <id>`, sent by `herdr-agent-state.ps1`/`.sh`). This populates `persisted_agent_session` (kind `Id`), which is the *only* join key the poll uses. If the hook is missing/not firing, `claude_session_id()` returns `None`, no yaml is located, and the panel silently falls back to the folder name.
2. **statusLine command** — `~/.claude/settings.json` must point `statusLine.command` at `statusline-command.sh`, whose job is: read stdin, take `.session_id` and `.transcript_path`, and write the raw payload to `<transcript-dir>/<session_id>.yaml` (transcript parent dir = the project folder; fall back to encoded-CWD if absent). The filename's `<session_id>` must equal the id the SessionStart hook reported — that equality is what links the two halves. Consumed payload fields: `model.display_name`, `effort.level`, `context_window.{used_percentage,total_input_tokens,total_output_tokens}`, `cost.{total_cost_usd,total_lines_added,total_lines_removed}`, `workspace.{current_dir,added_dirs}`, `exceeds_200k_tokens`, `fast_mode`, `thinking.enabled`. Missing fields degrade gracefully (defaults); a missing `model.display_name` makes the whole payload unusable. A backup of the original rendering script is at `~/.claude/statusline-command.sh.bak`.

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
- A running herdr holds a lock on `target\debug\herdr.exe`, so `cargo build`/`nextest` fails with `failed to remove file ... Access is denied. (os error 5)`. Build/test into an alternate dir instead: `$env:CARGO_TARGET_DIR="C:\GIT\herdr\target-test"`. Once herdr is stopped, build into the normal `target\` so a normal launch picks up the new binary.
- Git hooks (`just install-hooks`) use `chmod` and won't apply on Windows; configure hooks manually if needed

## Testing

Unit tests live next to the code (`#[cfg(test)] mod tests`). New `AppState` or `Workspace` behavior should use `AppState::test_new()` and `Workspace::test_new()` without PTYs. For identity/state refactors, use `AppState::assert_invariants_for_test()` with `AppState::test_with_adversarial_identity_state()`.
