Agentty for Windows
===================

Install
-------
1. Double-click install.cmd (or run: powershell -ExecutionPolicy Bypass -File .\install.ps1).
   It installs for your user only - no administrator rights - into
   %LOCALAPPDATA%\Programs\Agentty and adds a Start menu entry, agentty:// links,
   "Open in Agentty" on folders, and `agentty` on PATH. Options: -DesktopShortcut, -NoLaunch.
2. Uninstall from Settings > Apps > Agentty (your settings in %USERPROFILE%\.agentty stay).
   Running agentty.exe straight from this folder works too (without links and shortcuts).

Requirements
------------
Windows 10 version 1809 or later (Windows 11 recommended). On first launch Agentty checks
the tools it uses and offers one-click installs (winget) in Settings > System check:

  Git for Windows   required  Claude Code runs its hooks with Git Bash; without it pane
                              status (working / waiting / done) isn't shown.
  Claude Code       recommended   irm https://claude.ai/install.ps1 | iex
  Node.js LTS       recommended   winget install OpenJS.NodeJS.LTS  (for Codex and plugins)
  Codex CLI         optional      npm install -g @openai/codex
  PowerShell 7      recommended   winget install Microsoft.PowerShell  (faster, correct quoting)

Tools installed while Agentty runs are found without restarting it.

Signing in
----------
If `claude` / `codex login` can't sign in on this machine, use Settings > Accounts: API key,
gateway token, `claude setup-token` token, Amazon Bedrock, Google Vertex AI, or a Codex
auth.json. Keys are stored in Windows Credential Manager.

Shortcuts
---------
macOS shortcuts map to Ctrl+Shift (Cmd), Ctrl+Alt+Shift (Shift+Cmd), Ctrl+Alt (Option+Cmd)
and Alt+1..9, so Ctrl+letter always reaches the shell. Copy / paste in terminals:
Ctrl+Shift+C / Ctrl+Shift+V. Links: Ctrl+click.

Not available on Windows yet: the in-app browser (links open in your default browser), the
menu bar item and mini mode.
