# Installs the fdt CLI on Windows from GitHub Releases, checking its SHA256.
# Works in Windows PowerShell 5.1 and PowerShell 7+.
#
#   irm https://github.com/fundathyon/fdt-cli/releases/latest/download/install.ps1 | iex
#
# Variables (all optional):
#   $env:FDT_INSTALL_DIR      destination directory (default %LOCALAPPDATA%\fdt\bin; no admin needed)
#   $env:FDT_INSTALL_VERSION  version to install, e.g. v0.1.0 (default: the latest release)
#   $env:FDT_RELEASES_URL     where the releases live (default https://github.com/fundathyon/fdt-cli/releases)
#
#   $env:FDT_INSTALL_VERSION = "v0.1.0"; irm …/install.ps1 | iex

# A function, so `irm | iex` leaves no variables or preferences behind in the caller's session.
function Install-Fdt {
    $ErrorActionPreference = "Stop"
    $ProgressPreference = "SilentlyContinue" # the progress bar makes Invoke-WebRequest very slow on 5.1
    # Windows PowerShell 5.1 may default to TLS 1.0, which GitHub refuses.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $releases = if ($env:FDT_RELEASES_URL) { $env:FDT_RELEASES_URL.TrimEnd("/") } else { "https://github.com/fundathyon/fdt-cli/releases" }
    $dir = if ($env:FDT_INSTALL_DIR) { $env:FDT_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "fdt\bin" }
    $version = $env:FDT_INSTALL_VERSION

    # PROCESSOR_ARCHITEW6432 is set when a 32-bit PowerShell runs on a 64-bit Windows.
    $machine = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    switch ($machine) {
        "AMD64" { }
        # Windows 11 on ARM runs x64 binaries through its built-in emulation.
        "ARM64" { Write-Host "Windows ARM64: se instala la versión x64 (corre con la emulación de Windows)." }
        default { throw "fdt: no hay binario de fdt para Windows $machine; compílalo con: cargo install --git https://github.com/fundathyon/fdt-cli" }
    }
    $asset = "fdt-x86_64-pc-windows-msvc.zip"

    if ($version) {
        if (-not $version.StartsWith("v")) { $version = "v$version" }
        $base = "$releases/download/$version"
    } else {
        $base = "$releases/latest/download"
    }

    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("fdt-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $label = if ($version) { $version } else { "última versión" }
        Write-Host "Descargando $asset ($label)..."
        $archive = Join-Path $tmp $asset
        $sums = Join-Path $tmp "SHA256SUMS"
        try {
            Invoke-WebRequest "$base/$asset" -OutFile $archive -UseBasicParsing
            Invoke-WebRequest "$base/SHA256SUMS" -OutFile $sums -UseBasicParsing
        } catch {
            throw "fdt: no se pudo descargar $base/$asset ($($_.Exception.Message)); ¿existe esa versión?"
        }

        $expected = Get-Content $sums |
            ForEach-Object { $parts = $_ -split "\s+"; if ($parts.Count -ge 2 -and $parts[1].TrimStart("*") -eq $asset) { $parts[0] } } |
            Select-Object -First 1
        if (-not $expected) { throw "fdt: SHA256SUMS no trae la suma de $asset; no se instala." }
        $actual = (Get-FileHash $archive -Algorithm SHA256).Hash
        if ($expected.ToLower() -ne $actual.ToLower()) {
            throw "fdt: la suma SHA256 de $asset no coincide con SHA256SUMS; no se instala."
        }

        $unpacked = Join-Path $tmp "unpacked"
        Expand-Archive -Path $archive -DestinationPath $unpacked -Force
        $exe = Join-Path $unpacked "fdt.exe"
        if (-not (Test-Path $exe)) { throw "fdt: $asset no contiene fdt.exe." }

        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $target = Join-Path $dir "fdt.exe"
        try {
            Copy-Item -Force $exe $target
        } catch {
            throw "fdt: no se pudo escribir $target (¿hay un fdt abierto? ciérralo y reintenta): $($_.Exception.Message)"
        }
        $installed = & $target --version
        if ($LASTEXITCODE -ne 0) { throw "fdt: se instaló $target pero no arranca en esta máquina." }
        Write-Host "$installed instalado en $target"

        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $entries = if ($userPath) { $userPath -split ";" } else { @() }
        if (-not ($entries | Where-Object { $_.TrimEnd("\") -ieq $dir.TrimEnd("\") })) {
            $newPath = if ($userPath) { "$userPath;$dir" } else { $dir }
            [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
            $env:Path = "$env:Path;$dir"
            Write-Host "Agregué $dir a tu PATH de usuario; las terminales que ya estaban abiertas necesitan reabrirse."
        }
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}

Install-Fdt
