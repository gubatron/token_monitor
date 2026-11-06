TokenMon for Windows (portable)
==============================

TokenMon shows your AI usage (Grok, Cursor, Claude, ChatGPT, OpenCode,
OpenRouter and more) in the Windows system tray.

Run
---
Put this folder anywhere you can write to (for example
%LOCALAPPDATA%\Programs\TokenMon) and run TokenMon.exe. Windows 10 or 11,
x64. No installation or administrator rights are needed.

While builds are unsigned, SmartScreen may say "Windows protected your PC".
Choose "More info", then "Run anyway". Check the download first: its
SHA-256 must match the tokenmon-windows-<version>-SHA256SUMS.txt file on the
release page (PowerShell: Get-FileHash .\tokenmon-windows-<version>-x64.zip).

Your data
---------
Settings, usage history and the encrypted sign-in vault live in
%LOCALAPPDATA%\TokenMon, not in this folder. Replacing or deleting this
folder keeps them.

Updates
-------
TokenMon checks the GitHub releases tagged windows-vX.Y.Z and can replace
this TokenMon.exe in place when the folder is writable.

Remove
------
In Settings, turn off "Launch at login" and "Show TokenMon in the desktop
right-click menu". Quit TokenMon from the tray menu, then delete this
folder. Delete %LOCALAPPDATA%\TokenMon as well to remove your data.

The MSI installer (tokenmon-windows-<version>-x64.msi) installs the same
program per user, adds a Start-menu entry and cleans up its registry
entries when uninstalled.

Source and issues: https://github.com/gubatron/token_monitor
License: MIT (LICENSE.txt)
