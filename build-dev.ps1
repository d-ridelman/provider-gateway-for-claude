# Builds a DEV copy of Provider Gateway for Claude: its own identifier, folder, shortcut and
# data folder, so it lives next to the installed release without touching it.
#   .\build-dev.ps1 -Installer   NSIS installer in dist-dev\
#   .\build-dev.ps1             portable exe in dist-dev\
param([switch]$Installer)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot

if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '2' }

$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE '.rustup' }
$maps = @(
    "--remap-path-prefix=$cargoHome=/cargo",
    "--remap-path-prefix=$rustupHome=/rustup",
    "--remap-path-prefix=$root=/src",
    "--remap-path-prefix=$env:USERPROFILE=/home"
)
$env:RUSTFLAGS = (@($env:RUSTFLAGS) + $maps | Where-Object { $_ }) -join ' '

$dist = Join-Path $root 'dist-dev'
New-Item -ItemType Directory -Force -Path $dist | Out-Null

if ($Installer) {
    Push-Location (Join-Path $root 'crates\ctp-app')
    try {
        # No updater artifacts: this build must never pretend to be a release.
        & npx --yes @tauri-apps/cli@2.12.6 build --bundles nsis `
            --config tauri.dev.conf.json --config tauri.unsigned.conf.json
        if ($LASTEXITCODE -ne 0) { throw "tauri build failed ($LASTEXITCODE)" }
    } finally { Pop-Location }
    $setup = Get-ChildItem (Join-Path $root 'target\release\bundle\nsis') -Filter '*-setup.exe' |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $setup) { throw 'installer not found in target\release\bundle\nsis' }
    $target = Join-Path $dist ('Provider-Gateway-for-Claude_DEV_' + $setup.Name)
    Copy-Item $setup.FullName $target -Force
    Write-Host "DEV installer: $target (silent install: /S)"
} else {
    Push-Location $root
    try {
        # The identifier is compiled in: without it the portable exe would be the release
        # (same settings folder, same Claude profile) under another file name.
        $env:TAURI_CONFIG = Get-Content (Join-Path $root 'crates\ctp-app\tauri.dev.conf.json') -Raw
        & cargo build --release -p ctp-app
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    } finally { Pop-Location }
    $target = Join-Path $dist 'Provider Gateway for Claude (dev).exe'
    Copy-Item (Join-Path $root 'target\release\ctp-app.exe') $target -Force
    Write-Host "Portable DEV: $target ($([math]::Round((Get-Item $target).Length / 1MB, 1)) MB)"
}

"$((Get-FileHash $target -Algorithm SHA256).Hash.ToLower())  $(Split-Path $target -Leaf)" |
    Set-Content -Path (Join-Path $dist 'SHA256SUMS.txt') -Encoding ascii
Write-Host "Checksums: $(Join-Path $dist 'SHA256SUMS.txt')"
