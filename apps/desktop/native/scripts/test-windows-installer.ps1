# Install, inspect, and uninstall the packaged per-user installer
# (dist/windows/loams-desktop.iss) silently. Registers and removes the real per-user
# uninstall entry and loams:// handler, so it refuses to run outside CI unless
# -Force is given.
param(
    [string]$Setup,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
if (-not $env:CI -and -not $Force) {
    throw 'This test installs and uninstalls Loams Desktop for the current user; pass -Force to run it outside CI'
}
if (-not $Setup) {
    $Setup = Get-ChildItem (Join-Path $PSScriptRoot '../target/package') -Filter 'loams-desktop-*-windows-*-setup.exe' |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $Setup) { throw 'No loams-desktop-*-setup.exe under target/package' }
$Setup = (Resolve-Path -LiteralPath $Setup).Path
$match = [regex]::Match((Split-Path $Setup -Leaf), '\Aloams-desktop-(\d+\.\d+\.\d+)-windows-[a-z0-9_]+-setup\.exe\z')
if (-not $match.Success) { throw "Unexpected installer name: $Setup" }
$version = $match.Groups[1].Value
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{93DB7E9E-5B92-5E45-99A1-105C32A995B8}_is1'
$protocolKey = 'HKCU:\Software\Classes\loams'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Loams Desktop.lnk'
$root = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$dir = Join-Path $root "loams-desktop installer test $([guid]::NewGuid().ToString('N'))"

function Invoke-Checked([string]$File, [string[]]$Arguments, [string]$What) {
    $process = Start-Process -FilePath $File -ArgumentList $Arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "$What exited with $($process.ExitCode)" }
}

function Wait-Until([scriptblock]$Condition, [string]$What) {
    $deadline = (Get-Date).AddSeconds(90)
    while (-not (& $Condition)) {
        if ((Get-Date) -gt $deadline) { throw "Timed out waiting for $What" }
        Start-Sleep -Milliseconds 250
    }
}

$log = Join-Path $root 'loams-desktop-setup.log'
Invoke-Checked $Setup @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$dir`"", "/LOG=`"$log`"") 'Setup'
$exe = Join-Path $dir 'loams-desktop.exe'
foreach ($file in @('loams-desktop.exe', 'LICENSE', 'NOTICE', 'SCOPED_NOTICE.md', 'THIRD_PARTY_NOTICES.md', 'licenses/fonts', 'licenses/loams-apache-2.0.txt', 'unins000.exe')) {
    if (-not (Test-Path -LiteralPath (Join-Path $dir $file))) { throw "Installed file missing: $file" }
}
Write-Output 'PASS: installed files'

if (Test-Path -LiteralPath (Join-Path $dir 'loams-desktop-update.json')) {
    throw 'Automatic-update configuration must not be installed'
}
Write-Output 'PASS: manual-upgrade-only install'

$info = New-Object Diagnostics.ProcessStartInfo
$info.FileName = $exe
$info.Arguments = '--version'
$info.UseShellExecute = $false
$info.CreateNoWindow = $true
$info.RedirectStandardOutput = $true
$info.RedirectStandardError = $true
$process = [Diagnostics.Process]::Start($info)
try {
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $null = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(10000)) { $process.Kill(); throw 'Installed version probe timed out' }
    if ($process.ExitCode -ne 0 -or $stdout.Result.Trim() -ne "loams-desktop $version") {
        throw "Installed executable reports '$($stdout.Result.Trim())', expected 'loams-desktop $version'"
    }
} finally { $process.Dispose() }
Write-Output "PASS: installed executable is $version"

$entry = Get-ItemProperty -LiteralPath $uninstallKey
if ($entry.DisplayVersion -ne $version) { throw "DisplayVersion is '$($entry.DisplayVersion)', expected '$version'" }
if ($entry.DisplayName -ne 'Loams Desktop') { throw "DisplayName is '$($entry.DisplayName)'" }
$installed = [IO.Path]::GetFullPath($entry.InstallLocation).TrimEnd('\')
if ($installed -ne [IO.Path]::GetFullPath($dir).TrimEnd('\')) { throw "InstallLocation is '$installed'" }
$command = (Get-ItemProperty -LiteralPath "$protocolKey\shell\open\command").'(default)'
if ($command -ne "`"$exe`" `"%1`"") { throw "loams:// handler is '$command'" }
if (-not (Test-Path -LiteralPath $shortcut)) { throw "Start menu shortcut missing: $shortcut" }
Write-Output 'PASS: uninstall entry, loams:// handler, Start menu shortcut'

# Leftovers an in-app update can leave behind must go with the uninstall.
Set-Content -LiteralPath (Join-Path $dir 'loams-desktop.exe.old') -Value 'previous image'
New-Item -ItemType Directory -Path (Join-Path $dir '.loams-desktop-update-test') | Out-Null
Set-Content -LiteralPath (Join-Path $dir '.loams-desktop-update-test/loams-desktop.exe') -Value 'staged'

# The uninstaller re-launches itself from a temporary copy and returns early;
# wait for its effects rather than for the process.
Invoke-Checked (Join-Path $dir 'unins000.exe') @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') 'Uninstall'
Wait-Until { -not (Test-Path -LiteralPath $uninstallKey) } 'the uninstall entry to disappear'
Wait-Until { -not (Test-Path -LiteralPath $exe) } 'loams-desktop.exe to be removed'
foreach ($leftover in @('loams-desktop.exe.old', '.loams-desktop-update-test', 'loams-desktop-update.json', 'licenses')) {
    Wait-Until { -not (Test-Path -LiteralPath (Join-Path $dir $leftover)) } "$leftover to be removed"
}
if (Test-Path -LiteralPath $protocolKey) { throw 'loams:// handler survived uninstall' }
if (Test-Path -LiteralPath $shortcut) { throw 'Start menu shortcut survived uninstall' }
Write-Output 'PASS: uninstall removes the install, update leftovers, and registrations'
