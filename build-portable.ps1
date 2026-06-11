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
$launcherPayload = Join-Path $releaseRoot "vex-launcher.exe"
$loaderPayload = Join-Path $releaseRoot "WebView2Loader.dll"
$bootstrapSource = Join-Path $PSScriptRoot "portable-bootstrap\Program.cs"
$launcherIcon = Join-Path $PSScriptRoot "src-tauri\icons\icon.ico"
$csc = Join-Path $env:WINDIR "Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path -LiteralPath $csc)) {
    $csc = Join-Path $env:WINDIR "Microsoft.NET\Framework\v4.0.30319\csc.exe"
}
if (-not (Test-Path -LiteralPath $loaderPayload)) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot "src-tauri\resources\WebView2Loader.dll") -Destination $loaderPayload -Force
}
& $csc /nologo /target:winexe /optimize+ "/win32icon:$launcherIcon" "/resource:$launcherPayload,VexLauncher.Payload.exe" "/resource:$loaderPayload,VexLauncher.WebView2Loader.dll" "/out:$portableOutput" $bootstrapSource
if ($LASTEXITCODE -ne 0) {
    throw "Não foi possível gerar o portátil autocontido."
}
Write-Host "Portable: $portableOutput"
