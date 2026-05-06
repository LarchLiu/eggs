@echo off
REM Eggs skill launcher (Windows) — no Python required.
REM
REM Mirrors the POSIX `eggs` script next to it: download-on-first-use, cache
REM in %USERPROFILE%\.eggs\bin\eggs.exe, exec on every subsequent call.
REM Periodically compares the hash recorded at download time against the
REM server's latest SHA256SUMS so a new release gets picked up automatically;
REM no local sha tool is needed (PowerShell only fetches sums + parses).
REM
REM Override:
REM   EGGS_RELEASE_URL       base URL, defaults to GitHub Releases
REM   EGGS_BIN_DIR           cache directory, defaults to %USERPROFILE%\.eggs\bin
REM   EGGS_VERIFY_INTERVAL   seconds between SHA256SUMS checks, default 600
REM   EGGS_SKIP_VERIFY=1     always trust the cache (offline / CI)

setlocal enabledelayedexpansion

set "RELEASE_URL=%EGGS_RELEASE_URL%"
if "%RELEASE_URL%"=="" set "RELEASE_URL=https://github.com/larchliu/eggs/releases/latest/download"

set "BIN_DIR=%EGGS_BIN_DIR%"
if "%BIN_DIR%"=="" set "BIN_DIR=%USERPROFILE%\.eggs\bin"
set "CACHE=%BIN_DIR%\eggs.exe"
set "EXPECTED_HASH_FILE=%BIN_DIR%\eggs.sha256"
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

    REM Fetch the server's expected hash for our asset. Empty result means
    REM the SHA256SUMS request failed or the asset wasn't listed.
    set "SERVER_HASH="
    for /f "delims=" %%H in ('powershell -NoProfile -Command "$asset='%ASSET%'; try { $s = (Invoke-WebRequest '%RELEASE_URL%/SHA256SUMS' -UseBasicParsing).Content; foreach ($l in ($s -split [char]10)) { $p = $l -split '\s+', 2; if ($p.Count -eq 2 -and $p[1].Trim() -eq $asset) { $p[0].Trim().ToLower(); break } } } catch { }"') do set "SERVER_HASH=%%H"

    if "!SERVER_HASH!"=="" (
        echo eggs: version check unavailable, using cached binary >&2
        "%CACHE%" %*
        exit /b !ERRORLEVEL!
    )

    set "STORED_HASH="
    if exist "%EXPECTED_HASH_FILE%" (
        for /f "tokens=1 delims= " %%S in (%EXPECTED_HASH_FILE%) do set "STORED_HASH=%%S"
    )

    if /i "!STORED_HASH!"=="!SERVER_HASH!" (
        if not exist "%BIN_DIR%" mkdir "%BIN_DIR%"
        type nul > "%VERIFIED_MARKER%"
        "%CACHE%" %*
        exit /b !ERRORLEVEL!
    )

    if "!STORED_HASH!"=="" (
        echo eggs: no recorded hash for cached binary; re-downloading >&2
    ) else (
        echo eggs: cached binary differs from latest release; re-downloading >&2
    )
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
    powershell -NoProfile -Command "try { Invoke-WebRequest -Uri '%URL%' -OutFile '%CACHE%.tmp' -UseBasicParsing } catch { exit 1 }"
    if !ERRORLEVEL! NEQ 0 (
        del /q "%CACHE%.tmp" >nul 2>nul
        echo eggs: download failed >&2
        exit /b !ERRORLEVEL!
    )
)

move /y "%CACHE%.tmp" "%CACHE%" >nul

REM Record server's expected hash for future verifies. Best-effort: if the
REM SHA256SUMS request fails here, leave the hash file absent and the next
REM launch will trigger another download attempt once SHA256SUMS is reachable.
set "FRESH_HASH="
for /f "delims=" %%H in ('powershell -NoProfile -Command "$asset='%ASSET%'; try { $s = (Invoke-WebRequest '%RELEASE_URL%/SHA256SUMS' -UseBasicParsing).Content; foreach ($l in ($s -split [char]10)) { $p = $l -split '\s+', 2; if ($p.Count -eq 2 -and $p[1].Trim() -eq $asset) { $p[0].Trim().ToLower(); break } } } catch { }"') do set "FRESH_HASH=%%H"
if not "!FRESH_HASH!"=="" (
    > "%EXPECTED_HASH_FILE%" echo !FRESH_HASH!
)

type nul > "%VERIFIED_MARKER%"
echo eggs: cached at %CACHE% >&2
"%CACHE%" %*
exit /b %ERRORLEVEL%
