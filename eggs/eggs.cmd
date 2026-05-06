@echo off
REM Eggs skill launcher (Windows) — no Python required.
REM
REM Mirrors the POSIX `eggs` script next to it: download-on-first-use, cache
REM in %USERPROFILE%\.eggs\bin\eggs.exe, exec on every subsequent call.
REM
REM Override the release source via:
REM   EGGS_RELEASE_URL       base URL, defaults to GitHub Releases
REM   EGGS_BIN_DIR           cache directory, defaults to %USERPROFILE%\.eggs\bin

setlocal enabledelayedexpansion

set "RELEASE_URL=%EGGS_RELEASE_URL%"
if "%RELEASE_URL%"=="" set "RELEASE_URL=https://github.com/larchliu/eggs/releases/latest/download"

set "BIN_DIR=%EGGS_BIN_DIR%"
if "%BIN_DIR%"=="" set "BIN_DIR=%USERPROFILE%\.eggs\bin"
set "CACHE=%BIN_DIR%\eggs.exe"

REM Fast path: cache hit.
if exist "%CACHE%" (
    "%CACHE%" %*
    exit /b %ERRORLEVEL%
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

set "ASSET=eggs-windows-x86_64.exe"
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
echo eggs: cached at %CACHE% >&2
"%CACHE%" %*
exit /b %ERRORLEVEL%
