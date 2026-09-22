# Builds dist\agentty-<version>-windows-<arch>.zip: the release binary (a GUI app, no console window)
# with install.ps1 / install.cmd (per-user install: Start menu, agentty:// links, PATH) and the
# license files. Run on Windows from the repository root: `pwsh scripts/package-windows.ps1`.
#
# -Installer also builds the Inno Setup installer from packaging\windows\agentty.iss:
#   dist\Agentty-<version>-windows-x64-setup.exe  and  dist\Agentty-<version>-windows-x64-setup.zip (the same .exe in a
#   zip, for browsers that block unsigned .exe downloads). Needs Inno Setup 6 (winget install JRSoftware.InnoSetup).
param([switch]$Installer)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

# SHA-256 of a file, without `Get-FileHash`: the cmdlet arrived in PowerShell 4 and the build
# machine's PowerShell is older than the rest of this script needs it to be. .NET is always there.
function Get-Sha256 {
    param([string]$Path)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = [System.IO.File]::OpenRead((Resolve-Path -LiteralPath $Path).Path)
        try {
            return [System.BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '').ToLower()
        } finally {
            $stream.Dispose()
        }
    } finally {
        $sha.Dispose()
    }
}

# Inno Setup's compiler: on PATH, else where the installer (per machine or per user via winget) puts it.
function Find-Iscc {
    $onPath = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')
    )
    foreach ($candidate in $candidates) { if ($candidate -and (Test-Path -LiteralPath $candidate)) { return $candidate } }
    return $null
}

Write-Host "PowerShell $($PSVersionTable.PSVersion)"
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x86_64' }
$name = "agentty-$version-windows-$arch"
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$stage = Join-Path $target "package\$name"

# Fail before the long build rather than after it.
$iscc = $null
if ($Installer) {
    if ($arch -ne 'x86_64') { throw "The installer is built for x64 only (this machine is $arch)." }
    $iscc = Find-Iscc
    if (-not $iscc) {
        throw 'Inno Setup 6 (ISCC.exe) was not found. Install it on this machine with: winget install JRSoftware.InnoSetup'
    }
}

cargo build --release -p agentty-app
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item (Join-Path $target 'release\agentty.exe'), LICENSE, THIRD_PARTY_NOTICES.md, packaging\windows\README.txt, packaging\windows\install.ps1, packaging\windows\install.cmd $stage

New-Item -ItemType Directory -Force dist | Out-Null
$zip = "dist\$name.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path "$stage\*" -DestinationPath $zip
(Get-Sha256 $zip) + "  $name.zip" | Out-File -Encoding ascii "$zip.sha256"
Write-Host $zip

if ($Installer) {
    $setupName = "Agentty-$version-windows-x64-setup"
    $out = (Resolve-Path dist).Path
    Remove-Item "$out\$setupName.exe", "$out\$setupName.zip" -ErrorAction SilentlyContinue
    & $iscc /Q "/DAppVersion=$version" "/DBinDir=$((Resolve-Path $stage).Path)" "/O$out" packaging\windows\agentty.iss
    if ($LASTEXITCODE -ne 0) { throw "ISCC failed (exit $LASTEXITCODE)" }
    $setup = "$out\$setupName.exe"
    if (-not (Test-Path -LiteralPath $setup)) { throw "ISCC did not write $setup" }
    $info = (Get-Item -LiteralPath $setup).VersionInfo
    if ($info.ProductVersion -notlike "$version*") { throw "installer version is '$($info.ProductVersion)', expected $version" }
    Compress-Archive -LiteralPath $setup -DestinationPath "$out\$setupName.zip"
    Write-Host "dist\$setupName.exe"
    Write-Host "dist\$setupName.zip"
}
