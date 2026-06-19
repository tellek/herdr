#!/bin/sh
# installed by herdr
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=claude
# HERDR_INTEGRATION_VERSION=8

set -eu

action="${1:-}"
hook_input_file="$(mktemp "${TMPDIR:-/tmp}/herdr-claude-hook.XXXXXX")" || exit 0
trap 'rm -f "$hook_input_file"' EXIT HUP INT TERM
cat >"$hook_input_file" 2>/dev/null || true

case "$action" in
  session|statusline) ;;
  *) exit 0 ;;
esac

[ "${HERDR_ENV:-}" = "1" ] || exit 0
[ -n "${HERDR_SOCKET_PATH:-}" ] || exit 0
[ -n "${HERDR_PANE_ID:-}" ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

HERDR_ACTION="$action" HERDR_HOOK_INPUT_FILE="$hook_input_file" python3 - <<'PY'
import json
import os
import random
import socket
import time

source = "herdr:claude"
action = os.environ.get("HERDR_ACTION", "")
pane_id = os.environ.get("HERDR_PANE_ID")
socket_path = os.environ.get("HERDR_SOCKET_PATH")
hook_input_file = os.environ.get("HERDR_HOOK_INPUT_FILE")

if not pane_id or not socket_path:
    raise SystemExit(0)

hook_input = {}
if hook_input_file:
    try:
        with open(hook_input_file, encoding="utf-8") as handle:
            content = handle.read()
        if content.strip():
            hook_input = json.loads(content)
    except Exception:
        hook_input = {}

request_id = f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}"
report_seq = time.time_ns()

if action == "statusline":
    def get_bar(pct):
        bar_width = 10
        filled = min(bar_width, max(0, int(pct / 100.0 * bar_width)))
        return "█" * filled + "░" * (bar_width - filled) + f"{round(pct)}%"

    model_obj = hook_input.get("model") or {}
    model = model_obj.get("display_name") or "Unknown"
    style_obj = hook_input.get("output_style") or {}
    effort = style_obj.get("name") or "default"
    cw = hook_input.get("context_window") or {}
    used_pct = cw.get("used_percentage")
    ctx = f"ctx:[{get_bar(float(used_pct))}]" if used_pct is not None else "ctx:[----------]"
    ti = float(cw.get("total_input_tokens") or 0)
    to_ = float(cw.get("total_output_tokens") or 0)
    cu = cw.get("current_usage") or {}
    tw = float(cu.get("cache_creation_input_tokens") or 0)
    tr = float(cu.get("cache_read_input_tokens") or 0)
    cost = f"${(ti / 1e6 * 3.00 + to_ / 1e6 * 15.00 + tw / 1e6 * 3.75 + tr / 1e6 * 0.30):.4f}"
    rl = (hook_input.get("rate_limits") or {}).get("five_hour") or {}
    five_pct = rl.get("used_percentage")
    pts = f"pts:[{get_bar(float(five_pct))}]" if five_pct is not None else "pts:[----------]"
    ws = hook_input.get("workspace") or {}
    cwd = ws.get("current_dir") or hook_input.get("cwd") or "."
    folder = cwd.rstrip("/\\").split("/")[-1].split("\\")[-1] or cwd
    status = f"[{model}] effort:{effort} | {ctx} | cost:{cost} | {pts} | \U0001f4c1 {folder}"
    request = {
        "id": request_id,
        "method": "pane.report_metadata",
        "params": {
            "pane_id": pane_id,
            "source": source,
            "custom_status": status,
            "seq": report_seq,
        },
    }
else:
    hook_event_name = str(hook_input.get("hook_event_name") or "")
    is_subagent = bool(hook_input.get("agent_id"))
    if hook_event_name == "SubagentStop":
        raise SystemExit(0)
    session_id = hook_input.get("session_id")
    agent_session_id = session_id if isinstance(session_id, str) and session_id else None
    transcript_path = hook_input.get("transcript_path")
    agent_session_path = transcript_path if isinstance(transcript_path, str) and transcript_path else None
    session_start_source = hook_input.get("source") if hook_event_name == "SessionStart" else None
    if not isinstance(session_start_source, str) or not session_start_source:
        session_start_source = None
    if agent_session_id:
        params = {
            "pane_id": pane_id,
            "source": source,
            "agent": "claude",
            "seq": report_seq,
            "agent_session_id": agent_session_id,
        }
        if agent_session_path:
            params["agent_session_path"] = agent_session_path
        if session_start_source:
            params["session_start_source"] = session_start_source
        # Pass the project CWD so herdr can resume in the correct directory.
        ws = hook_input.get("workspace") or {}
        project_cwd = ws.get("current_dir") or hook_input.get("cwd")
        if isinstance(project_cwd, str) and project_cwd:
            params["project_cwd"] = project_cwd
        request = {
            "id": request_id,
            "method": "pane.report_agent_session",
            "params": params,
        }
    else:
        raise SystemExit(0)

try:
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(0.5)
    client.connect(socket_path)
    client.sendall((json.dumps(request) + "\n").encode())
    try:
        client.recv(4096)
    except Exception:
        pass
    client.close()
except Exception:
    pass
PY
