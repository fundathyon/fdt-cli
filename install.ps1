# Installs the latest fdt CLI on Windows from GitHub Releases, checking its SHA256.
#   irm https://github.com/fundathyon/fdt-cli/releases/latest/download/install.ps1 | iex
# $env:FDT_INSTALL_DIR overrides the destination (default %LOCALAPPDATA%\fdt\bin).
$ErrorActionPreference = "Stop"

$repo = "fundathyon/fdt-cli"
$dir = if ($env:FDT_INSTALL_DIR) { $env:FDT_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "fdt\bin" }
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "aarch64" } else { "x86_64" }
$asset = "fdt-$arch-pc-windows-msvc.exe"
$base = "https://github.com/$repo/releases/latest/download"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("fdt-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "Descargando $asset..."
    Invoke-WebRequest "$base/$asset" -OutFile (Join-Path $tmp "fdt.exe") -UseBasicParsing
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile (Join-Path $tmp "SHA256SUMS") -UseBasicParsing

    $expected = (Get-Content (Join-Path $tmp "SHA256SUMS") |
        Where-Object { ($_ -split "\s+")[1].TrimStart("*") -eq $asset } |
        ForEach-Object { ($_ -split "\s+")[0] }) | Select-Object -First 1
    $actual = (Get-FileHash (Join-Path $tmp "fdt.exe") -Algorithm SHA256).Hash
    if (-not $expected -or $expected.ToLower() -ne $actual.ToLower()) {
        throw "fdt: la suma SHA256 no coincide con SHA256SUMS; no se instala."
    }

    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Move-Item -Force (Join-Path $tmp "fdt.exe") (Join-Path $dir "fdt.exe")
    Write-Host "fdt instalado en $dir\fdt.exe: $(& (Join-Path $dir 'fdt.exe') --version)"

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not ($userPath -split ";" | Where-Object { $_ -eq $dir })) {
        [Environment]::SetEnvironmentVariable("Path", "$userPath;$dir", "User")
        Write-Host "Agregué $dir a tu PATH de usuario; abre una terminal nueva."
    }
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
