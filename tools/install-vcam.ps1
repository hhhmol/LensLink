#Requires -RunAsAdministrator
<#
Installs or removes the LensLink virtual camera (the media source DLL).

Why administrator: Windows loads the camera DLL inside the Frame Server service, which reads COM
registrations only from HKLM. The DLL is copied to Program Files so that only administrators can
replace a file that a system service loads.

    .\install-vcam.ps1               # install or update from pc\target\release
    .\install-vcam.ps1 -Uninstall    # remove
#>
param(
    [string]$Dll = (Join-Path $PSScriptRoot '..\pc\target\release\lenslink_vcam.dll'),
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'
$dir = Join-Path $env:ProgramFiles 'LensLink'
$target = Join-Path $dir 'lenslink_vcam.dll'
$logs = Join-Path $env:ProgramData 'LensLink\logs'

# The Frame Server keeps the DLL loaded while apps use cameras. Stopping it releases the file;
# Windows starts it again the next time an app opens a camera.
foreach ($svc in 'FrameServerMonitor', 'FrameServer') {
    Stop-Service $svc -Force -ErrorAction SilentlyContinue
}

if (Test-Path $target) {
    Start-Process regsvr32 -ArgumentList '/s', '/u', "`"$target`"" -Wait
}
if ($Uninstall) {
    Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host 'LensLink virtual camera removed.'
    return
}

if (-not (Test-Path $Dll)) { throw "Build it first: cd pc; cargo build --release  (missing $Dll)" }
New-Item -ItemType Directory -Force $dir, $logs | Out-Null
Copy-Item $Dll $target -Force
# LOCAL SERVICE (the Frame Server account) writes the log; users may read it.
icacls $logs /grant '*S-1-5-19:(OI)(CI)M' '*S-1-5-32-545:(OI)(CI)RX' | Out-Null

$p = Start-Process regsvr32 -ArgumentList '/s', "`"$target`"" -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "regsvr32 failed with code $($p.ExitCode)" }
Write-Host "LensLink virtual camera installed: $target"
