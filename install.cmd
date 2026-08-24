@echo off
REM Purpose: Bootstrap keel on Windows CMD by delegating to the PowerShell installer.
REM Caller: Windows CMD users running the documented one-line installer.
REM Dependencies: curl or PowerShell download support, PowerShell script execution, and GitHub release assets.
REM Main Functions: Download install.ps1 to temp, run it, and delete the temporary script.
REM Side Effects: Writes keel's host-neutral home under %USERPROFILE%\.keel and the Claude engagement files under %USERPROFILE%\.claude through install.ps1.

setlocal EnableExtensions
set "REPOSITORY=UntaDotMy/keel"
set "DOWNLOAD_BASE=%CLAUDE_SKILLS_INSTALL_BASE%"
set "VERSION=%CLAUDE_SKILLS_VERSION%"
if "%VERSION%"=="" set "VERSION=latest"
if defined DOWNLOAD_BASE goto download_base_ready
if /I "%VERSION%"=="latest" goto download_latest
set "TAG=%VERSION%"
if /I "%TAG:~0,1%"=="v" goto download_tag_ready
if /I "%TAG:~0,10%"=="bootstrap-" goto download_tag_ready
set "TAG=v%TAG%"

:download_tag_ready
set "DOWNLOAD_BASE=https://github.com/%REPOSITORY%/releases/download/%TAG%"
goto download_base_ready

:download_latest
set "DOWNLOAD_BASE=https://github.com/%REPOSITORY%/releases/latest/download"

:download_base_ready

set "TEMP_BASE=%TEMP%\keel-install-%RANDOM%-%RANDOM%"
set "TEMP_SCRIPT=%TEMP_BASE%\install.ps1"
set "TEMP_CHECKSUM=%TEMP_BASE%\install.ps1.sha256"
set "EXPECTED="
set "ACTUAL="
mkdir "%TEMP_BASE%" >nul 2>nul
if errorlevel 1 goto :failed

where curl >nul 2>nul
if errorlevel 1 (
  powershell -NoProfile -Command "Invoke-WebRequest -Uri '%DOWNLOAD_BASE%/install.ps1' -OutFile '%TEMP_SCRIPT%'"
  if errorlevel 1 goto :failed
  powershell -NoProfile -Command "Invoke-WebRequest -Uri '%DOWNLOAD_BASE%/install.ps1.sha256' -OutFile '%TEMP_CHECKSUM%'"
  if errorlevel 1 goto :failed
) else (
  curl -fsSL "%DOWNLOAD_BASE%/install.ps1" -o "%TEMP_SCRIPT%"
  if errorlevel 1 goto :failed
  curl -fsSL "%DOWNLOAD_BASE%/install.ps1.sha256" -o "%TEMP_CHECKSUM%"
  if errorlevel 1 goto :failed
)

for /f "usebackq tokens=1" %%H in ("%TEMP_CHECKSUM%") do if not defined EXPECTED set "EXPECTED=%%H"
for /f "skip=1 tokens=1" %%H in ('certutil -hashfile "%TEMP_SCRIPT%" SHA256') do if not defined ACTUAL set "ACTUAL=%%H"
if "%EXPECTED%"=="" goto :failed
if "%ACTUAL%"=="" goto :failed
if /I not "%EXPECTED%"=="%ACTUAL%" goto :failed

powershell -NoProfile -ExecutionPolicy Bypass -File "%TEMP_SCRIPT%" %*
set "INSTALL_EXIT=%ERRORLEVEL%"
goto :cleanup

:failed
set "INSTALL_EXIT=1"

:cleanup
del "%TEMP_SCRIPT%" >nul 2>nul
del "%TEMP_CHECKSUM%" >nul 2>nul
rmdir "%TEMP_BASE%" >nul 2>nul
exit /b %INSTALL_EXIT%
