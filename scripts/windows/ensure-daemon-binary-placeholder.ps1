<#
.SYNOPSIS
Ensure the Tauri externalBin path exists for the host triple.

.DESCRIPTION
Windows counterpart of scripts/macos/ensure-daemon-binary-placeholder.sh, for
dev `cargo check` / `tauri dev`. Release builds use prepare-daemon-binary.ps1
to place a real binary.
#>
$ErrorActionPreference = 'Stop'

$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$binDir = Join-Path $root 'apps\desktop\src-tauri\binaries'
New-Item -ItemType Directory -Path $binDir -Force | Out-Null

$host_triple = (rustc -vV | Select-String -Pattern '^host: (.+)$').Matches[0].Groups[1].Value
if (-not $host_triple) {
    throw 'Could not detect rustc host triple'
}

$dest = Join-Path $binDir "lumen-daemon-$host_triple.exe"
if (Test-Path -LiteralPath $dest -PathType Leaf) {
    Write-Output "OK $dest"
    exit 0
}

$candidates = @(
    (Join-Path $root 'target\release\lumen-daemon.exe'),
    (Join-Path $root 'target\debug\lumen-daemon.exe'),
    (Join-Path $root "target\$host_triple\release\lumen-daemon.exe"),
    (Join-Path $root "target\$host_triple\debug\lumen-daemon.exe")
)
foreach ($candidate in $candidates) {
    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
        Copy-Item -LiteralPath $candidate -Destination $dest -Force
        Write-Output "Copied $candidate -> $dest"
        exit 0
    }
}

# Tauri only needs the path to exist to resolve the sidecar at bundle time.
# A stub that exits non-zero makes a missing real build obvious at runtime
# instead of silently reporting Observe as started.
'exit 127' | Set-Content -LiteralPath (Join-Path $binDir 'lumen-daemon-stub.cmd') -Encoding ASCII
Copy-Item -LiteralPath (Join-Path $binDir 'lumen-daemon-stub.cmd') -Destination $dest -Force
Remove-Item -LiteralPath (Join-Path $binDir 'lumen-daemon-stub.cmd') -Force
Write-Output "Wrote stub $dest"
