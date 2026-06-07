$ErrorActionPreference = "Stop"

$tools = Join-Path $PSScriptRoot "..\devtools"
if (Test-Path -LiteralPath $tools) {
    $env:CARGO_HOME = Join-Path $tools "cargo"
    $env:RUSTUP_HOME = Join-Path $tools "rustup"
    $env:TEMP = Join-Path $tools "temp"
    $env:TMP = $env:TEMP
    $env:LOCALAPPDATA = Join-Path $tools "cache\localappdata"
    $env:APPDATA = Join-Path $tools "cache\appdata"
    $env:npm_config_cache = Join-Path $tools "npm-cache"

    $node = Join-Path $tools "node\node-v24.16.0-win-x64"
    $mingw = Join-Path $tools "winlibs\mingw64\bin"
    $env:PATH = "$node;$env:CARGO_HOME\bin;$mingw;$env:PATH"
    & (Join-Path $node "npm.cmd") run tauri dev
} else {
    npm run tauri dev
}
