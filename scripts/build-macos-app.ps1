$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$bundle = Join-Path $root "target/release/Mado.app"
$contents = Join-Path $bundle "Contents"
$macOS = Join-Path $contents "MacOS"
$resources = Join-Path $contents "Resources"

Push-Location $root
try {
    cargo build --release
    New-Item -ItemType Directory -Force -Path $macOS | Out-Null
    New-Item -ItemType Directory -Force -Path $resources | Out-Null
    Copy-Item "target/release/mado" (Join-Path $macOS "mado") -Force
    Copy-Item "packaging/macos/Info.plist" (Join-Path $contents "Info.plist") -Force
    Copy-Item "packaging/macos/Mado.icns" (Join-Path $resources "Mado.icns") -Force
    & chmod +x (Join-Path $macOS "mado")
    & codesign --force --deep --sign - $bundle
    & plutil -lint (Join-Path $contents "Info.plist")
    Write-Host "Built $bundle"
    Write-Host "Open it once, then choose Mado from Finder's Open With menu."
}
finally {
    Pop-Location
}
