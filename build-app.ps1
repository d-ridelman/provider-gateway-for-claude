# Builds Provider Gateway for Claude.
#   .\build-app.ps1              portable exe in dist\
#   .\build-app.ps1 -Installer   plus the NSIS installer (tauri-cli via npx; needs Node.js)
param([switch]$Installer)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot

# Keep the build to two jobs unless the caller chose otherwise.
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '2' }

# Release binaries keep source paths of panics. Map local folders (user profile, cargo and
# rustup homes, the checkout) to neutral names so no user name ends up in the exe.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE '.rustup' }
$maps = @(
    "--remap-path-prefix=$cargoHome=/cargo",
    "--remap-path-prefix=$rustupHome=/rustup",
    "--remap-path-prefix=$root=/src",
    "--remap-path-prefix=$env:USERPROFILE=/home"
)
$env:RUSTFLAGS = (@($env:RUSTFLAGS) + $maps | Where-Object { $_ }) -join ' '

# License notices of the linked crates go into the installer; refresh them when Python is here.
if (Get-Command python -ErrorAction SilentlyContinue) {
    Push-Location $root
    try {
        & python -I tools\third_party_notices.py THIRD-PARTY-NOTICES.txt
        if ($LASTEXITCODE -ne 0) { throw "third_party_notices.py failed ($LASTEXITCODE)" }
    } finally { Pop-Location }
} else {
    Write-Warning 'Python not found: THIRD-PARTY-NOTICES.txt is not refreshed.'
}

if ($Installer) {
    # Updater artifacts need the project's signing key (tools\release.ps1 loads it).
    # Without it, build a plain installer instead of failing.
    $extra = @()
    if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
        Write-Warning 'TAURI_SIGNING_PRIVATE_KEY is not set: building without updater signatures.'
        $extra = @('--config', 'tauri.unsigned.conf.json')
    }
    Push-Location (Join-Path $root 'crates\ctp-app')
    try {
        & npx --yes @tauri-apps/cli@2.12.6 build --bundles nsis @extra
        if ($LASTEXITCODE -ne 0) { throw "tauri build failed ($LASTEXITCODE)" }
    } finally { Pop-Location }
} else {
    Push-Location $root
    try {
        & cargo build --release -p ctp-app
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    } finally { Pop-Location }
}

$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$exe = Join-Path $dist 'Provider Gateway for Claude.exe'
# tauri build names the binary after mainBinaryName; plain cargo keeps the crate name.
$built = if ($Installer) { 'provider-gateway.exe' } else { 'ctp-app.exe' }
Copy-Item (Join-Path $root "target\release\$built") $exe -Force
Write-Host "Portable: $exe ($([math]::Round((Get-Item $exe).Length / 1MB, 1)) MB)"

if ($Installer) {
    $setup = Get-ChildItem (Join-Path $root 'target\release\bundle\nsis') -Filter '*-setup.exe' |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $setup) { throw 'installer not found in target\release\bundle\nsis' }
    Copy-Item $setup.FullName $dist -Force
    Write-Host "Installer: $(Join-Path $dist $setup.Name) (silent install: /S)"
}

# Checksums for the release page (the binaries are not code-signed).
Get-ChildItem $dist -Filter *.exe | ForEach-Object {
    "$((Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower())  $($_.Name)"
} | Set-Content -Path (Join-Path $dist 'SHA256SUMS.txt') -Encoding ascii
Write-Host "Checksums: $(Join-Path $dist 'SHA256SUMS.txt')"
