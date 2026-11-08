<#
.SYNOPSIS
  Builds 3zT Mirroring into .\release\:  3zT-Mirroring-v<ver>-Setup.exe

  3zTMirroring.exe is the window (rust\). The Python service is built as
  MirroringService.exe; the window starts it in the background.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File packaging\build.ps1 -PlatformTools C:\Android\platform-tools
#>
param(
    # Folder with Google's platform-tools (adb.exe, AdbWinApi.dll,
    # AdbWinUsbApi.dll, NOTICE.txt).
    [Parameter(Mandatory = $true)][string]$PlatformTools,
    [string]$Python = "python"
)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$Version = (Select-String -Path rust\Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$pyVersion = (Select-String -Path pyproject.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$svcVersion = (Select-String -Path mirroring\__init__.py -Pattern '__version__\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
if ($Version -ne $pyVersion -or $Version -ne $svcVersion) { throw "Version mismatch: rust\Cargo.toml $Version, pyproject.toml $pyVersion, mirroring\__init__.py $svcVersion" }
$VersionNum = ($Version -split "-")[0]
Write-Host "3zT Mirroring $Version" -ForegroundColor Cyan

# adb: Google's official files, unmodified and validly signed by Google.
$adbFiles = "adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll", "NOTICE.txt"
foreach ($f in $adbFiles) {
    $p = Join-Path $PlatformTools $f
    if (-not (Test-Path $p)) { throw "Missing $p" }
    if ($f -ne "NOTICE.txt") {
        $sig = Get-AuthenticodeSignature $p
        if ($sig.Status -ne "Valid" -or $sig.SignerCertificate.Subject -notmatch "CN=Google LLC") {
            throw "$p is not validly signed by Google ($($sig.Status))"
        }
    }
}

$release = Join-Path $root "release"
$build = Join-Path $root "build"
$dist = Join-Path $root "dist"
Remove-Item -Recurse -Force $release, $build, $dist -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $release, $build | Out-Null

Write-Host "[1/4] Python packages" -ForegroundColor Cyan
& $Python -m pip install --disable-pip-version-check -q -r requirements.txt pyinstaller -c requirements.lock
if ($LASTEXITCODE) { throw "pip failed" }

Write-Host "[2/4] MirroringService.exe (PyInstaller)" -ForegroundColor Cyan
$parts = ($VersionNum.Split(".") + @("0", "0", "0", "0"))[0..3] -join ", "
@"
VSVersionInfo(
  ffi=FixedFileInfo(filevers=($parts), prodvers=($parts), mask=0x3f, flags=0x0, OS=0x40004,
                    fileType=0x1, subtype=0x0, date=(0, 0)),
  kids=[
    StringFileInfo([StringTable('040904B0', [
      StringStruct('CompanyName', '3zT'),
      StringStruct('FileDescription', '3zT Mirroring service'),
      StringStruct('FileVersion', '$Version'),
      StringStruct('InternalName', 'MirroringService'),
      StringStruct('LegalCopyright', 'Copyright (C) 2026 3zT and Zenvy Zenshiro. GPL-3.0-or-later.'),
      StringStruct('OriginalFilename', 'MirroringService.exe'),
      StringStruct('ProductName', '3zT Mirroring'),
      StringStruct('ProductVersion', '$Version')])]),
    VarFileInfo([VarStruct('Translation', [1033, 1200])])
  ]
)
"@ | Set-Content -Encoding UTF8 (Join-Path $build "version_info.txt")

$pyiArgs = @(
    "-m", "PyInstaller", "$root\packaging\launch.py",
    "--name", "MirroringService", "--onedir", "--windowed", "--noconfirm", "--clean",
    "--distpath", $dist, "--workpath", (Join-Path $build "pyi"), "--specpath", $build,
    # No UPX: packed executables trip antivirus heuristics.
    "--noupx",
    "--icon", (Join-Path $root "assets\app.ico"),
    "--contents-directory", ".",
    "--version-file", (Join-Path $build "version_info.txt"),
    "--add-data", "$root\assets;assets",
    "--add-data", "$root\tools;tools",
    "--add-data", "$root\drivers;drivers",
    "--collect-all", "pymobiledevice3",
    # Package metadata of pymobiledevice3 and its dependencies: some read
    # their own version at import (pyimg4 does).
    "--recursive-copy-metadata", "pymobiledevice3",
    "--collect-all", "pmd_pytcp",
    "--collect-all", "pmd_net_addr",
    "--collect-all", "pytun_pmd3",
    "--additional-hooks-dir", "$root\packaging\pyi-hooks",
    "--hidden-import", "_sounddevice_data",
    "--collect-all", "zeroconf",
    "--add-binary", "$root\mirroring\airplay\playfair.dll;mirroring\airplay",
    "--add-binary", "$root\mirroring\aacdec.dll;mirroring",
    "--exclude-module", "av",
    "--hidden-import", "Crypto.Cipher.AES",
    "--collect-submodules", "mirroring",
    "--exclude-module", "tkinter",
    "--exclude-module", "matplotlib",
    "--exclude-module", "IPython",
    "--exclude-module", "webview",
    "--exclude-module", "clr",
    "--exclude-module", "clr_loader",
    "--exclude-module", "pythonnet",
    "--exclude-module", "bottle",
    "--exclude-module", "proxy_tools",
    "--exclude-module", "pystray",
    "--exclude-module", "PIL",
    "--exclude-module", "chardet",
    "--exclude-module", "pydoc",
    "--exclude-module", "pydantic",
    "--exclude-module", "pydantic_core",
    "--exclude-module", "setuptools",
    "--exclude-module", "pkg_resources",
    "--exclude-module", "_distutils_hack",
    "--exclude-module", "PyInstaller",
    "--exclude-module", "pefile",
    "--exclude-module", "sqlite3",
    "--exclude-module", "_sqlite3",
    "--exclude-module", "lib2to3"
)
& $Python @pyiArgs
if ($LASTEXITCODE) { throw "PyInstaller failed" }
$app = Join-Path $dist "MirroringService"

# Import every device-path module inside the built exe; a missing module or
# package metadata fails the build here instead of on a user's PC.
$report = Join-Path $build "selftest.json"
$p = Start-Process -FilePath (Join-Path $app "MirroringService.exe") -ArgumentList "--selftest", "`"$report`"" -PassThru -Wait
if (-not (Test-Path $report)) { throw "Self-test wrote no report (exit $($p.ExitCode))" }
$r = Get-Content $report -Raw | ConvertFrom-Json
if (-not $r.ok) {
    $bad = $r.modules.PSObject.Properties | Where-Object { $_.Value -ne "ok" } | ForEach-Object { "$($_.Name): $($_.Value)" }
    throw "Self-test failed:`n$($bad -join "`n")"
}
Write-Host "    self-test OK ($(@($r.modules.PSObject.Properties).Count) modules)" -ForegroundColor Green
if (-not (Test-Path (Join-Path $app "_sounddevice_data\portaudio-binaries\libportaudio64bit.dll"))) { throw "PortAudio x64 missing" }

foreach ($f in "LICENSE", "README.md", "THIRD_PARTY_NOTICES.md") { Copy-Item $f $app }
$adbDest = Join-Path $app "third_party\platform-tools"
New-Item -ItemType Directory -Force $adbDest | Out-Null
foreach ($f in $adbFiles) { Copy-Item (Join-Path $PlatformTools $f) $adbDest }
$scrcpyDest = Join-Path $app "third_party\scrcpy"
New-Item -ItemType Directory -Force $scrcpyDest | Out-Null
Copy-Item "third_party\scrcpy\LICENSE" $scrcpyDest

Write-Host "[3/4] 3zTMirroring.exe (Rust)" -ForegroundColor Cyan
& cargo build --release --manifest-path rust\Cargo.toml -p mirror-app
if ($LASTEXITCODE) { throw "cargo build failed" }
Copy-Item "rust\target\release\Mirroring.exe" (Join-Path $app "3zTMirroring.exe")

Write-Host "[4/4] Setup" -ForegroundColor Cyan
$names = @(Get-ChildItem -Force $app | ForEach-Object { $_.Name }) + "install-manifest.txt"
[IO.File]::WriteAllLines((Join-Path $app "install-manifest.txt"), [string[]]$names)
$iscc = @(
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6.6+ not found" }
& $iscc "/DAppVersion=$Version" "/DAppVersionNum=$VersionNum" "/DSourceDir=$app" "/DOutputDir=$release" "packaging\installer.iss"
if ($LASTEXITCODE) { throw "Inno Setup failed" }

Get-ChildItem $release | ForEach-Object { "{0,-45} {1,8:N1} MB" -f $_.Name, ($_.Length / 1MB) }
