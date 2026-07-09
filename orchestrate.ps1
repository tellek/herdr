# orchestrate-infinite.ps1
param (
    [string]$SessionId = $null
)

$IssuesRepo = "tellek/herdr"
$Prompt = "Use the get-github-issue skill to claim the next issue to work on from https://github.com/$IssuesRepo/issues. You will be working autonomously, so do not ask the user any questions, use your best judgement instead. With that issue in mind, execute the following in order: 1. Switch to the master branch 2. Get latest on the branch 3. Create a new branch 4. Run tests to get a baseline 5. Implement the change 6. Create/fix unit tests to cover the changes made 7. Run the unit tests you created, go back to #6 if any failures 8. Run all tests, fix any issues, do not proceed until all tests pass 9. Update claude.md and agents.md with the appropriate information regarding the changes made in this session 10. Commit, push the new branch, and merge it into master (no pull request) 11. Use the comment-close-issue skill to close the issue, passing a closing comment you compose summarizing what was done - do not comment or close the issue manually with 'gh issue comment'/'gh issue close' before invoking the skill 12. One final rebuild to Debug. Additional Rules: Do not launch parallel agents, subagents, or background processes - this includes running any Bash/PowerShell tool call with run_in_background=true, and any 'nohup'/'&'-style detached command (e.g. for the test suite or builds). Always run commands in the foreground and wait for them to finish before proceeding. Backgrounding a command hands control back to you while it keeps running unattended, which makes the herdr working status on this pane look idle prematurely and can get the pane closed mid-task with no way to resume."

# CONFIGURATION: Set how many hours to wait between iterations
$SleepDurationHours = 3

function Get-OpenIssueCount {
    $Result = gh issue list --repo $IssuesRepo --state open --json number 2>&1 | Out-String
    try {
        $Parsed = $Result | ConvertFrom-Json
        return @($Parsed).Count
    } catch {
        Write-Host "Could not parse open issue count from gh output: $Result" -ForegroundColor Red
        return -1
    }
}

# Finds an issue still tagged AI_WORKING (left over from a stalled/interrupted run, possibly
# from a previous invocation of this script) and pulls the Claude session id out of its
# claim comment so we can resume it instead of claiming fresh work.
function Get-InProgressIssue {
    $Result = gh issue list --repo $IssuesRepo --state open --label AI_WORKING --json number,comments 2>&1 | Out-String
    try {
        $Parsed = $Result | ConvertFrom-Json
    } catch {
        Write-Host "Could not parse AI_WORKING issue list from gh output: $Result" -ForegroundColor Red
        return $null
    }

    foreach ($Issue in @($Parsed)) {
        $SessionComment = $Issue.comments | Where-Object { $_.body -match 'Claude session:\s*([0-9a-fA-F-]{36})' } | Select-Object -Last 1
        if ($SessionComment) {
            return [PSCustomObject]@{
                Number    = $Issue.number
                SessionId = $Matches[1]
            }
        }
    }
    return $null
}

$CurrentPaneId = $env:HERDR_PANE_ID
if (-not $CurrentPaneId) {
    Write-Error "This script must be run from inside an active Herdr terminal session."
    exit 1
}

$Iteration = 1

# Initialize session state from the passed parameter if available
$LastSessionId = $SessionId
$ShouldResume = [bool](-not [string]::IsNullOrEmpty($SessionId))
$IsRetryAttempt = $false # Tracks if our current run is an automatic verification retry

if ($ShouldResume) {
    Write-Host "Initial session ID provided via parameter. Script will start by resuming session: $LastSessionId" -ForegroundColor White
}

