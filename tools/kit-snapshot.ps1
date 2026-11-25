# Snapshot of agent-kit for the PGC build (ADR-015): the commit pinned in crates\ctp-app\kit.pin,
# only what runs (.claude-plugin, bin, hooks, scripts, skills, templates, agents, LICENSE), with
# "version" in plugin.json set to the commit's first 12 characters. Writes
# crates\ctp-app\kit\agent-kit.tar (not in git); build.rs embeds it into the exe.
#   .\tools\kit-snapshot.ps1                      agent-kit from ..\agent-kit
#   .\tools\kit-snapshot.ps1 -Source <folder>     from another clone
#   .\tools\kit-snapshot.ps1 -Check               only say whether the snapshot matches the pin
param([string]$Source, [switch]$Check)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$pin = (Get-Content (Join-Path $root 'crates\ctp-app\kit.pin') -Raw).Trim()
if ($pin -notmatch '^[0-9a-f]{40}$') { throw "kit.pin must hold a full commit hash, not '$pin'" }
$version = $pin.Substring(0, 12)
$kitDir = Join-Path $root 'crates\ctp-app\kit'
$tar = Join-Path $kitDir 'agent-kit.tar'
$stamp = Join-Path $kitDir 'agent-kit.version'

if ($Check) {
    $have = if (Test-Path $stamp) { (Get-Content $stamp -Raw).Trim() } else { '' }
    if (-not (Test-Path $tar) -or $have -ne $version) {
        throw "agent-kit snapshot is missing or not $version (have '$have'): run tools\kit-snapshot.ps1"
    }
    Write-Host "agent-kit snapshot: $version"
    return
}

if (-not $Source) { $Source = Join-Path (Split-Path $root -Parent) 'agent-kit' }
if (-not (Test-Path (Join-Path $Source '.git'))) { throw "no agent-kit clone at $Source" }
& git -C $Source cat-file -e "$pin^{commit}" 2>$null
if ($LASTEXITCODE -ne 0) { throw "commit $pin is not in $Source" }

$work = Join-Path ([IO.Path]::GetTempPath()) ("pgc-kit-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $work | Out-Null
try {
    # bin: the `agent-kit` command the skill and agents call (the plugin puts it on PATH).
    $parts = '.claude-plugin', 'bin', 'hooks', 'scripts', 'skills', 'templates', 'agents', 'LICENSE'
    $present = @(& git -C $Source ls-tree --name-only $pin) | Where-Object { $parts -contains $_ }
    foreach ($need in '.claude-plugin', 'hooks', 'scripts') {
        if ($present -notcontains $need) { throw "agent-kit $version has no $need" }
    }
    & git -C $Source archive --format=tar -o (Join-Path $work 'src.tar') $pin @present
    if ($LASTEXITCODE -ne 0) { throw "git archive failed ($LASTEXITCODE)" }
    $tree = Join-Path $work 'tree'
    New-Item -ItemType Directory -Force $tree | Out-Null
    & tar -xf (Join-Path $work 'src.tar') -C $tree
    if ($LASTEXITCODE -ne 0) { throw "tar -x failed ($LASTEXITCODE)" }
    # The version Claude Code keys its plugin cache on: without it a folder plugin is "unknown".
    $manifest = Join-Path $tree '.claude-plugin\plugin.json'
    $json = Get-Content $manifest -Raw | ConvertFrom-Json
    $json | Add-Member -NotePropertyName version -NotePropertyValue $version -Force
    [IO.File]::WriteAllText($manifest, ($json | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
    New-Item -ItemType Directory -Force $kitDir | Out-Null
    $next = "$tar.tmp"
    & tar -cf $next -C $tree @present
    if ($LASTEXITCODE -ne 0) { throw "tar -c failed ($LASTEXITCODE)" }
    Move-Item -Force $next $tar
    [IO.File]::WriteAllText($stamp, $version, [Text.UTF8Encoding]::new($false))
    Write-Host "agent-kit snapshot: $version ($([math]::Round((Get-Item $tar).Length / 1KB)) KB) -> $tar"
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
