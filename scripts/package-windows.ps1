<#
.SYNOPSIS
  Build and package Drafft.ink for Windows x64.

.DESCRIPTION
  Writes into dist-release (override with -Dist):
    drafftink-<version>-windows-x64.zip            drafftink.exe (portable; `drafftink mcp` is its MCP bridge)
    drafftink-<version>-windows-x64.msi            installer (WiX 5; skipped with -NoMsi or when `wix` is missing)
    drafftink-headless-<version>-windows-x64.zip   drafftink-headless.exe + web-dist\ (run `drafftink-headless serve` from that folder)
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
$Bin = Join-Path $TargetDir "dist"
if ($env:BIN_DIR) { $Bin = $env:BIN_DIR }

Write-Host "==> Drafft.ink $Version for Windows x64"
if (-not $SkipBuild) {
  cargo build --profile dist --locked -p drafftink -p drafftink-headless --features drafftink-headless/sqlite,drafftink-headless/s3,drafftink-headless/hand-fonts,drafftink/hand-fonts
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}
foreach ($exe in "drafftink", "drafftink-headless") {
  if (-not (Test-Path (Join-Path $Bin "$exe.exe"))) { throw "$Bin\$exe.exe not found" }
}

$Work = Join-Path $TargetDir "windows-package"
if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
New-Item -ItemType Directory -Force $Work | Out-Null

# ---- portable zip --------------------------------------------------------------------------------
$Name = "drafftink-$Version-windows-x64"
$Portable = Join-Path $Work $Name
New-Item -ItemType Directory -Force $Portable | Out-Null
Copy-Item (Join-Path $Bin "drafftink.exe") $Portable
foreach ($f in "README.md", "LICENSE") { if (Test-Path $f) { Copy-Item $f $Portable } }
$Zip = Join-Path $Dist "$Name.zip"
if (Test-Path $Zip) { Remove-Item $Zip }
Compress-Archive -Path $Portable -DestinationPath $Zip
Write-Host "wrote $Zip"

# ---- headless zip (needs the web build) ----------------------------------------------------------
if (Test-Path (Join-Path $WebDist "index.html")) {
  $HlName = "drafftink-headless-$Version-windows-x64"
  $Hl = Join-Path $Work $HlName
  New-Item -ItemType Directory -Force $Hl | Out-Null
  Copy-Item (Join-Path $Bin "drafftink-headless.exe") $Hl
  # The default --web-dir of `serve` is .\web-dist, so running it from this folder serves the web app.
  Copy-Item -Recurse $WebDist (Join-Path $Hl "web-dist")
  foreach ($f in "README.md", "LICENSE") { if (Test-Path $f) { Copy-Item $f $Hl } }
  $HlZip = Join-Path $Dist "$HlName.zip"
  if (Test-Path $HlZip) { Remove-Item $HlZip }
  Compress-Archive -Path $Hl -DestinationPath $HlZip
  Write-Host "wrote $HlZip"
} else {
  Write-Warning "no web build in $WebDist (run scripts/build-web.sh), so the headless zip was not written"
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
foreach ($exe in "drafftink", "drafftink-headless") {
  $out = & (Join-Path $Bin "$exe.exe") --version 2>&1 | Out-String
  if ($LASTEXITCODE -ne 0 -or -not $out.Contains($Version)) { throw "$exe.exe --version printed '$out', expected $Version" }
  Write-Host "smoke: $($out.Trim())"
}
Write-Host "==> done"
Get-ChildItem $Dist | Format-Table Name, Length
