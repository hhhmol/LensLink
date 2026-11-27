<#
Builds a test release into dist\: LensLink-<version>-windows-x64.zip (the app, the virtual camera
and its installer, adb) and LensLink-<version>.apk (debug-signed).

    .\tools\make-release.ps1 -Version 0.1.0
#>
param([string]$Version = '0.1.0')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot

# The build tools report progress on stderr, which Windows PowerShell would take for errors.
$ErrorActionPreference = 'Continue'
Push-Location (Join-Path $root 'pc')
cargo build --release 2>&1 | Out-Host
if ($LASTEXITCODE) { throw 'cargo build failed' }
Pop-Location
Push-Location (Join-Path $root 'android')
.\gradlew assembleDebug -q 2>&1 | Out-Host
if ($LASTEXITCODE) { throw 'gradle build failed' }
Pop-Location
$ErrorActionPreference = 'Stop'

$sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME } else { Join-Path $env:LOCALAPPDATA 'Android\Sdk' }
$tools = Join-Path $sdk 'platform-tools'
$out = Join-Path $root 'dist'
$name = "LensLink-$Version-windows-x64"
$dir = Join-Path $out $name
Remove-Item $dir, "$dir.zip" -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $dir | Out-Null

$release = Join-Path $root 'pc\target\release'
Copy-Item (Join-Path $release 'LensLink.exe'), (Join-Path $release 'lenslink_vcam.dll') $dir
Copy-Item (Join-Path $root 'tools\install-vcam.ps1'), (Join-Path $root 'tools\release\*'), (Join-Path $root 'LICENSE') $dir
Copy-Item (Join-Path $tools 'adb.exe'), (Join-Path $tools 'AdbWinApi.dll'), (Join-Path $tools 'AdbWinUsbApi.dll') $dir
Copy-Item (Join-Path $tools 'NOTICE.txt') (Join-Path $dir 'NOTICE-platform-tools.txt')

Compress-Archive -Path $dir -DestinationPath "$dir.zip" -Force
Copy-Item (Join-Path $root 'android\app\build\outputs\apk\debug\app-debug.apk') (Join-Path $out "LensLink-$Version.apk") -Force
Get-ChildItem $out -File | Select-Object Name, @{ n = 'MB'; e = { [math]::Round($_.Length / 1MB, 1) } }
