[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$version = "21.1.8"
$expectedSha256 = "7a5386c26497db1691f320121e5b113364dd0274b98e55f15f4dbc00c0450113"
$installerName = "LLVM-$version-win64.exe"
$installerUrl = "https://github.com/llvm/llvm-project/releases/download/llvmorg-$version/$installerName"
$installerPath = Join-Path $env:RUNNER_TEMP $installerName
$llvmRoot = Join-Path $env:RUNNER_TEMP "llvm-$version"
$llvmBin = Join-Path $llvmRoot "bin"
$clangPath = Join-Path $llvmBin "clang.exe"
$libclangPath = Join-Path $llvmBin "libclang.dll"

Write-Host "Downloading LLVM $version from its pinned upstream release asset"
& curl.exe --fail --location --retry 3 --output $installerPath $installerUrl
if ($LASTEXITCODE -ne 0) {
  throw "LLVM download failed with exit code $LASTEXITCODE"
}

$actualSha256 = (Get-FileHash -Algorithm SHA256 $installerPath).Hash.ToLowerInvariant()
if ($actualSha256 -ne $expectedSha256) {
  throw "LLVM installer SHA-256 mismatch: expected $expectedSha256, got $actualSha256"
}

$install = Start-Process -FilePath $installerPath `
  -ArgumentList @("/S", "/D=$llvmRoot") `
  -PassThru `
  -Wait
if ($install.ExitCode -ne 0) {
  throw "LLVM installer failed with exit code $($install.ExitCode)"
}
Remove-Item -Force $installerPath
if (-not (Test-Path $clangPath)) {
  throw "LLVM installer did not create $clangPath"
}
if (-not (Test-Path $libclangPath)) {
  throw "LLVM installer did not create $libclangPath"
}

$llvmBin | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append
"LIBCLANG_PATH=$llvmBin" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
& $clangPath --version
