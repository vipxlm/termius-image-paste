[CmdletBinding()]
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
if (-not $IsWindows -or [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
    throw '此脚本仅支持 Windows x86_64，请使用 PowerShell 7。'
}
$projectRoot = Split-Path -Parent $PSScriptRoot
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $projectRoot 'src-tauri/target' }
if (-not [System.IO.Path]::IsPathRooted($targetDir)) { $targetDir = Join-Path (Get-Location) $targetDir }
if (-not $SkipBuild) {
    & cargo build --locked --bins --release --features tauri/custom-protocol --manifest-path (Join-Path $projectRoot 'src-tauri/Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'Windows release 编译失败。' }
}
$binary = Join-Path $targetDir 'release/termius-image-paste.exe'
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "找不到 release 程序：$binary" }
$outputDir = Join-Path $projectRoot 'dist'
$notices = Join-Path $outputDir 'THIRD_PARTY_NOTICES-windows-x86_64.txt'
if (-not (Test-Path -LiteralPath $notices -PathType Leaf) -or (Get-Item -LiteralPath $notices).Length -eq 0) {
    throw '缺少第三方许可清单；请先运行 node tools/third-party-notices.mjs dist/THIRD_PARTY_NOTICES-windows-x86_64.txt。'
}
New-Item -ItemType Directory -Path $outputDir -Force | Out-Null
$executable = Join-Path $outputDir 'TermiusImagePaste-windows-x86_64.exe'
Copy-Item -LiteralPath $binary -Destination $executable -Force
$stage = Join-Path ([System.IO.Path]::GetTempPath()) ('termius-package-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    Copy-Item -LiteralPath $binary -Destination (Join-Path $stage 'TermiusImagePaste.exe')
    Copy-Item -LiteralPath (Join-Path $projectRoot 'LICENSE'), (Join-Path $projectRoot 'README.md') -Destination $stage
    Copy-Item -LiteralPath $notices -Destination (Join-Path $stage 'THIRD_PARTY_NOTICES.txt')
    Compress-Archive -LiteralPath (Join-Path $stage 'TermiusImagePaste.exe'), (Join-Path $stage 'LICENSE'), (Join-Path $stage 'README.md'), (Join-Path $stage 'THIRD_PARTY_NOTICES.txt') -DestinationPath (Join-Path $outputDir 'TermiusImagePaste-windows-x86_64.zip') -Force
} finally {
    # Only remove this invocation's newly created temporary staging directory.
    if ((Split-Path -Parent $stage) -ne [System.IO.Path]::GetTempPath().TrimEnd('\', '/')) { throw '临时打包路径不符合预期。' }
    Remove-Item -LiteralPath $stage -Recurse -Force
}
Write-Output "Windows 便携程序及 ZIP 已生成：$outputDir"
