# Prepares a release: signed NSIS installer, its updater signature and latest.json.
#   .\tools\release.ps1 -Notes "What changed"            build into dist\release\v<version>
#   .\tools\release.ps1 -Notes "What changed" -Publish   also create the GitHub release (gh)
# -Notes is the short text the app shows in its update dialog (latest.json). The release
# page uses docs\releases\v<version>.md when it exists, else the same short text.
#
# The updater key lives outside the repository:
#   %USERPROFILE%\.tauri\provider-gateway-for-claude.key   (minisign private key)
#   %USERPROFILE%\.tauri\provider-gateway-for-claude.pass  (its password, Windows DPAPI)
# Override with TAURI_SIGNING_PRIVATE_KEY / TAURI_SIGNING_PRIVATE_KEY_PASSWORD.
param(
    [Parameter(Mandatory = $true)][string]$Notes,
    [string]$Repo = 'd-ridelman/provider-gateway-for-claude',
    [switch]$Publish
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
$conf = Get-Content (Join-Path $root 'crates\ctp-app\tauri.conf.json') -Raw | ConvertFrom-Json
if ($conf.version -ne $version) { throw "tauri.conf.json version $($conf.version) != Cargo.toml $version" }
$tag = "v$version"

$ownKey = -not $env:TAURI_SIGNING_PRIVATE_KEY
if ($ownKey) {
    $keyDir = Join-Path $env:USERPROFILE '.tauri'
    $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content (Join-Path $keyDir 'provider-gateway-for-claude.key') -Raw
    $secure = Get-Content (Join-Path $keyDir 'provider-gateway-for-claude.pass') | ConvertTo-SecureString
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [System.Net.NetworkCredential]::new('', $secure).Password
}

try {
    & (Join-Path $root 'build-app.ps1') -Installer
} finally {
    # Drop only what this script loaded; a key the caller set stays theirs.
    if ($ownKey) {
        Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY, Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
    }
}

$bundle = Join-Path $root 'target\release\bundle\nsis'
$setup = Get-ChildItem $bundle -Filter "*_${version}_x64-setup.exe" | Select-Object -First 1
if (-not $setup) { throw "installer for $version not found in $bundle" }
$sig = "$($setup.FullName).sig"
if (-not (Test-Path $sig)) { throw "updater signature not found: $sig (is createUpdaterArtifacts on?)" }

$out = Join-Path $root "dist\release\$tag"
New-Item -ItemType Directory -Force $out | Out-Null
# A space-free asset name keeps the download URL simple.
$assetName = "Provider-Gateway-for-Claude_${version}_x64-setup.exe"
Copy-Item $setup.FullName (Join-Path $out $assetName) -Force
Copy-Item $sig (Join-Path $out "$assetName.sig") -Force
Copy-Item (Join-Path $root 'dist\Provider Gateway for Claude.exe') (Join-Path $out "Provider-Gateway-for-Claude_${version}_x64-portable.exe") -Force

$latest = [ordered]@{
    version   = $version
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    platforms = [ordered]@{
        'windows-x86_64' = [ordered]@{
            signature = (Get-Content $sig -Raw).Trim()
            url       = "https://github.com/$Repo/releases/download/$tag/$assetName"
        }
    }
}
$latest | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $out 'latest.json') -Encoding utf8NoBOM

Get-ChildItem $out -Filter *.exe | ForEach-Object {
    "$((Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower())  $($_.Name)"
} | Set-Content (Join-Path $out 'SHA256SUMS.txt') -Encoding ascii

# The release must not carry the builder's user name (panic paths are remapped in build-app.ps1).
$user = [Environment]::UserName
foreach ($file in Get-ChildItem $out -Filter *.exe) {
    $bytes = [IO.File]::ReadAllBytes($file.FullName)
    foreach ($enc in [Text.Encoding]::ASCII, [Text.Encoding]::Unicode) {
        $needle = $enc.GetBytes($user)
        # The owner's public GitHub login (update URL, copyright) may contain the user name;
        # a local path such as Users\<name> still fails the check.
        $owner = $Repo.Split('/')[0]
        $text = [Text.Encoding]::Latin1.GetString($bytes).Replace(
            [Text.Encoding]::Latin1.GetString($enc.GetBytes($owner)), '')
        if ($text.Contains([Text.Encoding]::Latin1.GetString($needle))) {
            throw "$($file.Name) contains the user name '$user' ($($enc.EncodingName))"
        }
    }
}
Write-Host "Release files: $out"
Get-ChildItem $out | ForEach-Object { Write-Host "  $($_.Name)" }

if ($Publish) {
    $files = Get-ChildItem $out | ForEach-Object FullName
    $page = Join-Path $root "docs\releases\$tag.md"
    $body = if (Test-Path $page) { @('--notes-file', $page) } else { @('--notes', $Notes) }
    & gh release create $tag @files --repo $Repo --title "Provider Gateway for Claude $version" @body
    if ($LASTEXITCODE -ne 0) { throw "gh release create failed ($LASTEXITCODE)" }
    Write-Host "Published: https://github.com/$Repo/releases/tag/$tag"
}
