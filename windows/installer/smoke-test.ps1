<#
.SYNOPSIS
  Install, upgrade, downgrade and uninstall the TokenMon MSI on a clean
  Windows machine (the GitHub windows-latest runner) and check every promise
  main.wxs makes.

.DESCRIPTION
  Run by .github/workflows/windows-release.yml. Never run it on your own
  machine: it installs and removes TokenMon for the current user and deletes
  the app's HKCU registrations.

  1. Install the release MSI (per user, no elevation): files, Start-menu
     shortcut with System.AppUserModel.ID "TokenMon", ARP entry.
  2. Simulate what the app writes at run time (Run value, StartupApproved
     value, desktop verb, AUMID key, user data, updater leftovers).
  3. Install the upgrade-probe MSI (same package, higher version): a major
     upgrade that must keep those registrations and leave one ARP entry.
  4. Install the release MSI again: a downgrade, which must be refused.
  5. Uninstall: files, shortcut, ARP entry and the app's registrations are
     gone; %LOCALAPPDATA%\TokenMon (user data) is untouched.

  Writes a result table to $env:GITHUB_STEP_SUMMARY and exits 1 if any
  check fails.
#>
param(
  [Parameter(Mandatory)] [string] $Msi,
  [Parameter(Mandatory)] [string] $UpgradeMsi,
  [Parameter(Mandatory)] [string] $Version,
  [Parameter(Mandatory)] [string] $UpgradeVersion,
  [string] $LogDir = $env:RUNNER_TEMP
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\TokenMon'
$Exe = Join-Path $InstallDir 'TokenMon.exe'
$StartMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$Lnk = Join-Path $StartMenu 'TokenMon.lnk'
$DataDir = Join-Path $env:LOCALAPPDATA 'TokenMon'
$DataFile = Join-Path $DataDir 'smoke-test-user-data.json'
$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$ApprovedKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'
$VerbKey = 'HKCU:\Software\Classes\DesktopBackground\Shell\TokenMon'
$AumidKey = 'HKCU:\Software\Classes\AppUserModelId\TokenMon'
$InstallerKey = 'HKCU:\Software\TokenMon\Installer'
$UpgradeCode = '{97EEF0DA-B00F-4527-93D7-EF335558A346}'

$results = New-Object System.Collections.Generic.List[object]
function Check([string] $Name, [bool] $Ok, [string] $Detail = '') {
  $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
  $mark = if ($Ok) { 'PASS' } else { 'FAIL' }
  Write-Host "[$mark] $Name $Detail"
}

function Invoke-Msi([string[]] $MsiArgs, [string] $Log) {
  $all = $MsiArgs + @('/qn', '/norestart', '/l*v', (Join-Path $LogDir $Log))
  $p = Start-Process -FilePath 'msiexec.exe' -ArgumentList $all -Wait -PassThru
  return $p.ExitCode
}

function Get-ValueOrNull([string] $Key, [string] $Name) {
  $item = Get-ItemProperty -Path $Key -Name $Name -ErrorAction SilentlyContinue
  if ($null -eq $item) { return $null }
  return $item.$Name
}

function Get-ArpEntries {
  Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' -ErrorAction SilentlyContinue |
    ForEach-Object { Get-ItemProperty $_.PSPath } |
    Where-Object { $_.PSObject.Properties['DisplayName'] -and $_.DisplayName -eq 'TokenMon' }
}

function Get-RelatedProducts {
  $installer = New-Object -ComObject WindowsInstaller.Installer
  $products = $installer.GetType().InvokeMember('RelatedProducts', 'GetProperty', $null, $installer, @($UpgradeCode))
  return @($products | ForEach-Object { $_ })
}

function Get-Aumid([string] $Path) {
  $shell = New-Object -ComObject Shell.Application
  $folder = $shell.Namespace((Split-Path $Path))
  $item = $folder.ParseName((Split-Path $Path -Leaf))
  return $item.ExtendedProperty('System.AppUserModel.ID')
}

# Clean slate (the runner is fresh, but be explicit).
foreach ($k in @($VerbKey, $AumidKey, $InstallerKey)) { Remove-Item $k -Recurse -ErrorAction SilentlyContinue }
foreach ($k in @($RunKey, $ApprovedKey)) { Remove-ItemProperty $k -Name TokenMon -ErrorAction SilentlyContinue }
New-Item -ItemType Directory -Force $DataDir | Out-Null
Set-Content -Path $DataFile -Value '{"kept": true}'

# 1. Install.
$code = Invoke-Msi @('/i', $Msi) 'install.log'
Check 'install exits 0' ($code -eq 0) "exit $code"
Check 'TokenMon.exe in %LOCALAPPDATA%\Programs\TokenMon' (Test-Path $Exe)
Check 'LICENSE.txt installed' (Test-Path (Join-Path $InstallDir 'LICENSE.txt'))
Check 'Start-menu shortcut exists' (Test-Path $Lnk)
if (Test-Path $Lnk) {
  $target = (New-Object -ComObject WScript.Shell).CreateShortcut($Lnk).TargetPath
  Check 'shortcut targets TokenMon.exe' ($target -ieq $Exe) $target
  $aumid = Get-Aumid $Lnk
  Check 'shortcut System.AppUserModel.ID is TokenMon' ($aumid -ceq 'TokenMon') "'$aumid'"
}
$arp = @(Get-ArpEntries)
Check 'one per-user ARP entry' ($arp.Count -eq 1) "count $($arp.Count)"
if ($arp.Count -ge 1) {
  Check "ARP version is $Version" ($arp[0].DisplayVersion -eq $Version) $arp[0].DisplayVersion
  Check 'ARP install location' ($arp[0].InstallLocation -ieq "$InstallDir\") $arp[0].InstallLocation
}
Check 'nothing installed per machine' (-not (Test-Path (Join-Path $env:ProgramFiles 'TokenMon')))
$related = @(Get-RelatedProducts)
Check 'installed under the recorded UpgradeCode' ($related.Count -eq 1) "$UpgradeCode -> $($related -join ', ')"

# 2. What the app writes at run time.
Set-ItemProperty -Path $RunKey -Name TokenMon -Value "`"$Exe`" --background"
New-Item -Force $ApprovedKey | Out-Null
New-ItemProperty -Force -Path $ApprovedKey -Name TokenMon -PropertyType Binary -Value ([byte[]](2,0,0,0,0,0,0,0,0,0,0,0)) | Out-Null
New-Item -Force "$VerbKey\command" | Out-Null
Set-ItemProperty -Path "$VerbKey\command" -Name '(default)' -Value "`"$Exe`" --show"
New-Item -Force $AumidKey | Out-Null
Set-ItemProperty -Path $AumidKey -Name DisplayName -Value 'TokenMon'
Set-Content -Path (Join-Path $InstallDir 'TokenMon.old-1a2b.exe') -Value 'aside'
Set-Content -Path (Join-Path $InstallDir '.TokenMon-update-3c4d.exe') -Value 'staged'

# 3. Major upgrade keeps the app's registrations.
$code = Invoke-Msi @('/i', $UpgradeMsi) 'upgrade.log'
Check 'upgrade exits 0' ($code -eq 0) "exit $code"
$arp = @(Get-ArpEntries)
Check 'still one ARP entry after upgrade' ($arp.Count -eq 1) "count $($arp.Count)"
if ($arp.Count -ge 1) {
  Check "ARP version is $UpgradeVersion" ($arp[0].DisplayVersion -eq $UpgradeVersion) $arp[0].DisplayVersion
}
Check 'TokenMon.exe present after upgrade' (Test-Path $Exe)
Check 'shortcut present after upgrade' (Test-Path $Lnk)
Check 'upgrade keeps the Run value' ($null -ne (Get-ValueOrNull $RunKey 'TokenMon'))
Check 'upgrade keeps the StartupApproved value' ($null -ne (Get-ValueOrNull $ApprovedKey 'TokenMon'))
Check 'upgrade keeps the desktop verb' (Test-Path $VerbKey)
Check 'upgrade keeps the AUMID registration' (Test-Path $AumidKey)

# 4. Downgrade is refused and changes nothing.
$code = Invoke-Msi @('/i', $Msi) 'downgrade.log'
Check 'downgrade is refused' ($code -ne 0) "exit $code"
$arp = @(Get-ArpEntries)
Check 'downgrade leaves the newer version' ($arp.Count -eq 1 -and $arp[0].DisplayVersion -eq $UpgradeVersion)

# 5. Uninstall.
$related = @(Get-RelatedProducts)
Check 'one related product before uninstall' ($related.Count -eq 1) ($related -join ', ')
$product = $related | Select-Object -First 1
$code = Invoke-Msi @('/x', $product) 'uninstall.log'
Check 'uninstall exits 0' ($code -eq 0) "exit $code ($product)"
Check 'TokenMon.exe removed' (-not (Test-Path $Exe))
Check 'install folder removed (updater leftovers too)' (-not (Test-Path $InstallDir))
Check 'shortcut removed' (-not (Test-Path $Lnk))
Check 'ARP entry removed' (@(Get-ArpEntries).Count -eq 0)
Check 'no product left under the UpgradeCode' (@(Get-RelatedProducts).Count -eq 0)
Check 'installer key removed' (-not (Test-Path $InstallerKey))
Check 'Run value removed' ($null -eq (Get-ValueOrNull $RunKey 'TokenMon'))
Check 'StartupApproved value removed' ($null -eq (Get-ValueOrNull $ApprovedKey 'TokenMon'))
Check 'desktop verb removed' (-not (Test-Path $VerbKey))
Check 'AUMID registration removed' (-not (Test-Path $AumidKey))
Check 'user data in %LOCALAPPDATA%\TokenMon kept' (Test-Path $DataFile)

# Report.
$failed = @($results | Where-Object { -not $_.Ok })
$lines = @('### MSI smoke test (windows-latest)', '',
  "Release MSI $Version, upgrade probe $UpgradeVersion. Logs are in the ``msi-logs`` artifact.", '',
  '| check | result | detail |', '|---|---|---|')
foreach ($r in $results) {
  $mark = if ($r.Ok) { ':white_check_mark:' } else { ':x:' }
  $lines += "| $($r.Name) | $mark | $($r.Detail -replace '\|', '\|') |"
}
if ($env:GITHUB_STEP_SUMMARY) { $lines | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Append -Encoding utf8 }
if ($failed.Count -gt 0) {
  Write-Host "$($failed.Count) check(s) failed."
  exit 1
}
Write-Host "All $($results.Count) checks passed."
