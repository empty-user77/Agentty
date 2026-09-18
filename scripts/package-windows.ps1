# Builds dist\agentty-<version>-windows-<arch>.zip: the release binary (a GUI app, no console window)
# with install.ps1 / install.cmd (per-user install: Start menu, agentty:// links, PATH) and the
# license files. Run on Windows from the repository root: `pwsh scripts/package-windows.ps1`.
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x86_64' }
$name = "agentty-$version-windows-$arch"
$stage = Join-Path 'target\package' $name

cargo build --release -p agentty-app
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\agentty.exe, LICENSE, THIRD_PARTY_NOTICES.md, packaging\windows\README.txt, packaging\windows\install.ps1, packaging\windows\install.cmd $stage

New-Item -ItemType Directory -Force dist | Out-Null
$zip = "dist\$name.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path "$stage\*" -DestinationPath $zip
(Get-FileHash $zip -Algorithm SHA256).Hash.ToLower() + "  $name.zip" | Out-File -Encoding ascii "$zip.sha256"
Write-Host $zip
