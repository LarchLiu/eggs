<#
.SYNOPSIS
  Convenience wrapper for the Tauri desktop pet build (Windows / PowerShell).

.DESCRIPTION
  Mirror of `desktop/dev` (bash). Run from anywhere; the script cd's into
  desktop/src-tauri itself. Output binary lives at
  desktop/src-tauri/target/<profile>/eggs.exe.

.PARAMETER mode
  fast      fast-iteration release build (default; --profile release-fast)
  release   full optimized release (LTO + opt-level=s)
  debug     cargo build (no --release)
  check     cargo check (fastest, no codegen)
  clean     cargo clean
  run       build (fast), then exec the binary with -- the rest of the args
  stop      eggs.exe stop (using whatever binary is already built)
  restart   eggs.exe stop, rebuild fast, then run with the rest of the args
  test      cargo check + go test ./... in server/
  help      print this usage block

.EXAMPLE
  .\desktop\dev.ps1 run remote
  .\desktop\dev.ps1 run pet noir-webling
  .\desktop\dev.ps1 restart remote room ABCD
  .\desktop\dev.ps1 stop
#>

param(
    [Parameter(Position = 0)]
    [string]$mode = "fast",
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$rest = @()
)

$ErrorActionPreference = "Stop"

# Move into src-tauri so cargo finds Cargo.toml regardless of cwd.
$srcTauri = Join-Path $PSScriptRoot "src-tauri"
if (-not (Test-Path $srcTauri)) {
    Write-Error "src-tauri not found at $srcTauri"
    exit 1
}
Set-Location $srcTauri

function Locate-Bin {
    foreach ($p in @(
        "target\release-fast\eggs.exe",
        "target\release\eggs.exe",
        "target\debug\eggs.exe"
    )) {
        if (Test-Path $p) { return $p }
    }
    return $null
}

function Show-Help {
    Get-Content $PSCommandPath | Select-Object -First 30
}

switch ($mode) {
    "fast"          { cargo build --profile release-fast @rest;  exit $LASTEXITCODE }
    "release-fast"  { cargo build --profile release-fast @rest;  exit $LASTEXITCODE }
    "release"       { cargo build --release @rest;               exit $LASTEXITCODE }
    "debug"         { cargo build @rest;                          exit $LASTEXITCODE }
    "check"         { cargo check @rest;                          exit $LASTEXITCODE }
    "clean"         { cargo clean;                                exit $LASTEXITCODE }

    "run" {
        cargo build --profile release-fast
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        & ".\target\release-fast\eggs.exe" @rest
        exit $LASTEXITCODE
    }

    "stop" {
        $bin = Locate-Bin
        if (-not $bin) { Write-Error "no eggs binary built yet"; exit 1 }
        & $bin stop
        exit $LASTEXITCODE
    }

    "restart" {
        $bin = Locate-Bin
        if ($bin) { & $bin stop }
        cargo build --profile release-fast
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        & ".\target\release-fast\eggs.exe" @rest
        exit $LASTEXITCODE
    }

    "test" {
        cargo check
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        Push-Location (Join-Path $srcTauri "..\..\server")
        try {
            go test ./...
            exit $LASTEXITCODE
        } finally {
            Pop-Location
        }
    }

    { @("help", "-h", "--help") -contains $_ } {
        Show-Help
    }

    default {
        Write-Host "unknown mode: $mode" -ForegroundColor Red
        Show-Help
        exit 2
    }
}
