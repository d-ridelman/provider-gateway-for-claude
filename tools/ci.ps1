# All project checks in one command. People, agents, the pre-push hook and the GitHub
# workflow run this same script, so local and remote checks never drift apart.
#   .\tools\ci.ps1             every check
#   .\tools\ci.ps1 -Changed    skip everything when only docs changed since the last push
#                              (the pre-push hook uses it)
# Cheap checks go first; the first failure stops the run with a non-zero exit code.
param([switch]$Changed)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

# On the owner's machine: two build jobs at most. A CI runner (CI=true) uses all its cores.
if (-not $env:CI -and -not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '2' }

if ($Changed) {
    # Against what the remote already has; no upstream yet - check everything.
    $base = git rev-parse --verify --quiet '@{push}' 2>$null
    if ($LASTEXITCODE -eq 0 -and $base) {
        # A broken base (two refs in @{push}) must not look like "no code changed".
        $files = git diff --name-only "$base" HEAD
        if ($LASTEXITCODE -ne 0) {
            Write-Host 'ci: cannot diff against the push base, checking everything'
        } else {
            $code = $files | Where-Object { $_ -notmatch '^(docs/|.*\.md$)' }
            if (-not $code) {
                Write-Host 'ci: only docs changed since the last push, checks skipped'
                exit 0
            }
        }
    }
}

$total = [Diagnostics.Stopwatch]::StartNew()
function Invoke-Step([string]$Name, [scriptblock]$Command) {
    Write-Host "== $Name"
    $watch = [Diagnostics.Stopwatch]::StartNew()
    & $Command
    if ($LASTEXITCODE -ne 0) {
        Write-Host "ci: $Name FAILED" -ForegroundColor Red
        exit 1
    }
    Write-Host ("   ok ({0:n1} s)" -f $watch.Elapsed.TotalSeconds)
}

Invoke-Step 'translations' { node tools/check_i18n.js }
Invoke-Step 'format' { cargo fmt --all --check }
Invoke-Step 'clippy' { cargo clippy --workspace --all-targets --locked -- -D warnings }
Invoke-Step 'tests' { cargo test --workspace --locked }
Write-Host ("ci: all checks passed ({0:n0} s)" -f $total.Elapsed.TotalSeconds) -ForegroundColor Green
