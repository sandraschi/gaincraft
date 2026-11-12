#Requires -Version 5.1
<#
.SYNOPSIS
    gaincraft release: gates, release build, smoke, plain exes + NSIS installer + mcpb, checksums.

.DESCRIPTION
    Rust craft adaptation of the fleet release template (splatcraft model). The app is egui and
    ships as plain executables under stable names (no zip as the download). Extras: a per-user
    NSIS installer, the mcpb bundle, and the craft-launcher portable zip (same exes, launcher
    discovery only, never the sole download).
    By default this only BUILDS and STAGES artifacts in dist/. Nothing is tagged, pushed or
    uploaded unless -Publish is given. Exits at the first failed phase.

.PARAMETER SkipGates
    Skip fmt, clippy and tests (they must have been run just before).
.PARAMETER SkipMcpb
    Skip the mcpb bundle.
.PARAMETER SkipInstaller
    Skip the NSIS installer.
.PARAMETER AllowDirty
    Build from a dirty tree (dry runs only; refused together with -Publish).
.PARAMETER Publish
    After a successful build: tag, push the tag, create the GitHub release with
    CHANGELOG_LATEST.md as notes and upload the artifacts. Requires gh and a pushed, clean main.

.EXAMPLE
    powershell -NoProfile -File scripts/release.ps1
    powershell -NoProfile -File scripts/release.ps1 -Publish
#>
param(
    [switch]$SkipGates,
    [switch]$SkipMcpb,
    [switch]$SkipInstaller,
    [switch]$AllowDirty,
    [switch]$Publish
)

$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path -Parent $PSScriptRoot
$RepoName = 'gaincraft'
$Dist = Join-Path $RepoRoot 'dist'
$Trunk = 'main'
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

function Write-Phase { param([string]$m) Write-Host ''; Write-Host "=== $m ===" -ForegroundColor Cyan }
function Write-Ok { param([string]$m) Write-Host "  [ok] $m" -ForegroundColor Green }
function Write-Info { param([string]$m) Write-Host "  [..] $m" -ForegroundColor Gray }
function Fail { param([string]$m) Write-Host "  [FAIL] $m" -ForegroundColor Red; throw $m }
function Need { param([int]$code, [string]$step) if ($code -ne 0) { Fail "$step exited with code $code" } }

# ---- Phase 0: preflight ------------------------------------------------------
Write-Phase 'Phase 0: preflight'
Set-Location $RepoRoot
$cargo = Get-Content (Join-Path $RepoRoot 'Cargo.toml') -Raw
if ($cargo -notmatch '(?ms)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') { Fail 'workspace version not found in Cargo.toml' }
$Version = $Matches[1]
$Tag = "v$Version"
Write-Ok "Version $Version, tag $Tag"

if ($Publish -and $AllowDirty) { Fail '-AllowDirty is for dry runs; it cannot be combined with -Publish' }
$dirty = git status --porcelain
if ($dirty -and -not $AllowDirty) { Fail "working tree is dirty:`n$dirty" }
if ($dirty) { Write-Info 'dirty tree allowed (dry run)' } else { Write-Ok 'tree clean' }

$branch = (git rev-parse --abbrev-ref HEAD).Trim()
if ($branch -ne $Trunk) { Fail "on branch '$branch', releases are cut from $Trunk" }
$extra = git branch --format='%(refname:short)' | Where-Object { $_ -ne $Trunk }
if ($extra) { Fail "extra local branches exist: $($extra -join ', '). Merge or delete them first (see AGENTS.md)" }
Write-Ok "on $Trunk, no stray branches"

if (git tag --list $Tag) { Fail "tag $Tag already exists locally; bump the workspace version" }
if ($Publish) {
    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { Fail 'gh CLI not found' }
    if (git ls-remote --tags origin "refs/tags/$Tag") { Fail "tag $Tag already exists on the remote" }
    $prevEap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    git fetch -q origin $Trunk 2>$null; $fetchCode = $LASTEXITCODE
    $ErrorActionPreference = $prevEap
    Need $fetchCode "git fetch origin $Trunk"
    $ahead = git rev-list --count "origin/$Trunk..HEAD"
    if ($ahead -ne '0') { Fail "$ahead local commit(s) not pushed; push $Trunk first" }
}
$latest = Join-Path $RepoRoot 'CHANGELOG_LATEST.md'
if (-not (Test-Path $latest)) { Fail 'CHANGELOG_LATEST.md missing' }
if ((Get-Content $latest -TotalCount 1) -notmatch [regex]::Escape($Version)) { Fail "CHANGELOG_LATEST.md does not start with version $Version" }
$makensis = @('C:\Program Files (x86)\NSIS\makensis.exe', 'C:\Program Files\NSIS\makensis.exe') | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $SkipInstaller -and -not $makensis) { Fail 'makensis not found (install NSIS or pass -SkipInstaller)' }
Write-Ok 'preflight done'

# ---- Phase 1: gates ----------------------------------------------------------
if (-not $SkipGates) {
    Write-Phase 'Phase 1: gates'
    cargo fmt --all --check; Need $LASTEXITCODE 'fmt'
    cargo clippy --workspace --all-targets --all-features -- -D warnings; Need $LASTEXITCODE 'clippy'
    cargo test --workspace; Need $LASTEXITCODE 'tests'
    Write-Ok 'fmt, clippy, tests green'
}

