# Update db-studio from git and reinstall on Windows
$ErrorActionPreference = "Stop"
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $Root

if (Test-Path (Join-Path $Root ".git")) {
  Write-Host "-> git pull..."
  git pull --ff-only
  if ($LASTEXITCODE -ne 0) { git pull }
}

& (Join-Path $PSScriptRoot "install.ps1")
