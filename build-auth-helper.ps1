$ErrorActionPreference = "Stop"

$root = [System.IO.Path]::GetFullPath($PSScriptRoot)
$project = Join-Path $root "auth-helper\VexMicrosoftAuth.csproj"
$output = [System.IO.Path]::GetFullPath((Join-Path $root "src-tauri\resources\auth-helper"))
if (-not $output.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "O destino do componente de login saiu da pasta do projeto."
}

$tools = Join-Path $root "..\devtools"
if (Test-Path -LiteralPath $tools) {
    $env:NUGET_PACKAGES = Join-Path $tools "nuget-packages"
    $env:TEMP = Join-Path $tools "temp"
    $env:TMP = $env:TEMP
}

$dotnet = "dotnet"
if (Test-Path -LiteralPath "C:\Program Files\dotnet\dotnet.exe") {
    $dotnet = "C:\Program Files\dotnet\dotnet.exe"
}

& $dotnet publish $project -c Release -r win-x64 -o $output --nologo

Remove-Item -LiteralPath (Join-Path $output "Microsoft.Web.WebView2.Wpf.dll") -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath (Join-Path $output "VexMicrosoftAuth.pdb") -Force -ErrorAction SilentlyContinue
$runtimeOutput = Join-Path $output "runtimes"
if (Test-Path -LiteralPath $runtimeOutput) {
    $resolvedRuntime = [System.IO.Path]::GetFullPath($runtimeOutput)
    if (-not $resolvedRuntime.StartsWith($output, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "O destino de limpeza do componente de login é inválido."
    }
    Remove-Item -LiteralPath $resolvedRuntime -Recurse -Force
}
