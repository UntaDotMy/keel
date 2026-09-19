# Purpose: keel Cursor lifecycle hooks adapter for Windows PowerShell.
# keel:managed-host-file (remove this line before customizing to opt out of upgrades)
# Caller: Cursor hook events on Windows; mirrors cursor/hooks/keel-cursor.sh.
# Dependencies: keel.exe, ConvertFrom-Json, ConvertTo-Json.
# Main Functions: Iron Law gate, command reroute, observation, lifecycle dispatch.
# Side Effects: Emits a JSON hook response on stdout.

[CmdletBinding()]
param()

$ErrorActionPreference = "SilentlyContinue"

function Resolve-KeelBinary {
    if (-not [string]::IsNullOrWhiteSpace($env:KEEL_HOME)) {
        $candidate = Join-Path $env:KEEL_HOME "keel.exe"
        if (Test-Path $candidate) { return $candidate }
    }
    $default = Join-Path $env:USERPROFILE ".keel\keel.exe"
    if (Test-Path $default) { return $default }
    return "keel.exe"
}

function Write-Deny([string]$Reason) {
    if ([string]::IsNullOrWhiteSpace($Reason)) {
        $Reason = "keel Iron Law gate denied this action."
    }
    @{
        permission    = "deny"
        user_message  = $Reason
        agent_message = $Reason
    } | ConvertTo-Json -Compress
    exit 0
}

function Get-ReasonFromGate([string]$Gate, [string]$Fallback) {
    $lines = @($Gate -split "`n" | Select-Object -Skip 1)
    $reason = ($lines -join "`n").Trim()
    if ([string]::IsNullOrWhiteSpace($reason)) { return $Fallback }
    return $reason
}

# Cursor reports capitalized tool names; the Rust gate matches canonical ones.
function Get-CanonicalTool([string]$Name) {
    switch ($Name) {
        "StrReplace" { return "str_replace" }
        "SearchReplace" { return "search_replace" }
        "ApplyPatch" { return "apply_patch" }
        default { return $Name }
    }
}

function Test-EditClassTool([string]$Name) {
    return @(
        "Write", "Edit", "Delete", "StrReplace", "MultiEdit",
        "NotebookEdit", "ApplyPatch", "Patch", "SearchReplace"
    ) -contains $Name
}

function Test-ShellTool([string]$Name) {
    return @("Shell", "Bash", "PowerShell", "Command", "Terminal") -contains $Name
}

function Test-CompoundCommand([string]$Command) {
    if ([string]::IsNullOrWhiteSpace($Command)) { return $false }
    # Mirrors the POSIX adapter's local fail-closed guard.
    return $Command -match '&&|\|\||;|\||`|\$\(|\r|\n'
}

$KeelBin = Resolve-KeelBinary

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
$ToolName = [string]$Payload.tool_name
$Command = if ($Payload.tool_input -and $Payload.tool_input.command) { [string]$Payload.tool_input.command } else { "" }
$ToolPath = ""
if ($Payload.tool_input) {
    if ($Payload.tool_input.path) { $ToolPath = [string]$Payload.tool_input.path }
    elseif ($Payload.tool_input.file_path) { $ToolPath = [string]$Payload.tool_input.file_path }
    elseif ($Payload.tool_input.filePath) { $ToolPath = [string]$Payload.tool_input.filePath }
}
$Cwd = if ($Payload.cwd) { [string]$Payload.cwd } else { $PWD.Path }
$SessionId = if ($Payload.conversation_id) { [string]$Payload.conversation_id } else { "default" }

# Iron Law and session-start markers, shared with Rust (`iron-law-satisfied`).
$SessionKey = ($SessionId.ToLower() -replace '[^a-z0-9]+', '-').Trim('-')
if ([string]::IsNullOrWhiteSpace($SessionKey)) { $SessionKey = "workspace" }
$KeelStateRoot = if (-not [string]::IsNullOrWhiteSpace($env:KEEL_HOME)) {
    Join-Path $env:KEEL_HOME "state"
} elseif (Test-Path (Join-Path $env:USERPROFILE ".keel")) {
    Join-Path $env:USERPROFILE ".keel\state"
} else {
    Join-Path $env:USERPROFILE ".claude\state"
}
$MarkerPath = Join-Path $KeelStateRoot "iron-law-satisfied\$SessionKey"
$StartedMarkerPath = Join-Path $KeelStateRoot "cursor-session-started\$SessionKey"

