<#
.SYNOPSIS
Build lumen-daemon for a target triple and place it for Tauri externalBin.

.DESCRIPTION
Windows counterpart of scripts/macos/prepare-daemon-binary.sh.
Tauri expects src-tauri/binaries/<name>-<target-triple>.exe
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$Target
)

$ErrorActionPreference = 'Stop'

$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$binDir = Join-Path $root 'apps\desktop\src-tauri\binaries'
New-Item -ItemType Directory -Path $binDir -Force | Out-Null

if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path $root 'target'
}

Write-Output "Building lumen-daemon for $Target ..."
cargo build -p lumen-daemon --release --target $Target --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed with exit code $LASTEXITCODE"
}

$source = Join-Path $env:CARGO_TARGET_DIR "$Target\release\lumen-daemon.exe"
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    throw "Missing built daemon: $source"
}

$dest = Join-Path $binDir "lumen-daemon-$Target.exe"
Copy-Item -LiteralPath $source -Destination $dest -Force
Write-Output "Prepared $dest"
