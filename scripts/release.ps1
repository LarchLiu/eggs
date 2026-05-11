<#
.SYNOPSIS
  Bump version, commit, tag (Windows / PowerShell).

.DESCRIPTION
  Mirror of scripts/release.sh. Touches:
    desktop/src-tauri/Cargo.toml          version = "..."
    desktop/src-tauri/tauri.conf.json     "version": "..."
    claude-plugin/.claude-plugin/plugin.json   "version": "..."
    codex-plugins/.codex-plugin/plugin.json     "version": "..."
    desktop/src-tauri/Cargo.lock          via `cargo check`

  Then commits + tags vX.Y.Z locally. Does NOT push — review and push:
    git push origin <branch> vX.Y.Z

.EXAMPLE
  .\scripts\release.ps1 0.2.0
#>

param(
    [Parameter(Position = 0)]
    [string]$Version
)

$ErrorActionPreference = 'Stop'

$root      = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$cargoToml = Join-Path $root 'desktop/src-tauri/Cargo.toml'
$tauriConf = Join-Path $root 'desktop/src-tauri/tauri.conf.json'
$claudePlugin = Join-Path $root 'claude-plugin/.claude-plugin/plugin.json'
$codexPlugin = Join-Path $root 'codex-plugins/.codex-plugin/plugin.json'

$releaseTag = (git -C $root tag --list 'v*' --sort=-v:refname | Select-Object -First 1).Trim()

if (-not $PSBoundParameters.ContainsKey('Version') -or [string]::IsNullOrWhiteSpace($Version)) {
    Write-Error @"
missing version
latest release tag: $(if ($releaseTag) { $releaseTag } else { '<none>' })
usage: .\scripts\release.ps1 0.2.0
"@
    exit 2
}

if ($Version -notmatch '^\d+\.\d+\.\d+([-.+].+)?$') {
    Write-Error "version must look like X.Y.Z (got '$Version')"
    exit 2
}

# Refuse if the working tree has uncommitted work — the bump commit below
# would otherwise sweep unrelated changes into the release.
if (git -C $root status --porcelain) {
    Write-Error "working tree has uncommitted changes; commit or stash first"
    exit 1
}

# Refuse if the tag already exists; otherwise `git tag` fails AFTER the
# bump commit lands, leaving an orphan commit to clean up by hand.
if (git -C $root tag -l "v$Version") {
    Write-Error "tag v$Version already exists"
    exit 1
}

(Get-Content $cargoToml) `
    -replace '^version = ".*"', "version = `"$Version`"" |
    Set-Content $cargoToml -NoNewline:$false

(Get-Content $tauriConf) `
    -replace '"version":\s*".*"', "`"version`": `"$Version`"" |
    Set-Content $tauriConf -NoNewline:$false

(Get-Content $claudePlugin) `
    -replace '"version":\s*".*"', "`"version`": `"$Version`"" |
    Set-Content $claudePlugin -NoNewline:$false

(Get-Content $codexPlugin) `
    -replace '"version":\s*".*"', "`"version`": `"$Version`"" |
    Set-Content $codexPlugin -NoNewline:$false

Push-Location (Join-Path $root 'desktop/src-tauri')
try { cargo check } finally { Pop-Location }

git -C $root add `
    desktop/src-tauri/Cargo.toml `
    desktop/src-tauri/tauri.conf.json `
    claude-plugin/.claude-plugin/plugin.json `
    codex-plugins/.codex-plugin/plugin.json `
    desktop/src-tauri/Cargo.lock
git -C $root commit -m "chore: release v$Version"
git -C $root tag "v$Version"

$branch = (git -C $root rev-parse --abbrev-ref HEAD).Trim()
Write-Host ""
Write-Host "release v$Version staged on branch '$branch'."
Write-Host "to publish (triggers .github/workflows/release.yml):"
Write-Host "    git push origin $branch v$Version"