$IronLawReason = "IRON LAW ENFORCED (STRICT): Use a keel tool first (MCP system_map, recall, context_brief, skill_route, or keel doctor / code-search). Plain Read does not clear the gate."

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
        Remove-Item -Force -ErrorAction SilentlyContinue $MarkerPath, $StartedMarkerPath
        Write-Output "{}"
        exit 0
    }
    "postToolUse" {
        # Failures are recorded too: Cursor reports tool_output.exit_code.
        $ObserveArgs = @(
            "bridge", "observe", "--session", $SessionId, "--cwd", $Cwd,
            "--tool", $ToolName, "--phase", "post"
        )
        $ExitCode = $null
        if ($Payload.tool_output -and $null -ne $Payload.tool_output.exit_code) {
            $ExitCode = [string]$Payload.tool_output.exit_code
        }
        if (-not [string]::IsNullOrWhiteSpace($ExitCode) -and $ExitCode -ne "0") {
            $ObserveArgs += "--failed"
        }
        $InputJson | & $KeelBin @ObserveArgs 2>$null | Out-Null
        Write-Output "{}"
        exit 0
    }
    "preToolUse" {
        # Session-start once per conversation: bootstrap, digest, MCP self-heal.
        if (-not (Test-Path $StartedMarkerPath)) {
            & $KeelBin bridge session-start --session $SessionId --cwd $Cwd 2>$null | Out-Null
            $startedDir = Split-Path -Parent $StartedMarkerPath
            New-Item -ItemType Directory -Force -Path $startedDir | Out-Null
            New-Item -ItemType File -Force -Path $StartedMarkerPath | Out-Null
        }

        if ((Test-ShellTool $ToolName) -and (Test-CompoundCommand $Command)) {
            Write-Deny "Compound shell commands are denied by the keel gate."
        }

        if (Test-ShellTool $ToolName) {
            $Canonical = Get-CanonicalTool $ToolName
            $GateArgs = @("bridge", "pre-tool-use", "--session", $SessionId, "--cwd", $Cwd, "--tool", $Canonical)
            if (-not [string]::IsNullOrWhiteSpace($Command)) {
                $GateArgs += @("--command", $Command)
            }
            $Gate = [string](& $KeelBin @GateArgs 2>$null)
            if ($Gate -like "KEEL_GATE_DENY*") {
                Write-Deny (Get-ReasonFromGate $Gate "IRON LAW shell gate denied this command.")
            }
            if ($Gate -notlike "KEEL_GATE_ALLOW*") {
                Write-Deny "IRON LAW shell gate could not be evaluated."
            }
        }

        if (Test-EditClassTool $ToolName) {
            $Canonical = Get-CanonicalTool $ToolName
            $GateArgs = @("bridge", "pre-tool-use", "--session", $SessionId, "--cwd", $Cwd, "--tool", $Canonical)
            if (-not [string]::IsNullOrWhiteSpace($ToolPath)) {
                $GateArgs += @("--path", $ToolPath)
            }
            $Gate = [string](& $KeelBin @GateArgs 2>$null)
            if ($Gate -like "KEEL_GATE_DENY*") {
                Write-Deny (Get-ReasonFromGate $Gate $IronLawReason)
            }
            # Local fallback mirrors the POSIX adapter: a tool name the Rust core
            # does not recognize must not slip through before research.
            if (-not (Test-Path $MarkerPath)) {
                Write-Deny $IronLawReason
            }
        }

        # Compaction reroute for every shell tool with a known shell.
        if ((Test-ShellTool $ToolName) -and (-not [string]::IsNullOrWhiteSpace($Command))) {
            if ($Command -notmatch '^\s*keel\s+run\s+--') {
                $Rewrite = [string]($Command | & $KeelBin bridge rewrite --tool $ToolName 2>$null)
                if ($Rewrite -like "KEEL_REWRITE *") {
                    $Rewritten = ($Rewrite.Substring("KEEL_REWRITE ".Length)).Trim()
                    if (-not [string]::IsNullOrWhiteSpace($Rewritten) -and $Rewritten -ne $Command) {
                        @{
                            permission    = "allow"
                            updated_input = @{ command = $Rewritten }
                        } | ConvertTo-Json -Compress -Depth 4
                        exit 0
                    }
                }
            }
        }

        Write-Output "{}"
        exit 0
    }
}

Write-Output "{}"
exit 0
