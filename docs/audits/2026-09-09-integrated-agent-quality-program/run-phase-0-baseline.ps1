param(
    [string]$OutputRoot = (Join-Path $PSScriptRoot "raw\phase-0")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$resolvedOutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)
[System.IO.Directory]::CreateDirectory($resolvedOutputRoot) | Out-Null

$commands = @(
    [ordered]@{ id = "git-head"; group = "environment"; file = "git"; arguments = @("rev-parse", "HEAD") },
    [ordered]@{ id = "git-status"; group = "environment"; file = "git"; arguments = @("status", "--short", "--branch") },
    [ordered]@{ id = "git-version"; group = "environment"; file = "git"; arguments = @("--version") },
    [ordered]@{ id = "rustc-version"; group = "environment"; file = "rustc"; arguments = @("-Vv") },
    [ordered]@{ id = "cargo-version"; group = "environment"; file = "cargo"; arguments = @("-V") },
    [ordered]@{ id = "bun-version"; group = "environment"; file = "bun"; arguments = @("--version") },
    [ordered]@{ id = "keel-version"; group = "environment"; file = "keel"; arguments = @("version", "--json") },
    [ordered]@{ id = "cargo-fmt"; group = "required"; file = "cargo"; arguments = @("fmt", "--all", "--", "--check") },
    [ordered]@{ id = "cargo-clippy"; group = "required"; file = "cargo"; arguments = @("clippy", "--all-targets", "--", "-D", "warnings") },
    [ordered]@{ id = "cargo-build"; group = "required"; file = "cargo"; arguments = @("build", "--workspace") },
    [ordered]@{ id = "cargo-test"; group = "required"; file = "cargo"; arguments = @("test", "--workspace", "--locked", "--no-fail-fast") },
    [ordered]@{ id = "skill-lint"; group = "required"; file = "cargo"; arguments = @("run", "--locked", "-p", "keel", "--", "skill-lint") },
    [ordered]@{ id = "skill-eval"; group = "required"; file = "cargo"; arguments = @("run", "--locked", "-p", "keel", "--", "skill-eval") },
    [ordered]@{ id = "reducer-eval"; group = "required"; file = "cargo"; arguments = @("run", "--locked", "-p", "keel", "--", "eval") },
    [ordered]@{ id = "gain"; group = "required"; file = "cargo"; arguments = @("run", "--locked", "-p", "keel", "--", "gain") },
    [ordered]@{ id = "config-audit"; group = "required"; file = "cargo"; arguments = @("run", "--locked", "-p", "keel", "--", "config-audit") },
    [ordered]@{ id = "review-pre-commit"; group = "required"; file = "keel"; arguments = @("review", "pre-commit") },
    [ordered]@{ id = "review-pre-pr"; group = "required"; file = "keel"; arguments = @("review", "pre-pr") },
    [ordered]@{ id = "review-gates"; group = "required"; file = "keel"; arguments = @("review", "gates", "check") },
    [ordered]@{ id = "completion-gate"; group = "required"; file = "keel"; arguments = @("memory", "completion-gate", "check") },
    [ordered]@{ id = "host-contracts"; group = "required"; file = "bun"; arguments = @("test", "tests/host-adapter-contracts.test.ts") },
    [ordered]@{ id = "stats-json"; group = "measurement"; file = "keel"; arguments = @("stats", "--json") },
    [ordered]@{ id = "session-json"; group = "measurement"; file = "keel"; arguments = @("session", "--json") },
    [ordered]@{ id = "doctor"; group = "measurement"; file = "keel"; arguments = @("doctor") }
)

function Invoke-CapturedCommand {
    param(
        [Parameter(Mandatory)] [System.Collections.IDictionary]$CommandSpec
    )

    $processInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $processInfo.FileName = [string]$CommandSpec.file
    $processInfo.WorkingDirectory = $repositoryRoot
    $processInfo.UseShellExecute = $false
    $processInfo.CreateNoWindow = $true
    $processInfo.RedirectStandardOutput = $true
    $processInfo.RedirectStandardError = $true
    foreach ($argument in $CommandSpec.arguments) {
        [void]$processInfo.ArgumentList.Add([string]$argument)
    }

    $startedAt = [DateTimeOffset]::UtcNow
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $processInfo
    [void]$process.Start()
    $standardOutputTask = $process.StandardOutput.ReadToEndAsync()
    $standardErrorTask = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $standardOutput = $standardOutputTask.GetAwaiter().GetResult()
    $standardError = $standardErrorTask.GetAwaiter().GetResult()
    $timer.Stop()

    $standardOutputPath = Join-Path $resolvedOutputRoot "$($CommandSpec.id).stdout.log"
    $standardErrorPath = Join-Path $resolvedOutputRoot "$($CommandSpec.id).stderr.log"
    $utf8 = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($standardOutputPath, $standardOutput, $utf8)
    [System.IO.File]::WriteAllText($standardErrorPath, $standardError, $utf8)

    [ordered]@{
        id = $CommandSpec.id
        group = $CommandSpec.group
        command = @($CommandSpec.file) + @($CommandSpec.arguments)
        started_at = $startedAt.ToString("o")
        duration_ms = $timer.ElapsedMilliseconds
        exit_code = $process.ExitCode
        stdout_path = [System.IO.Path]::GetRelativePath($repositoryRoot, $standardOutputPath).Replace("\", "/")
        stdout_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $standardOutputPath).Hash.ToLowerInvariant()
        stderr_path = [System.IO.Path]::GetRelativePath($repositoryRoot, $standardErrorPath).Replace("\", "/")
        stderr_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $standardErrorPath).Hash.ToLowerInvariant()
    }
}

$results = foreach ($commandSpec in $commands) {
    Write-Host "capture $($commandSpec.id)"
    Invoke-CapturedCommand -CommandSpec $commandSpec
}

$warningCandidates = foreach ($result in $results) {
    foreach ($streamName in @("stdout", "stderr")) {
        $relativePath = $result["${streamName}_path"]
        $absolutePath = Join-Path $repositoryRoot $relativePath
        $lineNumber = 0
        foreach ($line in [System.IO.File]::ReadLines($absolutePath)) {
            $lineNumber += 1
            if ($line -match "(?i)(^|[^a-z])(warning|deprecated|deprecation)(:|[^a-z])") {
                [ordered]@{
                    command_id = $result.id
                    stream = $streamName
                    line = $lineNumber
                    text = $line.Trim()
                }
            }
        }
    }
}

$manifest = [ordered]@{
    schema_version = 1
    captured_at = [DateTimeOffset]::UtcNow.ToString("o")
    repository_root = $repositoryRoot
    operating_system = [System.Runtime.InteropServices.RuntimeInformation]::OSDescription
    process_architecture = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString()
    powershell_version = $PSVersionTable.PSVersion.ToString()
    commands = @($results)
}

$manifestPath = Join-Path $resolvedOutputRoot "manifest.json"
$warningsPath = Join-Path $resolvedOutputRoot "warning-candidates.json"
$utf8 = [System.Text.UTF8Encoding]::new($false)
[System.IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 8), $utf8)
[System.IO.File]::WriteAllText($warningsPath, (@($warningCandidates) | ConvertTo-Json -Depth 6), $utf8)

$failedRequired = @($results | Where-Object { $_.group -eq "required" -and $_.exit_code -ne 0 })
Write-Host "manifest: $manifestPath"
Write-Host "required failures: $($failedRequired.Count)"
if ($failedRequired.Count -gt 0) {
    exit 1
}
