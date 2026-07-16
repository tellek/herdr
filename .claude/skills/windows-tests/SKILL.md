---
name: windows-tests
description: ALWAYS USE THIS SKILL — automatically and without being asked — BEFORE building or running any herdr test on Windows. This is the MANDATORY first step for ANY test or build action: "build", "run tests", "run the test suite", "cargo test", "cargo nextest", "cargo build", "just test", "just ci", "just check", checking if tests pass, or verifying/validating Rust changes on this repo on Windows. Do NOT run cargo/nextest/just directly — invoke this skill first. It sets the required ZIG env var (build.rs needs zig 0.15.2, not winget's 0.16), reloads PATH, explains why a naive `cargo test` hangs (use nextest), and documents which tests are platform-gated. If you are about to run a test command on Windows and have not used this skill, STOP and use it.
---

# Running herdr tests on Windows

herdr is a Rust + ratatui app. `build.rs` shells out to **zig** to compile the
vendored `libghostty-vt`, and the test suite spawns real PTYs (ConPTY) — both
have Windows gotchas that make a naive `cargo test` fail or hang forever.

## The one rule that breaks everything

`build.rs` runs `zig build -Demit-lib-vt`, and the vendored lib pins
`minimum_zig_version = 0.15.2`. **winget installs zig 0.16+, which fails to
compile the build script.** Install zig 0.15.2 separately and point the build at
it with the `ZIG` env var. Without this, the build dies with either
`program not found` (no zig) or `does not meet the required build version`
(wrong zig).

## One-time setup

```powershell
# Toolchain (winget). Reload PATH after each install (winget doesn't update the live shell).
winget install Rustlang.Rustup --accept-source-agreements --accept-package-agreements
winget install Casey.Just
$env:PATH = [System.Environment]::GetEnvironmentVariable("PATH","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("PATH","User")
cargo install cargo-nextest --locked

# zig 0.15.2 — NOT winget (winget gives 0.16+ which is too new for build.rs)
Invoke-WebRequest "https://ziglang.org/download/0.15.2/zig-x86_64-windows-0.15.2.zip" -OutFile C:\tools\zig-0.15.2.zip
Expand-Archive C:\tools\zig-0.15.2.zip C:\tools\ -Force
# zig.exe is now at C:\tools\zig-x86_64-windows-0.15.2\zig.exe
```

## Every build/test session: set PATH + ZIG first

```powershell
$env:PATH = [System.Environment]::GetEnvironmentVariable("PATH","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("PATH","User")
$env:ZIG  = "C:\tools\zig-x86_64-windows-0.15.2\zig.exe"
```

## Build

```powershell
cargo build --locked          # first build runs zig; ~90s cold
```

## Run the tests — use nextest, never `cargo test`

A plain `cargo test` runs all tests in one process; a few `app::*` tests spawn
real PTYs whose ConPTY teardown intermittently **deadlocks on Windows**, so the
whole run hangs with no output. `cargo nextest run` isolates each test in its
own process and (via `.config/nextest.toml`) bounds + retries them, so a hung
test is killed and named instead of blocking the suite.

```powershell
cargo nextest run --locked --no-fail-fast
```

`.config/nextest.toml` already applies a **Windows-only** `slow-timeout`
(terminate-after) + `retries = 2` to absorb the flaky ConPTY-teardown race.
Unix/macOS runs are unaffected. Expected baseline: **all bin tests pass**
(~1844). If a test times out on all 3 retries it's a *deterministic* Windows
hang — guard it with `#[cfg(not(windows))]`, don't keep retrying.

## What does NOT run on Windows (by design)

- **`tests/*.rs` integration tests** use `std::os::unix::net::UnixStream` /
  `FileType::is_socket()` and are gated with `#![cfg(unix)]`. They compile and
  run only on unix. (nextest still *compiles* every test target, so these guards
  are required for nextest to build at all on Windows.)
- A set of `app::*` tests that spawn real PTYs/worktrees (pane-split, worktree
  open/create, layout-apply, workspace-create) are gated `#[cfg(not(windows))]`
  because their ConPTY teardown hangs. Search the tree for `cfg(not(windows))`
  to see the full list.

## `just` on Windows

`just check` includes `windows-lint`, which uses Unix env-var syntax and won't
run on native Windows. Use the pieces directly:

```powershell
just ci                       # fmt + clippy + nextest (works once ZIG is set)
cargo nextest run --locked    # tests only
```
