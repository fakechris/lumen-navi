<#
.SYNOPSIS
Verify the NSIS bundle and copy it out under the canonical release name.
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$BundleDirectory,

    [Parameter(Mandatory = $true)]
    [string]$VersionTag,

    [Parameter(Mandatory = $true)]
    [string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'

if ($VersionTag -notmatch '^v\d+\.\d+\.\d+') {
    throw "VersionTag must start with vMAJOR.MINOR.PATCH: $VersionTag"
}

$bundle = (Resolve-Path -LiteralPath $BundleDirectory).Path
$installers = @(Get-ChildItem -LiteralPath $bundle -File -Filter '*-setup.exe')
if ($installers.Count -ne 1) {
    throw "Expected exactly one NSIS installer in $bundle, found $($installers.Count)"
}

$expectedVersion = $VersionTag.TrimStart('v')
if ($installers[0].Name -notmatch [regex]::Escape($expectedVersion)) {
    throw "Installer $($installers[0].Name) does not carry tag version $expectedVersion"
}

# An installer without the bundled WebView2 bootstrapper and the daemon
# sidecar comes out implausibly small; catch that before it reaches a Release.
$minBytes = 1MB
if ($installers[0].Length -lt $minBytes) {
    throw "Installer is only $($installers[0].Length) bytes; expected at least $minBytes"
}

New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$output = Join-Path $OutputDirectory "Lumen-Navi-$VersionTag-windows-x64-setup.exe"
Copy-Item -LiteralPath $installers[0].FullName -Destination $output -Force

if (-not (Test-Path -LiteralPath $output -PathType Leaf)) {
    throw "Windows release asset was not created: $output"
}

Write-Output $output
