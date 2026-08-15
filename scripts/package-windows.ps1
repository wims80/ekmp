[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [string]$OutputDirectory = "dist"
)

$ErrorActionPreference = "Stop"
$RepositoryRoot = Split-Path -Parent $PSScriptRoot
$Binary = Join-Path $RepositoryRoot "target\release\ekmp.exe"
$ArchiveName = "ekmp-v$Version-x86_64-pc-windows-msvc.zip"

if (-not (Test-Path -LiteralPath $Binary -PathType Leaf)) {
    throw "Release binary not found: $Binary. Run cargo build --release first."
}

$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$ArchivePath = Join-Path $OutputDirectory $ArchiveName
if (Test-Path -LiteralPath $ArchivePath) {
    throw "Refusing to overwrite existing archive: $ArchivePath"
}

$StagingDirectory = Join-Path ([System.IO.Path]::GetTempPath()) "ekmp-$Version-$PID"
$PackageDirectory = Join-Path $StagingDirectory "ekmp-v$Version-x86_64-pc-windows-msvc"

try {
    New-Item -ItemType Directory -Path $PackageDirectory | Out-Null
    Copy-Item -LiteralPath $Binary -Destination (Join-Path $PackageDirectory "ekmp.exe")
    Copy-Item -LiteralPath (Join-Path $RepositoryRoot "packaging\INSTALL.md") -Destination (Join-Path $PackageDirectory "README.md")
    Copy-Item -LiteralPath (Join-Path $RepositoryRoot "LICENSE") -Destination (Join-Path $PackageDirectory "LICENSE")
    Compress-Archive -LiteralPath $PackageDirectory -DestinationPath $ArchivePath
}
finally {
    if (Test-Path -LiteralPath $StagingDirectory) {
        Remove-Item -LiteralPath $StagingDirectory -Recurse -Force
    }
}

Write-Output "Created $ArchivePath"
