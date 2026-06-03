# Purpose: Bootstrap claude-skills from the latest GitHub release without a manual archive download.
# Caller: Windows PowerShell users running the documented one-line installer.
# Dependencies: PowerShell, Invoke-WebRequest, Expand-Archive, and claude-skills GitHub release assets.
# Main Functions: Detect platform, download a release archive to temp, extract it, run install, and verify status.
# Side Effects: Writes the managed claude-skills surface under $env:USERPROFILE\.claude and removes temporary download files.

[CmdletBinding()]
param(
    [string]$Version = $env:CLAUDE_SKILLS_VERSION,
    [string]$Repository = $env:CLAUDE_SKILLS_REPOSITORY
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Repository)) {
    $Repository = "UntaDotMy/claude_core"
}
if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = "latest"
}

function Normalize-ReleaseTag {
    param([string]$RawVersion)
    if ($RawVersion -match "^(v|bootstrap-)") {
        return $RawVersion
    }
    if ($RawVersion -match "^[0-9]") {
        return "v$RawVersion"
    }
    return $RawVersion
}

function Get-AssetVersion {
    param([string]$ReleaseTag)
    if ($ReleaseTag -match "^v[0-9]") {
        return $ReleaseTag.Substring(1)
    }
    return $ReleaseTag
}

function Get-NormalizedArchitecture {
    $architecture = $env:PROCESSOR_ARCHITECTURE
    if ([string]::IsNullOrWhiteSpace($architecture)) {
        $architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    }
    switch -Regex ($architecture.ToLowerInvariant()) {
        "^(amd64|x64|x86_64)$" { return "amd64" }
        "^(arm64|aarch64)$" { return "arm64" }
        default { throw "Unsupported architecture: $architecture" }
    }
}

function Get-LatestReleaseTag {
    param([string]$RepositorySlug)
    $headers = @{
        Accept = "application/vnd.github+json"
        "User-Agent" = "claude-skills-installer"
    }
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$RepositorySlug/releases/latest" -Headers $headers
    return $release.tag_name
}

if ($Version -eq "latest") {
    $ReleaseTag = Get-LatestReleaseTag -RepositorySlug $Repository
    if ([string]::IsNullOrWhiteSpace($ReleaseTag)) {
        throw "Unable to resolve latest claude-skills release for $Repository"
    }
} else {
    $ReleaseTag = Normalize-ReleaseTag -RawVersion $Version
}

$AssetVersion = Get-AssetVersion -ReleaseTag $ReleaseTag
$Architecture = Get-NormalizedArchitecture
$ArchiveName = "claude-skills_${AssetVersion}_windows_${Architecture}.zip"
$DownloadUrl = "https://github.com/$Repository/releases/download/$ReleaseTag/$ArchiveName"
$TemporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ("claude-skills-install-" + [System.Guid]::NewGuid().ToString("N"))

try {
    New-Item -ItemType Directory -Path $TemporaryDirectory | Out-Null
    $ArchivePath = Join-Path $TemporaryDirectory $ArchiveName
    $ExtractDirectory = Join-Path $TemporaryDirectory "extract"
    New-Item -ItemType Directory -Path $ExtractDirectory | Out-Null

    Write-Host "Downloading claude-skills $ReleaseTag for windows-$Architecture..."
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $ArchivePath -Headers @{ "User-Agent" = "claude-skills-installer" }

    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractDirectory -Force

    $InstallerBinary = Get-ChildItem -Path $ExtractDirectory -Filter "claude-skills.exe" -File -Recurse | Select-Object -First 1
    if ($null -eq $InstallerBinary) {
        throw "Release archive did not contain claude-skills.exe."
    }

    $BundleRoot = $InstallerBinary.Directory.FullName
    & $InstallerBinary.FullName install --repo-root $BundleRoot
    if ($LASTEXITCODE -ne 0) {
        throw "claude-skills install failed with exit code $LASTEXITCODE"
    }

    $InstalledBinary = Join-Path $env:USERPROFILE ".claude\claude-skills.exe"
    if (-not (Test-Path $InstalledBinary -PathType Leaf)) {
        throw "Installed binary not found at $InstalledBinary"
    }

    & $InstalledBinary status --repo-root $BundleRoot
    if ($LASTEXITCODE -ne 0) {
        throw "claude-skills status failed with exit code $LASTEXITCODE"
    }

    & $InstalledBinary hook install
    if ($LASTEXITCODE -ne 0) {
        throw "claude-skills hook install failed with exit code $LASTEXITCODE"
    }

    # MCP registration is handled natively by `claude-skills install` above, which
    # writes the claude_core entry into ~/.claude.json *with* `alwaysLoad: true`
    # (see rust/.../manager/mcp_register.rs). That flag pins the recall,
    # system_map, run_command, and recall_status tools into context instead of
    # leaving them deferred behind ToolSearch. We deliberately do NOT shell out to
    # `claude mcp add` here: that command cannot set `alwaysLoad`, so it would
    # register the server in a degraded (deferred) state, and it requires the
    # `claude` CLI on PATH. The native path needs neither and is the single source
    # of truth. Run `claude-skills doctor` to confirm the entry and `alwaysLoad`,
    # or `claude-skills repair` to re-register if anything looks off.

    Write-Host "claude-skills installed successfully at $InstalledBinary"
} finally {
    if (Test-Path $TemporaryDirectory) {
        Remove-Item -Path $TemporaryDirectory -Recurse -Force
    }
}
