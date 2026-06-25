@echo off
REM Purpose: Bootstrap keel on Windows CMD by delegating to the PowerShell installer.
REM Caller: Windows CMD users running the documented one-line installer.
REM Dependencies: curl or PowerShell download support, PowerShell script execution, and GitHub release assets.
REM Main Functions: Download install.ps1 to temp, run it, and delete the temporary script.
REM Side Effects: Writes the managed keel surface under %USERPROFILE%\.claude through install.ps1.

setlocal
set "INSTALL_BASE=%CLAUDE_SKILLS_INSTALL_BASE%"
if "%INSTALL_BASE%"=="" set "INSTALL_BASE=https://raw.githubusercontent.com/UntaDotMy/keel/main"
set "TEMP_SCRIPT=%TEMP%\keel-install-%RANDOM%-%RANDOM%.ps1"

where curl >nul 2>nul
if %ERRORLEVEL%==0 (
  curl -fsSL "%INSTALL_BASE%/install.ps1" -o "%TEMP_SCRIPT%"
) else (
  powershell -NoProfile -ExecutionPolicy Bypass -Command "Invoke-WebRequest -Uri '%INSTALL_BASE%/install.ps1' -OutFile '%TEMP_SCRIPT%'"
)

if not exist "%TEMP_SCRIPT%" (
  echo Failed to download keel PowerShell installer. 1>&2
  exit /b 1
)

powershell -NoProfile -ExecutionPolicy Bypass -File "%TEMP_SCRIPT%" %*
set "INSTALL_EXIT=%ERRORLEVEL%"
del "%TEMP_SCRIPT%" >nul 2>nul
exit /b %INSTALL_EXIT%
