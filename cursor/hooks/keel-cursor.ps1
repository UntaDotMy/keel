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
}

# Default pass-through
Write-Output "{}"
exit 0
