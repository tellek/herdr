---
name: windows-build
description: ALWAYS USE THIS SKILL — automatically and without being asked — BEFORE building herdr on Windows. MANDATORY first step for ANY build action: "build", "cargo build", "cargo build --release", "compile herdr", "build the binary", or before running the app/tests on this repo on Windows. Do NOT run cargo build directly — invoke this skill first. It installs/sets the required toolchain and the ZIG env var (build.rs needs zig 0.15.2, NOT winget's 0.16), reloads PATH after winget, and explains the two build.rs failure modes. If you are about to build on Windows and have not used this skill, STOP and use it.
---

# Building herdr on Windows

The build itself is a plain `cargo build` — the complexity is the toolchain.
`build.rs` shells out to **zig** to compile the vendored `libghostty-vt`, and the
zig version requirement is strict and non-obvious.

## The one rule that breaks everything

`build.rs` runs `zig build -Demit-lib-vt`, and the vendored lib pins
`minimum_zig_version = 0.15.2`. **`winget install zig.zig` gives 0.16+, which
fails the build.** Install zig 0.15.2 separately and point the build at it with
the `ZIG` env var. The two failure modes:

- `failed to execute zig build ... program not found` → no zig on PATH / `ZIG` unset.
- `Your Zig version v0.16.0 does not meet the required build version of v0.15.2`
  → wrong zig (winget's). Set `ZIG` to the 0.15.2 binary.

## One-time setup

```powershell
# Rust toolchain (winget). Reload PATH after each install — winget does not update the live shell.
winget install Rustlang.Rustup --accept-source-agreements --accept-package-agreements
$env:PATH = [System.Environment]::GetEnvironmentVariable("PATH","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("PATH","User")

# zig 0.15.2 — NOT winget (winget gives 0.16+, too new for build.rs)
Invoke-WebRequest "https://ziglang.org/download/0.15.2/zig-x86_64-windows-0.15.2.zip" -OutFile C:\tools\zig-0.15.2.zip
Expand-Archive C:\tools\zig-0.15.2.zip C:\tools\ -Force
# zig.exe is now at C:\tools\zig-x86_64-windows-0.15.2\zig.exe
```

## Every build session: set PATH + ZIG first

```powershell
$env:PATH = [System.Environment]::GetEnvironmentVariable("PATH","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("PATH","User")
$env:ZIG  = "C:\tools\zig-x86_64-windows-0.15.2\zig.exe"
```

## Build

**Default to debug unless explicitly asked for a release build.**

```powershell
cargo build --locked              # DEFAULT — debug binary; first build runs zig, ~90s cold
cargo build --release --locked    # release only when explicitly requested
```

The `ZIG` env var and PATH must be set in the same shell that runs cargo
(shell state does not persist between tool calls).

## Next steps

To run tests after building, use the `windows-tests` skill (it relies on this
same `ZIG`/PATH setup and uses nextest, because a naive `cargo test` hangs on
the ConPTY-teardown race).
