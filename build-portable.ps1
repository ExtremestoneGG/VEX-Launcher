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
$portableDirectory = Join-Path $releaseRoot "VEX Launcher Portable"
$portableZip = Join-Path $releaseRoot "VEX Launcher Portable.zip"
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

$portableResolvedRoot = [IO.Path]::GetFullPath($portableDirectory)
$releaseResolvedRoot = [IO.Path]::GetFullPath($releaseRoot)
if (-not $portableResolvedRoot.StartsWith($releaseResolvedRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Pasta portátil fora do diretório de build."
}
if (Test-Path -LiteralPath $portableDirectory) {
    Remove-Item -LiteralPath $portableDirectory -Recurse -Force
}
New-Item -ItemType Directory -Path $portableDirectory | Out-Null
Copy-Item -LiteralPath $launcherPayload -Destination (Join-Path $portableDirectory "VEX Launcher.exe")
Copy-Item -LiteralPath $loaderPayload -Destination (Join-Path $portableDirectory "WebView2Loader.dll")
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "PORTABLE.md") -Destination (Join-Path $portableDirectory "README.md")
if (Test-Path -LiteralPath $portableZip) {
    Remove-Item -LiteralPath $portableZip -Force
}
Compress-Archive -Path (Join-Path $portableDirectory "*") -DestinationPath $portableZip -CompressionLevel Optimal

$installerOutput = Get-ChildItem -LiteralPath (Join-Path $releaseRoot "bundle\nsis") -Filter "*-setup.exe" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
$checksumOutput = Join-Path $releaseRoot "SHA256SUMS.txt"
$checksumFiles = @($portableOutput, $portableZip)
if ($installerOutput) {
    $checksumFiles += $installerOutput.FullName
}
$checksumLines = $checksumFiles | ForEach-Object {
    $hash = Get-FileHash -LiteralPath $_ -Algorithm SHA256
    "$($hash.Hash.ToLowerInvariant())  $(Split-Path $_ -Leaf)"
}
Set-Content -LiteralPath $checksumOutput -Value $checksumLines -Encoding UTF8

Write-Host "Portable: $portableOutput"
Write-Host "Portable ZIP recomendado: $portableZip"
Write-Host "Checksums: $checksumOutput"
