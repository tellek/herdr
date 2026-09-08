@echo off
cd /d "%~dp0"
set LIBGHOSTTY_VT_PREBUILT=true
cargo build --release --locked
