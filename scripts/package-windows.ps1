<#
.SYNOPSIS
  Build and package Drafft.ink for Windows x64.

.DESCRIPTION
  Writes into dist-release (override with -Dist):
    drafftink-<version>-windows-x64.zip            drafftink.exe + drafftink-mcp.exe (portable)
    drafftink-<version>-windows-x64.msi            installer (WiX 5; skipped with -NoMsi or when `wix` is missing)
    drafftink-server-<version>-windows-x64.zip     drafftink-server.exe + web-dist\ (run it from that folder)
  The script ends by running every packaged exe with --version.

  Needs: Rust (MSVC toolchain), PowerShell 7. For the MSI: dotnet tool install --global wix --version 5.0.2
  The exes are unsigned (docs/packaging.md, "Signing").

.EXAMPLE
  pwsh scripts/package-windows.ps1 -SkipBuild -WebDist dist
#>
param(
  [switch]$SkipBuild,
  [switch]$NoMsi,
  [string]$Dist = "",
  [string]$WebDist = ""
)
$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $Root

$Version = $env:DRAFFTINK_VERSION
if (-not $Version) {
  $inPkg = $false
  foreach ($line in Get-Content "Cargo.toml") {
    if ($line -match '^\[') { $inPkg = ($line.Trim() -eq "[workspace.package]"); continue }
    if ($inPkg -and $line -match '^version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
  }
}
if (-not $Version) { throw "could not read [workspace.package] version from Cargo.toml" }
if (-not $Dist) { $Dist = Join-Path $Root "dist-release" }
if (-not $WebDist) { $WebDist = Join-Path $Root "dist" }
New-Item -ItemType Directory -Force $Dist | Out-Null
$Dist = (Resolve-Path $Dist).Path
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root "target" }
$Bin = Join-Path $TargetDir "release"
if ($env:BIN_DIR) { $Bin = $env:BIN_DIR }

Write-Host "==> Drafft.ink $Version for Windows x64"
if (-not $SkipBuild) {
  cargo build --release --locked -p drafftink -p drafftink-mcp -p drafftink-server
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}
foreach ($exe in "drafftink", "drafftink-mcp", "drafftink-server") {
  if (-not (Test-Path (Join-Path $Bin "$exe.exe"))) { throw "$Bin\$exe.exe not found" }
}

$Work = Join-Path $TargetDir "windows-package"
if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
New-Item -ItemType Directory -Force $Work | Out-Null

# ---- portable zip --------------------------------------------------------------------------------
$Name = "drafftink-$Version-windows-x64"
$Portable = Join-Path $Work $Name
New-Item -ItemType Directory -Force $Portable | Out-Null
foreach ($exe in "drafftink", "drafftink-mcp") { Copy-Item (Join-Path $Bin "$exe.exe") $Portable }
foreach ($f in "README.md", "LICENSE") { if (Test-Path $f) { Copy-Item $f $Portable } }
$Zip = Join-Path $Dist "$Name.zip"
if (Test-Path $Zip) { Remove-Item $Zip }
Compress-Archive -Path $Portable -DestinationPath $Zip
Write-Host "wrote $Zip"

# ---- server zip (needs the web build) ------------------------------------------------------------
if (Test-Path (Join-Path $WebDist "index.html")) {
  $SrvName = "drafftink-server-$Version-windows-x64"
  $Srv = Join-Path $Work $SrvName
  New-Item -ItemType Directory -Force $Srv | Out-Null
  Copy-Item (Join-Path $Bin "drafftink-server.exe") $Srv
  # The server's default --web-dir is .\web-dist, so running it from this folder serves the web app.
  Copy-Item -Recurse $WebDist (Join-Path $Srv "web-dist")
  foreach ($f in "README.md", "LICENSE") { if (Test-Path $f) { Copy-Item $f $Srv } }
  $SrvZip = Join-Path $Dist "$SrvName.zip"
  if (Test-Path $SrvZip) { Remove-Item $SrvZip }
  Compress-Archive -Path $Srv -DestinationPath $SrvZip
  Write-Host "wrote $SrvZip"
} else {
  Write-Warning "no web build in $WebDist (run scripts/build-web.sh), so the server zip was not written"
}

# ---- MSI -----------------------------------------------------------------------------------------
if (-not $NoMsi) {
  if (Get-Command wix -ErrorAction SilentlyContinue) {
    # WiX wants a numeric ProductVersion: drop any pre-release suffix.
    $Numeric = ($Version -split '[-+]')[0]
    $Msi = Join-Path $Dist "$Name.msi"
    wix build (Join-Path $Root "packaging/windows/drafftink.wxs") `
      -arch x64 -o $Msi `
      -d "Version=$Numeric" -d "BinDir=$Bin" -d "IconFile=$(Join-Path $Root 'packaging/icons/drafftink.ico')"
    if ($LASTEXITCODE -ne 0) { throw "wix build failed" }
    Write-Host "wrote $Msi"
  } else {
    Write-Warning "wix not found (dotnet tool install --global wix --version 5.0.2), so the MSI was not written"
  }
}

# ---- smoke ---------------------------------------------------------------------------------------
foreach ($exe in "drafftink", "drafftink-mcp", "drafftink-server") {
  $out = & (Join-Path $Bin "$exe.exe") --version 2>&1 | Out-String
  if ($LASTEXITCODE -ne 0 -or -not $out.Contains($Version)) { throw "$exe.exe --version printed '$out', expected $Version" }
  Write-Host "smoke: $($out.Trim())"
}
Write-Host "==> done"
Get-ChildItem $Dist | Format-Table Name, Length
