# Install db-studio on Windows to %LOCALAPPDATA%\db-studio
$ErrorActionPreference = "Stop"
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $Root

$Dest = Join-Path $env:LOCALAPPDATA "db-studio"
New-Item -ItemType Directory -Force -Path $Dest | Out-Null

Write-Host "-> building release..."
cargo build -p ide-app --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$Src = Join-Path $Root "target\release\db-studio.exe"
if (-not (Test-Path $Src)) {
  throw "binary not found at $Src (need Rust toolchain + cargo)"
}

Copy-Item -Force $Src (Join-Path $Dest "db-studio.exe")
Write-Host "-> installed: $Dest\db-studio.exe"

$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$Dest*") {
  Write-Host "-> adding $Dest to user PATH"
  [Environment]::SetEnvironmentVariable("Path", "$UserPath;$Dest", "User")
  Write-Host "   reopen the terminal for PATH to apply"
}

Write-Host "done. Run: db-studio"
