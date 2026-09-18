<#
.SYNOPSIS
  Installs Agentty for the current user (no administrator rights needed).

.DESCRIPTION
  Copies agentty.exe to %LOCALAPPDATA%\Programs\Agentty and registers, for this user only:
    - a Start Menu shortcut (and a desktop shortcut with -DesktopShortcut)
    - agentty:// links (plugins, "Continue in Agentty" buttons)
    - "Open in Agentty" on folders and folder backgrounds in Explorer
    - the install folder on PATH, so `agentty notify` / `agentty browser` work in any terminal
    - an entry in Settings > Apps (Add or remove programs) that runs this script with -Uninstall

  Run from the unzipped folder:  powershell -ExecutionPolicy Bypass -File .\install.ps1
  (or double-click install.cmd). Uninstall: Settings > Apps, or  .\install.ps1 -Uninstall
#>
param(
    [switch]$Uninstall,
    [switch]$DesktopShortcut,
    [switch]$NoLaunch
)

$ErrorActionPreference = 'Stop'
$AppName = 'Agentty'
$Dest = Join-Path $env:LOCALAPPDATA 'Programs\Agentty'
$Exe = Join-Path $Dest 'agentty.exe'
$StartMenu = Join-Path ([Environment]::GetFolderPath('Programs')) 'Agentty.lnk'
$Desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Agentty.lnk'
$Classes = 'HKCU:\Software\Classes'
$UninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Agentty'

function Stop-Agentty {
    $running = Get-Process -Name agentty -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Exe }
    if ($running) {
        Write-Host 'Closing the running Agentty...'
        $running | ForEach-Object { $_.CloseMainWindow() | Out-Null }
        Start-Sleep -Seconds 2
        $running | Where-Object { -not $_.HasExited } | Stop-Process -Force
    }
}

function Set-UserPath([bool]$Add) {
    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @()
    if ($current) { $entries = $current -split ';' | Where-Object { $_ -and ($_.TrimEnd('\') -ine $Dest.TrimEnd('\')) } }
    if ($Add) { $entries += $Dest }
    # SetEnvironmentVariable broadcasts the change, so new terminals see it without signing out.
    [Environment]::SetEnvironmentVariable('Path', ($entries -join ';'), 'User')
}

function New-Shortcut([string]$Path) {
    $shell = New-Object -ComObject WScript.Shell
    $link = $shell.CreateShortcut($Path)
    $link.TargetPath = $Exe
    $link.WorkingDirectory = $env:USERPROFILE
    $link.IconLocation = "$Exe,0"
    $link.Description = 'Agentty - multi-agent AI terminal'
    $link.Save()
}

function Set-DefaultValue([string]$Key, [string]$Value) {
    New-Item -Path $Key -Force | Out-Null
    Set-ItemProperty -LiteralPath $Key -Name '(default)' -Value $Value
}

if ($Uninstall) {
    Stop-Agentty
    Remove-Item -LiteralPath $StartMenu, $Desktop -Force -ErrorAction SilentlyContinue
    foreach ($key in @("$Classes\agentty", "$Classes\Directory\shell\Agentty", "$Classes\Directory\Background\shell\Agentty", $UninstallKey)) {
        Remove-Item -LiteralPath $key -Recurse -Force -ErrorAction SilentlyContinue
    }
    Set-UserPath $false
    # The script may be running from the install folder; remove it once this process has exited.
    $cleanup = "Start-Sleep -Seconds 2; Remove-Item -LiteralPath '$($Dest.Replace("'", "''"))' -Recurse -Force"
    Start-Process -WindowStyle Hidden powershell.exe -ArgumentList @('-NoProfile', '-Command', $cleanup)
    Write-Host 'Agentty was removed. Your settings and sessions in %USERPROFILE%\.agentty are kept.'
    return
}

$Source = $PSScriptRoot
$SourceExe = Join-Path $Source 'agentty.exe'
if (-not (Test-Path -LiteralPath $SourceExe)) { throw "agentty.exe was not found next to install.ps1 ($Source)." }

$build = [Environment]::OSVersion.Version.Build
if ($build -lt 17763) { throw "Agentty needs Windows 10 version 1809 (build 17763) or later; this is build $build." }

Stop-Agentty
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
foreach ($file in @('agentty.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'README.txt', 'install.ps1', 'install.cmd')) {
    $from = Join-Path $Source $file
    if ((Test-Path -LiteralPath $from) -and ((Resolve-Path -LiteralPath $from).Path -ne (Join-Path $Dest $file))) {
        Copy-Item -LiteralPath $from -Destination $Dest -Force
    }
}

New-Shortcut $StartMenu
if ($DesktopShortcut) { New-Shortcut $Desktop }

# agentty:// links
$protocol = "$Classes\agentty"
Set-DefaultValue $protocol 'URL:Agentty'
Set-ItemProperty -LiteralPath $protocol -Name 'URL Protocol' -Value ''
Set-DefaultValue "$protocol\DefaultIcon" "`"$Exe`",0"
Set-DefaultValue "$protocol\shell\open\command" "`"$Exe`" `"%1`""

# "Open in Agentty" on folders and inside folders
foreach ($shellKey in @("$Classes\Directory\shell\Agentty", "$Classes\Directory\Background\shell\Agentty")) {
    Set-DefaultValue $shellKey 'Open in Agentty'
    Set-ItemProperty -LiteralPath $shellKey -Name 'Icon' -Value "`"$Exe`",0"
    Set-DefaultValue "$shellKey\command" "`"$Exe`" `"%V`""
}

Set-UserPath $true

# Settings > Apps entry
$version = (Get-Item -LiteralPath $Exe).VersionInfo.ProductVersion
New-Item -Path $UninstallKey -Force | Out-Null
$uninstall = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$(Join-Path $Dest 'install.ps1')`" -Uninstall"
$values = @{
    DisplayName = $AppName; DisplayVersion = "$version"; Publisher = 'Agentty contributors'; DisplayIcon = "`"$Exe`",0"
    InstallLocation = $Dest; UninstallString = $uninstall; URLInfoAbout = 'https://www.agentty.run'
}
foreach ($name in $values.Keys) { Set-ItemProperty -LiteralPath $UninstallKey -Name $name -Value $values[$name] }
Set-ItemProperty -LiteralPath $UninstallKey -Name 'NoModify' -Value 1 -Type DWord
Set-ItemProperty -LiteralPath $UninstallKey -Name 'NoRepair' -Value 1 -Type DWord

Write-Host "Agentty $version was installed to $Dest"
Write-Host 'Open it from the Start menu. Settings > System check lists (and installs) the tools it uses: Git for Windows, Node.js, Claude Code, Codex, PowerShell 7.'
if (-not $NoLaunch) { Start-Process -FilePath $Exe }