# ---- Phase 2: release build + smoke -----------------------------------------
Write-Phase 'Phase 2: release build and smoke'
cargo build --release --workspace; Need $LASTEXITCODE 'release build'
$app = Join-Path $RepoRoot 'target\release\gaincraft-app.exe'
$cli = Join-Path $RepoRoot 'target\release\gaincraft.exe'
foreach ($f in $app, $cli) { if (-not (Test-Path $f)) { Fail "missing build output: $f" } }
# Smoke on a throwaway database, never the user's.
$smokeDb = Join-Path ([IO.Path]::GetTempPath()) "gaincraft-release-smoke-$PID.db"
& $app --smoke --db $smokeDb | Out-Null; Need $LASTEXITCODE 'gaincraft-app --smoke (status + menu parity + tofu guard)'
& $cli --db $smokeDb status | Out-Null; Need $LASTEXITCODE 'gaincraft status'
$init = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"release-smoke","version":"0"}}}'
$resp = $init | & $cli --db $smokeDb mcp | Select-Object -First 1
if ($resp -notmatch '"result"') { Fail "MCP initialize returned no result: $resp" }
Remove-Item "$smokeDb*" -Force -ErrorAction SilentlyContinue
Write-Ok 'smoke: app (menu parity, tofu guard), cli status, MCP initialize'

# ---- Phase 3: stage artifacts -----------------------------------------------
Write-Phase 'Phase 3: artifacts'
if (Test-Path $Dist) { Remove-Item $Dist -Recurse -Force }
New-Item -ItemType Directory -Force $Dist | Out-Null
# Plain executables under stable names: releases/latest/download/gaincraft.exe is a fixed URL.
$assets = @()
foreach ($f in $app, $cli) {
    $dst = Join-Path $Dist (Split-Path -Leaf $f)
    Copy-Item $f $dst
    Write-Ok "$(Split-Path -Leaf $f): $([math]::Round((Get-Item $dst).Length / 1MB, 1)) MB"
    $assets += $dst
}
if (-not $SkipInstaller) {
    $setup = Join-Path $Dist "$RepoName-$Version-setup.exe"
    & $makensis /V2 "/DPRODUCT_VERSION=$Version" "/DOUT_FILE=$setup" (Join-Path $RepoRoot 'installer\installer.nsi'); Need $LASTEXITCODE 'makensis'
    Write-Ok "installer: $(Split-Path -Leaf $setup), $([math]::Round((Get-Item $setup).Length / 1MB, 1)) MB"
    $assets += $setup
}
# Launcher-only extra (craft-launcher discovery): same two exes, never the sole download.
$zip = Join-Path $Dist "$RepoName-$Version-windows-x64-portable.zip"
Compress-Archive -Path $app, $cli -DestinationPath $zip -Force
Write-Ok "launcher zip (extra): $(Split-Path -Leaf $zip)"
$assets += $zip
if (-not $SkipMcpb) {
    $bunx = Join-Path $env:USERPROFILE '.bun\bin\bunx.exe'
    if (-not (Test-Path $bunx)) { $g = Get-Command bunx -ErrorAction SilentlyContinue; if ($g) { $bunx = $g.Source } else { Fail 'bunx not found' } }
    $mstage = Join-Path $Dist 'mcpb-stage'
    New-Item -ItemType Directory -Force $mstage | Out-Null
    $manifest = (Get-Content (Join-Path $RepoRoot 'mcpb\manifest.json') -Raw) -replace '__VERSION__', $Version
    [IO.File]::WriteAllText((Join-Path $mstage 'manifest.json'), $manifest, (New-Object Text.UTF8Encoding $false))
    Copy-Item $cli $mstage
    $mcpb = Join-Path $Dist "$RepoName-v$Version.mcpb"
    & $bunx '@anthropic-ai/mcpb' validate (Join-Path $mstage 'manifest.json'); Need $LASTEXITCODE 'mcpb validate manifest'
    & $bunx '@anthropic-ai/mcpb' pack $mstage $mcpb; Need $LASTEXITCODE 'mcpb pack'
    Remove-Item $mstage -Recurse -Force
    Write-Ok "mcpb: $([math]::Round((Get-Item $mcpb).Length / 1MB, 1)) MB"
    $assets += $mcpb
}

$sums = foreach ($a in $assets) { "{0}  {1}" -f (Get-FileHash $a -Algorithm SHA256).Hash.ToLower(), (Split-Path -Leaf $a) }
$sumFile = Join-Path $Dist 'SHA256SUMS.txt'
[IO.File]::WriteAllText($sumFile, (($sums -join "`n") + "`n"), (New-Object Text.UTF8Encoding $false))
$assets += $sumFile
$sums | ForEach-Object { Write-Info $_ }

# ---- Phase 4: publish (opt-in) ----------------------------------------------
if ($Publish) {
    Write-Phase "Phase 4: publish $Tag"
    git tag $Tag; Need $LASTEXITCODE 'git tag'
    git push origin $Tag; Need $LASTEXITCODE 'git push tag'
    gh release create $Tag --title "$RepoName $Tag" --notes-file $latest; Need $LASTEXITCODE 'gh release create'
    foreach ($a in $assets) { gh release upload $Tag $a --clobber; Need $LASTEXITCODE "upload $(Split-Path -Leaf $a)" }
    Write-Ok "https://github.com/sandraschi/$RepoName/releases/tag/$Tag"
} else {
    Write-Phase 'Done (not published)'
    Write-Info "Artifacts are in $Dist. Re-run with -Publish to tag, push and upload."
}
