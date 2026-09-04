# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

AGENTS.md

## Rules

- **ALWAYS capture a passing test baseline before making changes.** Run the full test suite first (via the `windows-tests` skill on Windows) and record the result, so that after your changes you can re-run and diff against the baseline to tell whether a failure was caused by your change or was pre-existing.
- **ALWAYS when starting new work in a fresh session**
    Execute the following in order, working directly on `master` (no feature branches, no pull requests — commits on master are the rollback points):
    1. Switch to the master branch
    2. Get latest on the branch
    3. Run tests to get a baseline
    4. Implement the changes required for the current work
    5. Create/fix unit tests to cover the changes made
    6. Run the unit tests you created, go back to #5 if any failures
    7. Run all test, fix any issues, do not proceed until all tests pass
    8. Update claude.md and agents.md with the appropriate information regarding the changes made in this session
    9. Commit and push to master
    10. Mark the item as done in the todo.yaml (if you are working from there)

## Project

Herdr is a terminal workspace manager for AI coding agents, built with Rust and ratatui. It runs as a server/client pair communicating over a local socket (Unix socket on Linux/macOS, named pipe on Windows via `interprocess`). Windows support is preview beta.
- The primary branch is 'master'

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

**Agent panel ordering.** The agent panel is always sorted by originating tab (group), then alphabetically by label within each tab — there is no user-facing sort mode toggle. `agent_panel_entries_with_runtimes()` (`src/ui/sidebar.rs`) sorts the flat entry list by `(ws_idx, tab_idx, primary_label.to_lowercase())`. The former `AgentPanelSort`/`AgentPanelSortConfig` "grouped"/"priority" toggle (state field, config key, header click target, mouse hit-test) was removed entirely (issue #7); a legacy `agent_panel_sort` key in an old config file is silently ignored rather than erroring.

**Panel tracks all terminal sessions, not just detected agents (issue #6).** `Tab::pane_details()` (`src/workspace/aggregate.rs`) used to filter out any pane whose terminal had neither an explicit `agent_name` nor a detected agent (`effective_agent_label()`), so plain shells never appeared in the sidebar. That filter is gone — every attached pane now produces a `PaneDetail`/`AgentPanelEntry`. `PaneDetail::agent_label` and `AgentPanelEntry::agent_label` are `Option<String>`: `Some(type)` when an agent is detected/named, `None` for a plain shell. `src/ui/sidebar.rs`'s status row already skipped the `· <agent_label>` segment when absent, so no render change was needed there beyond the type change. A plain shell renders with `AgentState::Unknown`, which displays as the "idle" status label with a dim dot — it's visually indistinguishable from an idle agent except for the missing `· <type>` suffix.

## Agent naming

Agent entries in the left sidebar use a two- or three-row display: primary label (row 1, bold), an optional tab-name row (row 2, only when the entry's workspace has more than one tab — `AgentPanelEntry::primary_tab_label`), and a status row (status · agent type · custom status). Each entry's height is `agent_panel_entry_height()` (`src/ui/sidebar.rs`): 3 rows when `primary_tab_label` is `Some`, else 2; `agent_panel_visible_count`/`agent_panel_scroll_metrics` and the click-mapping in `agent_detail_target_at` (`src/app/input/sidebar.rs`) both walk entries using this per-entry height rather than assuming a fixed row count, since entries can now differ in height within the same list. The primary label is chosen by `TerminalState::primary_display_name()`, in priority order: `manual_label` (herdr pane rename via `pane.rename`) > `agent_name` (`herdr agent rename`) > `session_title` (the Claude session name). When all are `None`, the sidebar falls back to the CWD-derived label (`derive_label_from_cwd`: git repo root name, else folder name).

`session_title` is the live Claude session name and is kept fresh by the live-status poll: `AppState::refresh_agent_live_statuses()` (`src/app/live_status.rs`, every 2 s) reads the statusLine payload yaml for each Claude pane and mirrors its `session_name` field into `set_session_title`. The yaml payload is **authoritative for the current session**: when a payload exists, its `session_name` is written verbatim (`Some` sets it, absent/blank clears it), so a stale title from a closed session is dropped once a new, unnamed session replaces it. Note Claude only re-renders the statusLine (and thus rewrites the yaml) on activity, so a session renamed while idle won't reflect until its next turn. `/rename` is *not* written to the transcript jsonl in current Claude versions — `session_name` in the statusLine payload is the only carrier.

`name_override` in `PaneDetail` carries whatever `primary_display_name()` returns; `agent_label` (row 2) always shows the detected agent type.

## Statusline panel

A 1-row statusline bar sits at the bottom of the terminal pane area (right of the sidebar, spanning to the right edge). It is desktop-only (not rendered in mobile layout). `compute_view_internal` carves it out of `main_terminal_area` before passing `terminal_area` to the pane renderer; `ViewState::statusline_rect` stores its coordinates.

`render_statusline` in `src/ui/statusline.rs` looks up the focused pane's `TerminalState` (via `app.active → ws.active_tab → tab.layout.focused() → tab.terminal_id() → app.terminals`) and renders, in priority order: `terminal.live_status` (preferred), then `terminal.effective_custom_status()`, then the CWD folder name fallback. The `custom_status` string is populated by the Claude integration hook (`herdr-agent-state.ps1`/`.sh`) on `PreToolUse`, `PostToolUse`, and `Stop` events with the `statusline` action. The status persists across tool calls (no TTL) so the last-known info stays visible between events. The integration is installed via `herdr integrate install claude` and is at `CLAUDE_INTEGRATION_VERSION=9`. `normalize_custom_status` in `src/app/api_helpers.rs` strips control characters and caps at 512 chars.

The CWD fallback resolves the *live* foreground CWD, not `TerminalState::cwd`: `render_statusline` takes the `TerminalRuntimeRegistry` and calls `Tab::cwd_for_pane` + `derive_label_from_cwd`, matching the sidebar label (git repo root name), so a shell spawned in `C:\GIT` no longer displays `GIT` for a pane working inside a repo below it.

Selection highlight: `automatic_selection_bg` (`src/ui/panes.rs`) blends the host terminal background 55% toward `Palette::accent` (panel background when the host reports none, white/black when the accent is not RGB), so drag selections stay visible on dark themes like Satisfactory.

### Live status (statusLine yaml polling)

`live_status: Option<LiveStatus>` on `TerminalState` (not persisted) is the primary statusline source. It is fed by the user's Claude `statusLine` command, which dumps its full stdin payload (raw JSON, valid YAML) to `~/.claude/projects/<encoded-cwd>/<session_id>.yaml` on every render — available immediately, even before the first assistant turn, with fields the transcript JSONL lacks (cost, effort, context %, line counts). herdr polls this on a 2 s timer: `AppState::refresh_agent_live_statuses()` (in `src/app/live_status.rs`) iterates terminals, and for each whose `persisted_agent_session` is a Claude id-session (`TerminalState::claude_session_id()`), globs `~/.claude/projects/*/<session_id>.yaml` (id is the dependable join key — no CWD-encoding guesswork) and parses it into `LiveStatus`. For any terminal whose `claude_session_id()` is `None` (e.g. Claude was closed, so the detected agent disappeared and the persisted session was cleared), the poll clears `live_status` and `session_title` so the statusline and sidebar label revert to the CWD folder instead of keeping stale Claude data. The timer is `App::next_live_status_poll` (`LIVE_STATUS_POLL_INTERVAL = 2 s`), driven from both `runtime.rs` (`run_live_status_poll`) and the headless scheduled-task loop, and included in `next_loop_deadline`.

`LiveStatus` holds only data (model, effort, fast_mode, thinking, context %, input/output tokens, cost, lines added/removed, current_dir, added_dirs count, exceeds_200k, session_id); all styling/icons live in `src/ui/statusline.rs::live_status_spans`, which colors via the `Palette` (model: opus→peach, sonnet→teal, haiku→yellow, fable→red; effort: low→teal, medium→green, high→yellow, xhigh→red; context bar→mauve; lines added→green, removed→red; `exceeds_200k`→red). Note `xhigh` effort is Opus-only. The writer half is the user's own `~/.claude/statusline-command.sh` (not in-repo); it renders nothing to stdout — its sole job is the yaml dump for herdr.

**Two external pieces must keep working (both outside this repo, in `~/.claude/`):**

1. **SessionStart hook** — must register the pane's Claude session id with herdr (`herdr pane report-agent-session --agent-session-id <id>`, sent by `herdr-agent-state.ps1`/`.sh`). This populates `persisted_agent_session` (kind `Id`), which is the *only* join key the poll uses. If the hook is missing/not firing, `claude_session_id()` returns `None`, no yaml is located, and the panel silently falls back to the folder name.
2. **statusLine command** — `~/.claude/settings.json` must point `statusLine.command` at `statusline-command.sh`, whose job is: read stdin, take `.session_id` and `.transcript_path`, and write the raw payload to `<transcript-dir>/<session_id>.yaml` (transcript parent dir = the project folder; fall back to encoded-CWD if absent). The filename's `<session_id>` must equal the id the SessionStart hook reported — that equality is what links the two halves. Consumed payload fields: `model.display_name`, `effort.level`, `context_window.{used_percentage,total_input_tokens,total_output_tokens}`, `cost.{total_cost_usd,total_lines_added,total_lines_removed}`, `workspace.{current_dir,added_dirs}`, `exceeds_200k_tokens`, `fast_mode`, `thinking.enabled`. Missing fields degrade gracefully (defaults); a missing `model.display_name` makes the whole payload unusable. A backup of the original rendering script is at `~/.claude/statusline-command.sh.bak`.

## Claude session resume CWD

When herdr auto-resumes a Claude session it runs `claude --resume <id>` inside a shell spawned at the pane's saved CWD. Historically that was a problem: if the CWD was a subdirectory Claude had navigated into, older Claude versions could not find the session (sessions were keyed to the project root where `claude` was originally launched). See issue #13 below for the current precedence.

`TerminalState.agent_session_project_cwd: Option<PathBuf>` stores the project root CWD, captured at `SessionStart` from the hook payload (`workspace.current_dir` or `cwd`). The hook sends it via `herdr pane report-agent-session --project-cwd <dir>` (PS1) or as `project_cwd` JSON param (SH). `PaneAgentSessionSnapshot` persists it. On restore, `persist/restore.rs` uses this path as the shell's spawn directory only when the saved pane CWD no longer exists on disk (see issue #13 below).

The hook script reports `project_cwd` on *every* "session" hook call, not just `SessionStart` (PreToolUse/PostToolUse/Stop also call `report-agent-session`). Only `SessionStart` sends `--session-start-source`, so `AgentSessionReported`'s handler in `src/app/actions.rs` only overwrites `agent_session_project_cwd` when `session_start_source.is_some()` (or when nothing is stored yet) — otherwise a later hook event's `cwd`, which can reflect a subdirectory the agent `cd`'d into mid-session, would clobber the correct project root and break resume (reported in issue #9 as "resuming all terminals in C:\GIT instead of the project folder"). The restore-time HOME fallback (`persist/restore.rs`, used when a saved pane CWD no longer exists on disk) also checks `USERPROFILE` on Windows, since `HOME` is typically unset there.

**Resume lands in the folder the pane was left in (issue #13).** The precedence above is inverted from what issues #5/#9 originally shipped. `persist/restore.rs` now uses the *saved pane CWD* as the resume spawn directory whenever it still exists on disk; `PaneAgentSessionSnapshot.project_cwd` is consulted only when that folder is gone, and the git-repo-root walk-up was dropped entirely (`git_repo_root` went back to `pub(super)` and is no longer re-exported through `workspace::git`/`workspace`). Preferring an inferred project root silently moved a resumed pane out of the directory the user left it in — and the resumed agent inherits that directory as its own CWD, so a later `/clear` starts a fresh session pointed at the wrong folder. Current Claude versions locate a session id regardless of the launch directory, so the walk-up no longer buys anything.

**Restored panes are `cd`'d explicitly, and the agent resume is typed 300 ms later.** `CommandBuilder::cwd` alone does not reliably place a restored pane (panes came back in the directory herdr itself was launched from), so `persist/restore.rs` types a directory-change line into every restored plain shell, and `start_pending_agent_resume` (`src/app/agent_resume.rs`) does the same before an agent resume. The two cannot share one line: a combined `Set-Location ...; claude --resume ...` ran the agent before the directory change had taken effect, so the resume command is queued instead and typed `AGENT_RESUME_COMMAND_DELAY` (300 ms, `src/app/mod.rs`) later.

The queue is `App::deferred_pane_inputs: Vec<DeferredPaneInput>` (terminal id, `due` instant, line). `App::flush_due_pane_inputs(now)` types every entry whose delay has elapsed and drops entries whose pane no longer has a runtime; it is called from all three loop drive points (`app/runtime.rs`, the two in `server/headless.rs`, and the render loop in `app/mod.rs`) and its earliest `due` is folded into `next_loop_deadline_with_resize_poll`.

The `cd` line itself comes from `crate::pane::shell_cd_command(dir, shell_config)`: `Set-Location -LiteralPath '<dir>'` when `pane_shell` resolves to PowerShell (`-LiteralPath` avoids glob expansion), `cd '<dir>'` otherwise. Quoting lives in `pane.rs` as `powershell_shell_quote` (doubles `'`) and `posix_shell_quote` (the argv quoter `shell_command_from_argv` also uses).

## Paste handling

Paste text is sent to PTY panes via `encode_paste_payload` in `src/pane.rs`. Line endings are first normalized (`\r\n`/`\r` → `\n`) in both paths so stray Windows carriage returns aren't interpreted as Enter by the inner app (e.g. Claude submitting on the first line). When the pane has bracketed paste mode enabled (`InputState::bracketed_paste`), the normalized text is wrapped in `\x1b[200~...\x1b[201~`. When not, newlines are backslash-escaped so the shell treats the entire paste as a single command continuation rather than executing on each newline.

On Windows the paste payload reaches the client one console event at a time, so `windows_stdin_reader_loop` (`src/client/input.rs`) reassembles the bracketed-paste byte stream (`\x1b[200~…\x1b[201~`) in `RawInputFramer` while `raw_sequence_pending` is set. Newlines and tabs inside the paste surface as `KeyCode::Enter`/`KeyCode::Tab`; `windows_key_raw_bytes` feeds those back as raw `\r`/`\t` bytes *while a sequence is pending* so they stay part of the paste instead of interrupting it and reaching the pane as a literal Enter (which made Claude submit on the first pasted line). Outside a pending sequence, Enter/Tab remain normal semantic keys.

**Windows requires `ENABLE_VIRTUAL_TERMINAL_INPUT`.** That reassembly only works if the `\x1b[200~`/`\x1b[201~` markers actually arrive. The Windows console *strips* those markers from the input stream unless VT input is enabled on the console input handle — without it a paste arrives as a bare burst of `Char`+`Enter` key events with no markers, `raw_sequence_pending` never arms, and each pasted newline reaches the PTY as a bare `\r` (whichever ones land while the inner app is idle submit, hence the "first/random newline submits" symptom). `crate::platform::enable_console_vt_input()` (`src/platform/windows.rs`; no-op stub off Windows in `src/platform/mod.rs`) sets the flag during terminal setup (`setup_terminal_with_capabilities` in `src/client/mod.rs`, and the monolithic path in `src/main.rs`), returning the previous mode; `restore_console_input_mode` restores it on teardown (via `TerminalGuard`/panic hook) *before* raw mode is cleared. crossterm 0.29 never sets this flag itself and never emits `Event::Paste` on Windows, which is why the markers are the join point.

As a fallback for consoles where the markers never arrive, `PasteBurstAccumulator` (`src/client/input.rs`, `#[cfg(windows)]`) coalesces the bare keystroke burst directly: `windows_stdin_reader_loop` feeds each event to `observe()`, which buffers consecutive unmodified printable / `Enter` (`\n`) / `Tab` (`\t`) presses (ignoring `Release` events). The run is finalized on an idle `poll` (`Ok(false)`) or when a non-text key arrives; `finish()` emits one `ClientInputEvent::Paste` when paste-shaped: `>= 2` chars **and** contains `\n`, **or** the run is `>= LONG_RUN_PASTE_THRESHOLD` (24) chars even with no embedded newline — a single unbroken line (a URL, or a paragraph pasted with no hard line breaks) is still a zero-gap burst that real typing essentially never produces at that length, and without this it was replayed as thousands of individual key events instead of one paste, which risked dropped/incomplete text (issue #8). Otherwise it replays the buffered chars as individual `Key` events via `client_key_for_char`. This guarantees a lone Enter (`"\n"`, length 1) still submits and short single-line input is unchanged. The accumulator runs **after** the `windows_key_raw_bytes`/`RawInputFramer` path so in-flight bracketed-paste reassembly, mouse, and arrow sequences keep their existing behavior.

## Text selection (mouse + keyboard)

A drag selection in a terminal pane stays highlighted after mouse release instead of auto-copying. The mouse-up handler (`src/app/input/mouse.rs`) only calls `Selection::finish()` to mark the selection `Done` and keep the highlight visible. Acting on the selection is keyboard-driven: **Ctrl+C** copies the text (highlight stays, copy toast shows), **Delete/Backspace** deletes it from the pane (highlight clears). Both keys are intercepted at the top of `prepare_terminal_key_forward` (`src/app/input/terminal.rs`) *before* the usual clear-on-keypress wipes the highlight, gated by `AppState::has_visible_selection()`. `copy_visible_selection` sets `request_clipboard_write`; `delete_visible_selection_bytes` emits one `0x7f` (backspace) per selected character (counted from `extract_selection`). Both `handle_terminal_key` and `handle_terminal_key_headless` then drain the pending clipboard write via `forward_pending_clipboard_write` (the `ClipboardWrite` event path, identical to copy mode, which shows the "copied to clipboard" feedback). Double-click word copy is unchanged. When no selection is visible, Ctrl+C/Delete forward to the pane normally (interrupt / edit).

## Dynamic agent label CWD (Windows)

On Windows, `PaneRuntime` tracks the foreground subprocess PID in `foreground_pid: Arc<AtomicU32>` (shared with the detection task). The detection loop sets `foreground_pid` to the agent (e.g. Claude) subprocess PID whenever it identifies a foreground agent, and to 0/shell-PID when the shell is foreground. `PaneRuntime.cwd()` on Windows first checks `foreground_pid` — if it differs from `child_pid` (the shell), it calls `platform::process_cwd(foreground_pid)` to read the agent's actual CWD, so the sidebar label dynamically reflects where Claude is working rather than where the shell started.

## Sidebar status label for non-agent panes

When a pane's `agent_label` is `None` (a plain shell, or a running process that isn't a recognized coding agent), the agent panel status row shows the actual foreground process's display name (e.g. `pwsh`, or a .NET console app's name minus `.exe`) instead of the generic `idle` state label. `agent_panel_status_label()` (`src/ui/sidebar.rs`) picks `AgentPanelEntry.foreground_display_name` when `agent_label` is `None` and it's `Some`, else falls back to the usual state label; the mobile layout (`src/ui/mobile.rs::mobile_agent_detail`) applies the same precedence. `foreground_display_name` is resolved per-entry in `agent_panel_entries_from` via `Tab::foreground_display_name_for_pane` → `TerminalRuntime::foreground_display_name` → `PaneRuntime::foreground_display_name`, which reads the same `foreground_pid` atomic used for the CWD feature below (falling back to `child_pid` when 0) and resolves it to a name with the new cross-platform `platform::process_name(pid)` (Windows: PEB `image_path_name`; Linux: `/proc/<pid>/stat` `comm`; macOS: `proc_pidinfo` `pbi_comm`), normalized by `detect::display_process_name` (strips path prefix and a case-insensitive trailing `.exe`, preserving casing). This is a live, on-demand lookup (like `cwd()`), not routed through the `AppEvent::StateChanged` channel — the channel only fires on agent-identity transitions, which would miss updates when switching between two *unrecognized* foreground processes (e.g. pwsh → an unknown console app → pwsh) with no agent ever detected.

## Windows process snapshots (lazy command lines, issue #12)

`snapshot_processes()` (`src/platform/windows.rs`) captures only pid/parent/name from the toolhelp snapshot. Command lines are hydrated lazily by `hydrate_command_lines(entries, pids)` for just the pids a caller inspects — `foreground_job` hydrates the pane shell + descendants, `foreground_group_leader_job` one pid, `session_processes` none. Previously every detection tick (300–500 ms per pane) read the command line of every process on the system (OpenProcess + NtQueryInformationProcess + 2× ReadProcessMemory + CommandLineToArgvW each), which was the allocation-churn source behind the growing memory footprint reported in issue #12. Unhydrated entries keep `argv0 = name`.

## Windows-specific notes

- Config and logs: `%APPDATA%\herdr\` (e.g. `C:\Users\<user>\AppData\Roaming\herdr\`)
- Agent detection overrides: `%APPDATA%\herdr\agent-detection\<agent>.toml`
- IPC uses Windows named pipes via `interprocess` — the socket "path" is actually a named pipe name under `GenericNamespaced`
- Keyboard enhancement flags (Kitty protocol) are no-ops on Windows (`src/main.rs`)
- Clipboard image bridging is not yet wired on Windows (`src/platform/windows.rs`)
- Desktop notifications are not yet implemented on Windows
- When running a debug build inside a live herdr session, clear socket overrides in PowerShell: `$env:HERDR_SOCKET_PATH=$null; $env:HERDR_CLIENT_SOCKET_PATH=$null; cargo run -- <command>`
- A running herdr holds a lock on `target\debug\herdr.exe`, so `cargo build`/`nextest` fails with `failed to remove file ... Access is denied. (os error 5)`. For **tests only**, build into the alternate dir: `$env:CARGO_TARGET_DIR="C:\GIT\herdr\target-test"`. **NEVER use `target-test` for deployment builds** — always build into the normal `target\` (`cargo build --locked` or `--release`) so the binary the user launches picks up the new code. If herdr is running and locks the debug binary, ask Topher to close it before building.
- Git hooks (`just install-hooks`) use `chmod` and won't apply on Windows; configure hooks manually if needed

## Testing

Unit tests live next to the code (`#[cfg(test)] mod tests`). New `AppState` or `Workspace` behavior should use `AppState::test_new()` and `Workspace::test_new()` without PTYs. For identity/state refactors, use `AppState::assert_invariants_for_test()` with `AppState::test_with_adversarial_identity_state()`.
