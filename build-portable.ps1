$ErrorActionPreference = "Stop"

$tools = Join-Path $PSScriptRoot "..\devtools"
if (Test-Path -LiteralPath $tools) {
    $env:CARGO_HOME = Join-Path $tools "cargo"
    $env:RUSTUP_HOME = Join-Path $tools "rustup"
    $env:CARGO_TARGET_DIR = "D:\MINEMEULAUCHER\build-cache\launcher-experimental"
    $env:TEMP = Join-Path $tools "temp"
    $env:TMP = $env:TEMP
    $env:LOCALAPPDATA = Join-Path $tools "cache\localappdata"
    $env:APPDATA = Join-Path $tools "cache\appdata"
    $env:npm_config_cache = Join-Path $tools "npm-cache"

    $node = Join-Path $tools "node\node-v24.16.0-win-x64"
    $mingw = Join-Path $tools "winlibs\mingw64\bin"
    $env:PATH = "$node;$env:CARGO_HOME\bin;$mingw;$env:PATH"
    & (Join-Path $PSScriptRoot "build-auth-helper.ps1")
    & (Join-Path $node "npm.cmd") run tauri -- build
} else {
    & (Join-Path $PSScriptRoot "build-auth-helper.ps1")
    npm run tauri -- build
}

$releaseRoot = if ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR "release" } else { Join-Path $PSScriptRoot "src-tauri\target\release" }
$portableOutput = Join-Path $releaseRoot "VEX Launcher Portable.exe"
Copy-Item -LiteralPath (Join-Path $releaseRoot "vex-launcher.exe") -Destination $portableOutput -Force
Write-Host "Portable: $portableOutput"
