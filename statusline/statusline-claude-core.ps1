<#
.SYNOPSIS
  claude-core statusline (Windows PowerShell). Renders model + context usage and
  appends a compaction-savings badge sourced from `claude-skills gain --json`.
.DESCRIPTION
  Caller: Claude Code statusLine command on Windows, e.g. settings.json:
    { "type": "command",
      "command": "powershell -NoProfile -ExecutionPolicy Bypass -File %USERPROFILE%\\.claude\\statusline-claude-core.ps1" }
  Input : Claude Code pipes session JSON on stdin (model, context_window, ...).
  Output: one status line on stdout. Degrades gracefully — if the binary or gain
          data is unavailable the savings badge is omitted, and the line never
          errors (a non-zero exit or empty output blanks the statusline).
#>

$ErrorActionPreference = 'SilentlyContinue'

# Read all of stdin.
$raw = [Console]::In.ReadToEnd()

$model = ''
$usedPct = ''
if ($raw) {
    try {
        $data = $raw | ConvertFrom-Json
        if ($data.model -and $data.model.display_name) { $model = [string]$data.model.display_name }
        if ($data.context_window -and ($null -ne $data.context_window.used_percentage)) {
            $usedPct = [string]$data.context_window.used_percentage
        }
    } catch {
        # Malformed JSON: fall through to the safe default line.
    }
}

# Locate the installed claude-skills binary (best-effort, never required).
function Resolve-Binary {
    $claudeHome = $env:USERPROFILE
    if (-not $claudeHome) { $claudeHome = $env:HOME }
    if ($claudeHome) {
        $candidate = Join-Path $claudeHome '.claude\claude-skills.exe'
        if (Test-Path $candidate) { return $candidate }
    }
    $onPath = Get-Command 'claude-skills' -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    return $null
}

# Savings badge from `gain --json` (optional). Pinned to the real schema:
# top-level integer field 'tokensSaved'.
$savingsBadge = ''
$bin = Resolve-Binary
if ($bin) {
    try {
        $gainRaw = & $bin gain --since today --json 2>$null
        if ($gainRaw) {
            $gain = ($gainRaw -join "`n") | ConvertFrom-Json
            $saved = [int]$gain.tokensSaved
            if ($saved -gt 0) { $savingsBadge = "saved $saved tok" }
        }
    } catch {
        # gain unavailable or unparseable: omit the badge.
    }
}

# Compose the line; omit empty segments cleanly.
$segments = @()
if ($model)        { $segments += $model }
if ($usedPct -ne '') { $segments += "ctx $usedPct%" }
if ($savingsBadge) { $segments += $savingsBadge }
if ($segments.Count -eq 0) { $line = 'claude-core' } else { $line = ($segments -join ' | ') }

Write-Output $line