while ($true) {
    Write-Host "`n=== Starting Iteration #${Iteration} ===" -ForegroundColor White
    $ForceSleep = $false

    # GitHub is the source of truth for unfinished work: if an issue is still tagged
    # AI_WORKING, a prior run (possibly a prior process, e.g. after a restart) claimed it
    # and never closed it out. Resume that session instead of claiming something new.
    $InProgress = Get-InProgressIssue
    if ($InProgress) {
        Write-Host "Issue #$($InProgress.Number) is still tagged AI_WORKING with session $($InProgress.SessionId) - resuming it instead of claiming new work." -ForegroundColor Yellow
        $LastSessionId = $InProgress.SessionId
        $ShouldResume = $true
    }

    # Check how many active panes/agents exist in Herdr before proceeding
    Write-Host "Checking active Herdr workspaces..." -ForegroundColor White
    $AgentList = herdr agent list 2>&1 | Out-String
    $ActivePaneCount = ($AgentList -split "`r?`n" | Where-Object { $_ -match '"pane_id"' -or $_ -match 'w\d+:p\d+' -or $_.Trim().Length -gt 5 }).Count

    if ($ActivePaneCount -gt 1) {
        Write-Host "Detected $ActivePaneCount active panes. An agent workspace is already open!" -ForegroundColor Red
        Write-Host "Skipping execution loop to avoid overlapping tasks." -ForegroundColor Red
        $ForceSleep = $true
    } else {
        # Establish the baseline of remaining work before running the agent
        $BeforeCount = Get-OpenIssueCount
        Write-Host "Initial open issue count: $BeforeCount." -ForegroundColor White

        $NewPaneId = $null
        try {
            Write-Host "Creating new split pane to the right..." -ForegroundColor Cyan
            $SplitOutput = herdr pane split $CurrentPaneId --direction right --no-focus | Out-String

            if ($SplitOutput -match '"pane_id"\s*:\s*"([^"]+)"') {
                $NewPaneId = $Matches[1]
                Write-Host "Successfully created new pane with ID: $NewPaneId" -ForegroundColor Green
            } else {
                throw "Could not parse the new pane ID from Herdr output."
            }

            # Check if we should go down the resume path (either due to a hard block or an automatic retry)
            if (($ShouldResume -or $IsRetryAttempt) -and $LastSessionId) {
                if ($IsRetryAttempt) {
                    Write-Host "VERIFICATION RETRY: Re-entering session $LastSessionId to prompt 'continue'..." -ForegroundColor Cyan
                } else {
                    Write-Host "Previous run was BLOCKED or manually targeted. Resuming session: $LastSessionId" -ForegroundColor Cyan
                }

                $LaunchCommand = "claude --resume ${LastSessionId} --permission-mode auto --model claude-sonnet-5 --effort high"

                Write-Host "Launching Claude Code in resume mode in pane $NewPaneId..." -ForegroundColor Cyan
                herdr pane run $NewPaneId $LaunchCommand
                Start-Sleep -Seconds 5

                # Continuing previous work
                Write-Host "Sending 'continue' verification command to Claude Code..." -ForegroundColor Cyan
                herdr pane run $NewPaneId "continue"

                # Claude's TUI can still be initializing when the text+Enter above lands, in
                # which case the Enter is swallowed and the text just sits unsubmitted in the
                # prompt box. Send a bare Enter a moment later as a safety net.
                Start-Sleep -Seconds 3
                herdr pane send-keys $NewPaneId Enter
            }
            else {
                $LaunchCommand = "claude --permission-mode auto --model claude-sonnet-5 --effort high"
                Write-Host "Launching Claude Code in pane $NewPaneId..." -ForegroundColor Cyan
                herdr pane run $NewPaneId $LaunchCommand
                Start-Sleep -Seconds 5

                # Inject the core prompt instructions
                Write-Host "Sending prompt to Claude Code..." -ForegroundColor Cyan
                herdr pane run $NewPaneId $Prompt

                # Same startup-race safety net as the resume path above.
                Start-Sleep -Seconds 3
                herdr pane send-keys $NewPaneId Enter
            }

            Start-Sleep -Seconds 5

            # Poll status until it drops out of the working state
            Write-Host "Monitoring execution progress..." -ForegroundColor Cyan
            $IsWorking = $true
            $ConsecutiveIdleChecks = 0
            $RequiredIdleChecks = 3   # debounce: don't trust "not working" until seen this many times in a row

            while ($IsWorking) {
                $StatusCheck = herdr agent get $NewPaneId 2>&1 | Out-String

                if ($StatusCheck -match '"agent_status"\s*:\s*"working"') {
                    $ConsecutiveIdleChecks = 0
                    Start-Sleep -Seconds 2
                    continue
                }

                $ConsecutiveIdleChecks++
                if ($ConsecutiveIdleChecks -lt $RequiredIdleChecks) {
                    Start-Sleep -Seconds 5
                    continue
                }

                # Belt-and-suspenders: even after debounced idle status, don't close the pane
                # out from under a still-running build/test process (e.g. a backgrounded
                # cargo/nextest run) - that's what silently killed a run before.
                $ProcInfo = herdr pane process-info $NewPaneId 2>&1 | Out-String
                if ($ProcInfo -match '"name"\s*:\s*"(cargo|nextest|zig)[^"]*"') {
                    Write-Host "Idle status seen but a build/test process is still running in the pane - waiting." -ForegroundColor Yellow
                    $ConsecutiveIdleChecks = 0
                    Start-Sleep -Seconds 5
                    continue
                }

                Write-Host "Claude has finished working." -ForegroundColor White
                $IsWorking = $false

                # Capture the target session identifier
                if ($StatusCheck -match '"value"\s*:\s*"([^"]+)"') {
                    $LastSessionId = $Matches[1]
                    Write-Host "Captured session ID: $LastSessionId" -ForegroundColor White
                }

                # Analyze the exit state condition
                if ($StatusCheck -match '"agent_status"\s*:\s*"blocked"') {
                    Write-Host "Claude hit a hard BLOCKED profile." -ForegroundColor Red
                    $ShouldResume = $true
                } else {
                    Write-Host "Claude completed without hitting a hard block profile." -ForegroundColor Green
                    $ShouldResume = $false
                }
            }

        } catch {
            Write-Host "An error occurred in iteration #${Iteration}: ${_}" -ForegroundColor Red
            $ForceSleep = $true
        } finally {
            if ($NewPaneId) {
                Write-Host "Cleaning up: Closing pane $NewPaneId..." -ForegroundColor Cyan
                herdr pane close $NewPaneId
            }
        }

        # Progress Tracker audit execution
        if (-not $ForceSleep) {
            $AfterCount = Get-OpenIssueCount

            if ($AfterCount -ge $BeforeCount) {
                # Claude stopped but the work count didn't drop
                if (-not $IsRetryAttempt) {
                    Write-Host "STALL OR PREMATURE DROPOUT DETECTED ($BeforeCount items left)." -ForegroundColor Red
                    Write-Host "Attempting automatic 1-time session verification restart..." -ForegroundColor Cyan

                    $IsRetryAttempt = $true   # Flag that the next immediate run is our verification look
                    $ForceSleep = $false      # Bypass sleep to retry right now
                } else {
                    Write-Host "STALL PERSISTS: Failed verification run on retry loop." -ForegroundColor Red
                    Write-Host "Forcing 3-hour sleep cooldown sequence to prevent token thrashing." -ForegroundColor Red
                    $IsRetryAttempt = $false  # Reset retry flag for future iterations
                    $ForceSleep = $true
                }
            } else {
                Write-Host "Progress verified! Remaining items decreased from $BeforeCount to $AfterCount." -ForegroundColor Green
                $IsRetryAttempt = $false # Clear out retry history on success

                if ($AfterCount -gt 0) {
                    Write-Host "More work remains. Bypassing the $SleepDurationHours hour sleep to run the next task immediately!" -ForegroundColor Cyan
                }
            }
        }
    }

    Write-Host "Iteration #${Iteration} complete." -ForegroundColor White

    # Check if there's any remaining work left to justify skipping the cooldown
    $FinalWorkCheck = Get-OpenIssueCount

    # Fallback to the countdown timer if explicitly forced, if queue is empty, or if we are blocked and not retrying
    if ($ForceSleep -or ($FinalWorkCheck -le 0) -or ($FinalWorkCheck -gt 0 -and $ShouldResume -and -not $IsRetryAttempt)) {
        if ($FinalWorkCheck -eq 0) {
            Write-Host "All open issues in $IssuesRepo are resolved!" -ForegroundColor Green
        }

        # If we didn't just flag a verification retry and we are forced to sleep, run cooldown timer
        if (-not $IsRetryAttempt) {
            for ($HoursLeft = $SleepDurationHours; $HoursLeft -gt 0; $HoursLeft--) {
                $Unit = if ($HoursLeft -eq 1) { "hour" } else { "hours" }
                Write-Host "Waiting $HoursLeft $Unit..." -ForegroundColor Red
                Start-Sleep -Seconds 3600
            }
        }
    }

    $Iteration++
}
