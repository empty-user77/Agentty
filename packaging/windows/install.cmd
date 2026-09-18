@echo off
rem Double-click to install Agentty for this user (see install.ps1 for options).
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
if errorlevel 1 pause
