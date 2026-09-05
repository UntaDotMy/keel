# Purpose: Cursor lifecycle hooks adapter for Windows PowerShell.
# Caller: Cursor editor hook events on Windows when configured to use PowerShell.
# Dependencies: keel.exe, ConvertFrom-Json, ConvertTo-Json.
# Main Functions: PreToolUse Iron Law check, command rewriting, and lifecycle dispatch.
# Side Effects: Emits JSON hook response on stdout.

[CmdletBinding()]
param()

$ErrorActionPreference = "SilentlyContinue"

# Resolve keel.exe binary path
$KeelBin = $env:KEEL_HOME
if (-not [string]::IsNullOrWhiteSpace($KeelBin) -and (Test-Path (Join-Path $KeelBin "keel.exe"))) {
    $KeelBin = Join-Path $KeelBin "keel.exe"
} else {
    $DefaultKeel = Join-Path $env:USERPROFILE ".keel\keel.exe"
    if (Test-Path $DefaultKeel) {
        $KeelBin = $DefaultKeel
    } else {
        $KeelBin = "keel.exe"
    }
}

# Read stdin JSON payload
$InputJson = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($InputJson)) {
    Write-Output "{}"
    exit 0
}

try {
    $Payload = ConvertFrom-Json -InputObject $InputJson
} catch {
    Write-Output "{}"
    exit 0
}

$HookEvent = $Payload.hook_event_name
$ToolName = $Payload.tool_name
$Command = if ($Payload.tool_input -and $Payload.tool_input.command) { $Payload.tool_input.command } else { "" }
$ToolPath = ""
if ($Payload.tool_input) {
    if ($Payload.tool_input.path) { $ToolPath = [string]$Payload.tool_input.path }
    elseif ($Payload.tool_input.file_path) { $ToolPath = [string]$Payload.tool_input.file_path }
    elseif ($Payload.tool_input.filePath) { $ToolPath = [string]$Payload.tool_input.filePath }
}
$Cwd = if ($Payload.cwd) { $Payload.cwd } else { $PWD.Path }
$SessionId = if ($Payload.conversation_id) { $Payload.conversation_id } else { "default" }

# Handle lifecycle events
switch ($HookEvent) {
    "preCompact" {
        & $KeelBin bridge pre-compact --session $SessionId --cwd $Cwd 2>$null | Out-Null
        Write-Output "{}"
        exit 0
    }
    "stop" {
        Write-Output "{}"
        exit 0
    }
    "sessionEnd" {
        & $KeelBin bridge session-end --session $SessionId --cwd $Cwd 2>$null | Out-Null
        Write-Output "{}"
        exit 0
    }
    "postToolUse" {
        Write-Output "{}"
        exit 0
    }
    "preToolUse" {
        $GateArgs = @("bridge", "pre-tool-use", "--session", $SessionId, "--cwd", $Cwd, "--tool", $ToolName)
        if (-not [string]::IsNullOrWhiteSpace($Command)) {
            $GateArgs += @("--command", $Command)
        }
        if (-not [string]::IsNullOrWhiteSpace($ToolPath)) {
            $GateArgs += @("--path", $ToolPath)
        }
        $Gate = & $KeelBin @GateArgs 2>$null
        if ($Gate -like "KEEL_GATE_DENY*") {
            $Reason = (($Gate -split "`n") | Select-Object -Skip 1) -join "`n"
            if ([string]::IsNullOrWhiteSpace($Reason)) {
                $Reason = "keel Iron Law gate: call system_map/recall/context_brief before editing."
            }
            @{
                permission = "deny"
                user_message = $Reason
                agent_message = $Reason
            } | ConvertTo-Json -Compress
            exit 0
        }
        Write-Output "{}"
        exit 0
    }
}

# Default pass-through
Write-Output "{}"
exit 0
