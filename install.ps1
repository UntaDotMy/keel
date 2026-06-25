# Purpose: Bootstrap keel from the latest GitHub release without a manual archive download.
# Caller: Windows PowerShell users running the documented one-line installer.
# Dependencies: PowerShell, Invoke-WebRequest, Expand-Archive, and keel GitHub release assets.
# Main Functions: Detect platform, download a release archive to temp, extract it, run install, and verify status.
# Side Effects: Writes the managed keel surface under $env:USERPROFILE\.claude and removes temporary download files.

[CmdletBinding()]
param(
    [string]$Version = $env:CLAUDE_SKILLS_VERSION,
    [string]$Repository = $env:CLAUDE_SKILLS_REPOSITORY,
    [switch]$Semantic
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Repository)) {
    $Repository = "UntaDotMy/keel"
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
        "User-Agent" = "keel-installer"
    }
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$RepositorySlug/releases/latest" -Headers $headers
    return $release.tag_name
}

if ($Version -eq "latest") {
    $ReleaseTag = Get-LatestReleaseTag -RepositorySlug $Repository
    if ([string]::IsNullOrWhiteSpace($ReleaseTag)) {
        throw "Unable to resolve latest keel release for $Repository"
    }
} else {
    $ReleaseTag = Normalize-ReleaseTag -RawVersion $Version
}

$AssetVersion = Get-AssetVersion -ReleaseTag $ReleaseTag
$Architecture = Get-NormalizedArchitecture
$ArchiveSuffix = if ($Semantic) { "_semantic" } else { "" }
$ArchiveName = "keel_${AssetVersion}_windows_${Architecture}${ArchiveSuffix}.zip"
$DownloadUrl = "https://github.com/$Repository/releases/download/$ReleaseTag/$ArchiveName"
$TemporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ("keel-install-" + [System.Guid]::NewGuid().ToString("N"))

try {
    New-Item -ItemType Directory -Path $TemporaryDirectory | Out-Null
    $ArchivePath = Join-Path $TemporaryDirectory $ArchiveName
    $ExtractDirectory = Join-Path $TemporaryDirectory "extract"
    New-Item -ItemType Directory -Path $ExtractDirectory | Out-Null

    Write-Host "Downloading keel $ReleaseTag for windows-$Architecture..."
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $ArchivePath -Headers @{ "User-Agent" = "keel-installer" }

    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractDirectory -Force

    $InstallerBinary = Get-ChildItem -Path $ExtractDirectory -Filter "keel.exe" -File -Recurse | Select-Object -First 1
    if ($null -eq $InstallerBinary) {
        throw "Release archive did not contain keel.exe."
    }

    $BundleRoot = $InstallerBinary.Directory.FullName
    & $InstallerBinary.FullName install --repo-root $BundleRoot
    if ($LASTEXITCODE -ne 0) {
        throw "keel install failed with exit code $LASTEXITCODE"
    }

    $InstalledBinary = Join-Path $env:USERPROFILE ".claude\keel.exe"
    if (-not (Test-Path $InstalledBinary -PathType Leaf)) {
        throw "Installed binary not found at $InstalledBinary"
    }

    & $InstalledBinary status --repo-root $BundleRoot
    if ($LASTEXITCODE -ne 0) {
        throw "keel status failed with exit code $LASTEXITCODE"
    }

    # Native `keel install` (above) already wires the lifecycle hooks
    # into settings.json, but does so best-effort (a failure is reported, not
    # fatal). This explicit re-run is the bootstrap's verification gate: it is
    # idempotent, and it turns a hook-wiring failure into a hard install error so
    # a broken engagement surface never ships silently.
    & $InstalledBinary hook install
    if ($LASTEXITCODE -ne 0) {
        throw "keel hook install failed with exit code $LASTEXITCODE"
    }

    # MCP registration is handled natively by `keel install` above, which
    # writes the keel entry into ~/.claude.json *with* `alwaysLoad: true`
    # (see rust/.../manager/mcp_register.rs). That flag pins the recall,
    # system_map, run_command, and recall_status tools into context instead of
    # leaving them deferred behind ToolSearch. We deliberately do NOT shell out to
    # `claude mcp add` here: that command cannot set `alwaysLoad`, so it would
    # register the server in a degraded (deferred) state, and it requires the
    # `claude` CLI on PATH. The native path needs neither and is the single source
    # of truth. Run `keel doctor` to confirm the entry and `alwaysLoad`,
    # or `keel repair` to re-register if anything looks off.

    Write-Host "keel installed successfully at $InstalledBinary"
} finally {
    if (Test-Path $TemporaryDirectory) {
        Remove-Item -Path $TemporaryDirectory -Recurse -Force
    }
}
