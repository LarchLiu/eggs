@echo off
REM Eggs skill launcher (Windows) — no Python required.
REM
REM Mirrors the POSIX `eggs` script next to it: download-on-first-use, cache
REM in %USERPROFILE%\.eggs\bin\eggs.exe, exec on every subsequent call.
REM Periodically re-verifies the cache against the server's SHA256SUMS so a
REM new release gets picked up automatically.
REM
REM Override:
REM   EGGS_RELEASE_URL       base URL, defaults to GitHub Releases
REM   EGGS_BIN_DIR           cache directory, defaults to %USERPROFILE%\.eggs\bin
REM   EGGS_VERIFY_INTERVAL   seconds between sha256 checks, default 600
REM   EGGS_SKIP_VERIFY=1     always trust the cache (offline / CI)

setlocal enabledelayedexpansion

set "RELEASE_URL=%EGGS_RELEASE_URL%"
if "%RELEASE_URL%"=="" set "RELEASE_URL=https://github.com/larchliu/eggs/releases/latest/download"

set "BIN_DIR=%EGGS_BIN_DIR%"
if "%BIN_DIR%"=="" set "BIN_DIR=%USERPROFILE%\.eggs\bin"
set "CACHE=%BIN_DIR%\eggs.exe"
set "VERIFIED_MARKER=%BIN_DIR%\.verified"
set "VERIFY_INTERVAL=%EGGS_VERIFY_INTERVAL%"
if "%VERIFY_INTERVAL%"=="" set "VERIFY_INTERVAL=600"
set "ASSET=eggs-windows-x86_64.exe"

if exist "%CACHE%" (
    REM Fast path: skip-verify flag or marker fresher than the interval.
    set "TRUST_CACHE=0"
    if "%EGGS_SKIP_VERIFY%"=="1" set "TRUST_CACHE=1"
    if "!TRUST_CACHE!"=="0" if exist "%VERIFIED_MARKER%" (
        for /f "delims=" %%A in ('powershell -NoProfile -Command "[int]((Get-Date) - (Get-Item '%VERIFIED_MARKER%').LastWriteTime).TotalSeconds"') do set "MARKER_AGE=%%A"
        if !MARKER_AGE! GEQ 0 if !MARKER_AGE! LSS !VERIFY_INTERVAL! set "TRUST_CACHE=1"
    )
    if "!TRUST_CACHE!"=="1" (
        "%CACHE%" %*
        exit /b !ERRORLEVEL!
    )

    REM Verify cache hash against server SHA256SUMS.
    REM   exit 0 = match, 1 = mismatch, 2 = inconclusive (net/parse error).
    powershell -NoProfile -Command ^
        "$ErrorActionPreference='Stop';" ^
        "try {" ^
        "  $local = (Get-FileHash -Path '%CACHE%' -Algorithm SHA256).Hash.ToLower();" ^
        "  $sums  = (Invoke-WebRequest -Uri '%RELEASE_URL%/SHA256SUMS' -UseBasicParsing).Content;" ^
        "  $line  = ($sums -split \"`n\") | Where-Object { $_ -match ('\s' + [regex]::Escape('%ASSET%') + '\s*$') } | Select-Object -First 1;" ^
        "  if (-not $line) { exit 2 }" ^
        "  $expected = ($line -split '\s+')[0].ToLower();" ^
        "  if ($local -eq $expected) { exit 0 } else { exit 1 }" ^
        "} catch { exit 2 }"
    set "VERIFY_RC=!ERRORLEVEL!"

    if "!VERIFY_RC!"=="0" (
        if not exist "%BIN_DIR%" mkdir "%BIN_DIR%"
        type nul > "%VERIFIED_MARKER%"
        "%CACHE%" %*
        exit /b !ERRORLEVEL!
    )
    if "!VERIFY_RC!"=="2" (
        echo eggs: version check unavailable, using cached binary >&2
        "%CACHE%" %*
        exit /b !ERRORLEVEL!
    )
    echo eggs: cached binary differs from latest release; re-downloading >&2
    REM Fall through to re-download.
)

REM Prefer a pre-existing eggs on PATH (e.g. installed via NSIS, cli_install
REM ran) over a fresh download.
where eggs.exe >nul 2>nul
if %ERRORLEVEL% == 0 (
    for /f "delims=" %%P in ('where eggs.exe') do (
        if /i not "%%P"=="%CACHE%" (
            "%%P" %*
            exit /b !ERRORLEVEL!
        )
    )
)

set "URL=%RELEASE_URL%/%ASSET%"

if not exist "%BIN_DIR%" mkdir "%BIN_DIR%"

echo eggs: downloading %URL% >&2

REM Win10 1803+ ships curl.exe; older systems get the powershell fallback.
where curl.exe >nul 2>nul
if %ERRORLEVEL% == 0 (
    curl.exe -fL --progress-bar -o "%CACHE%.tmp" "%URL%"
    if !ERRORLEVEL! NEQ 0 (
        del /q "%CACHE%.tmp" >nul 2>nul
        echo eggs: download failed >&2
        exit /b !ERRORLEVEL!
    )
) else (
    powershell -NoProfile -Command ^
        "try { Invoke-WebRequest -Uri '%URL%' -OutFile '%CACHE%.tmp' -UseBasicParsing } catch { exit 1 }"
    if !ERRORLEVEL! NEQ 0 (
        del /q "%CACHE%.tmp" >nul 2>nul
        echo eggs: download failed >&2
        exit /b !ERRORLEVEL!
    )
)

move /y "%CACHE%.tmp" "%CACHE%" >nul
type nul > "%VERIFIED_MARKER%"
echo eggs: cached at %CACHE% >&2
"%CACHE%" %*
exit /b %ERRORLEVEL%
