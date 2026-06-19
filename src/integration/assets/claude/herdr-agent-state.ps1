# installed by herdr
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=claude
# HERDR_INTEGRATION_VERSION=8

param([string]$Action = "")

if ($Action -ne "session" -and $Action -ne "statusline") { exit 0 }
if ($env:HERDR_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:HERDR_PANE_ID)) { exit 0 }

$inputText = [Console]::In.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json }
} catch {
    exit 0
}

if ($Action -eq "statusline") {
    function Get-HerdrBar {
        param([double]$pct)
        $barWidth = 10
        $filled = [int][Math]::Floor(($pct / 100.0) * $barWidth)
        if ($filled -lt 0) { $filled = 0 }
        if ($filled -gt $barWidth) { $filled = $barWidth }
        ([string][char]0x2588 * $filled) + ([string][char]0x2591 * ($barWidth - $filled)) + "$([Math]::Round($pct, 0))%"
    }
    $model   = if ($payload -and $payload.model -and $payload.model.display_name) { $payload.model.display_name } else { "Unknown" }
    $effort  = if ($payload -and $payload.output_style -and $payload.output_style.name) { $payload.output_style.name } else { "default" }
    $usedPct = if ($payload -and $payload.context_window -and $null -ne $payload.context_window.used_percentage) { $payload.context_window.used_percentage } else { $null }
    $ctx     = if ($null -ne $usedPct) { "ctx:[$(Get-HerdrBar -pct ([double]$usedPct))]" } else { "ctx:[----------]" }
    $cw      = if ($payload) { $payload.context_window } else { $null }
    $ti      = if ($cw -and $null -ne $cw.total_input_tokens)  { [double]$cw.total_input_tokens }  else { 0 }
    $to_     = if ($cw -and $null -ne $cw.total_output_tokens) { [double]$cw.total_output_tokens } else { 0 }
    $cu      = if ($cw) { $cw.current_usage } else { $null }
    $tw      = if ($cu -and $null -ne $cu.cache_creation_input_tokens) { [double]$cu.cache_creation_input_tokens } else { 0 }
    $tr      = if ($cu -and $null -ne $cu.cache_read_input_tokens)     { [double]$cu.cache_read_input_tokens }     else { 0 }
    $cost    = "`$" + (($ti / 1e6 * 3.00) + ($to_ / 1e6 * 15.00) + ($tw / 1e6 * 3.75) + ($tr / 1e6 * 0.30)).ToString("F4")
    $fivePct = if ($payload -and $payload.rate_limits -and $payload.rate_limits.five_hour -and $null -ne $payload.rate_limits.five_hour.used_percentage) { $payload.rate_limits.five_hour.used_percentage } else { $null }
    $pts     = if ($null -ne $fivePct) { "pts:[$(Get-HerdrBar -pct ([double]$fivePct))]" } else { "pts:[----------]" }
    $cwd     = if ($payload -and $payload.workspace -and $payload.workspace.current_dir) { $payload.workspace.current_dir } elseif ($payload -and $payload.cwd) { $payload.cwd } else { "." }
    $folder  = Split-Path -Leaf $cwd.TrimEnd('/\')
    if (-not $folder) { $folder = $cwd }
    $emoji   = [char]::ConvertFromUtf32(0x1F4C1)
    $status  = "[$model] effort:$effort | $ctx | cost:$cost | $pts | $emoji $folder"
    $seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    try {
        & herdr pane report-metadata $env:HERDR_PANE_ID --source "herdr:claude" --custom-status $status --seq "$seq" 2>$null | Out-Null
    } catch {}
    exit 0
}

if ($payload.hook_event_name -eq "SubagentStop") { exit 0 }

$sessionId = $payload.session_id
if ([string]::IsNullOrWhiteSpace($sessionId)) { exit 0 }

$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
try {
    $args = @(
        "pane",
        "report-agent-session",
        $env:HERDR_PANE_ID,
        "--source",
        "herdr:claude",
        "--agent",
        "claude",
        "--seq",
        "$seq",
        "--agent-session-id",
        "$sessionId"
    )
    if ($payload.transcript_path -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.transcript_path)) {
        $args += @("--agent-session-path", "$($payload.transcript_path)")
    }
    if ($payload.hook_event_name -eq "SessionStart" -and $payload.source -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.source)) {
        $args += @("--session-start-source", "$($payload.source)")
    }
    # Pass the project CWD so herdr can resume in the correct directory.
    $projectCwd = if ($payload.workspace -and $payload.workspace.current_dir) { $payload.workspace.current_dir } elseif ($payload.cwd) { $payload.cwd } else { $null }
    if (-not [string]::IsNullOrWhiteSpace($projectCwd)) {
        $args += @("--project-cwd", "$projectCwd")
    }
    & herdr @args 2>$null | Out-Null
} catch {
}
