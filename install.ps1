# Quiczilla Windows Installer
# Usage:
#   Remote: irm https://raw.githubusercontent.com/cmdPromptCritical/quiczilla/master/install.ps1 | iex
#   Local:  powershell -ExecutionPolicy Bypass -File .\install.ps1

$ErrorActionPreference = "Stop"

$AppName = "quiczilla"
$Repo = if ($env:QUICZILLA_REPO) { $env:QUICZILLA_REPO } else { "cmdPromptCritical/quiczilla" }
$Version = if ($env:QUICZILLA_VERSION) { $env:QUICZILLA_VERSION } else { "latest" }
$InstallDir = if ($env:QUICZILLA_INSTALL_DIR) { 
    $env:QUICZILLA_INSTALL_DIR 
} else { 
    Join-Path $env:LOCALAPPDATA "Programs\$AppName" 
}
$TargetExe = Join-Path $InstallDir "$AppName.exe"
$TargetWorker = Join-Path $InstallDir "quiczilla-worker.exe"
$TargetMsQuic = Join-Path $InstallDir "msquic.dll"

Write-Host ""
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "   Quiczilla CLI - Fast Encrypted P2P File Transfer" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host ""

# 1. Architecture Check
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne "AMD64") {
    Write-Error "Unsupported Windows architecture: $arch. This release supports AMD64 (x64)."
    exit 1
}
Write-Host "[1/4] Detected Architecture: $arch" -ForegroundColor Green

# 2. Ensure Install Directory Exists
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
}
Write-Host "[2/4] Target Directory: $InstallDir" -ForegroundColor Green

# 3. Obtain Binary (Local build or Download)
$localBin = $null
if ($PSScriptRoot) {
    $localBin = Join-Path $PSScriptRoot "target\release\quiczilla-cli.exe"
}
if ($localBin -and (Test-Path $localBin)) {
    Write-Host "[3/4] Installing from local repository build..." -ForegroundColor Yellow
    Copy-Item -Force -Path $localBin -Destination $TargetExe
} else {
    Write-Host "[3/4] Downloading Quiczilla CLI ($Version)..." -ForegroundColor Yellow
    $assetName = "quiczilla-windows-x86_64.zip"
    if ($Version -eq "latest") {
        $downloadUrl = "https://github.com/$Repo/releases/latest/download/$assetName"
    } else {
        $downloadUrl = "https://github.com/$Repo/releases/download/$Version/$assetName"
    }

    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13
        $archivePath = Join-Path ([System.IO.Path]::GetTempPath()) "quiczilla-windows-x86_64.zip"
        $extractPath = Join-Path ([System.IO.Path]::GetTempPath()) ("quiczilla-" + [guid]::NewGuid())
        $downloadHeaders = @{ "User-Agent" = "quiczilla-installer" }
        if ($env:QUICZILLA_GITHUB_TOKEN) {
            $apiHeaders = @{
                "Authorization" = "Bearer $($env:QUICZILLA_GITHUB_TOKEN)"
                "Accept" = "application/vnd.github+json"
                "User-Agent" = "quiczilla-installer"
            }
            $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers $apiHeaders
            $asset = $release.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1
            if (-not $asset) { throw "Release asset '$assetName' was not found" }
            $downloadHeaders["Authorization"] = "Bearer $($env:QUICZILLA_GITHUB_TOKEN)"
            $downloadHeaders["Accept"] = "application/octet-stream"
            Invoke-WebRequest -Uri $asset.url -OutFile $archivePath -UseBasicParsing -Headers $downloadHeaders
        } else {
            Invoke-WebRequest -Uri $downloadUrl -OutFile $archivePath -UseBasicParsing -Headers $downloadHeaders
        }
        Expand-Archive -Path $archivePath -DestinationPath $extractPath -Force
        Copy-Item -Force -Path (Join-Path $extractPath "quiczilla.exe") -Destination $TargetExe
        Copy-Item -Force -Path (Join-Path $extractPath "quiczilla-worker.exe") -Destination $TargetWorker
        Copy-Item -Force -Path (Join-Path $extractPath "msquic.dll") -Destination $TargetMsQuic
        Remove-Item -Force $archivePath
        Remove-Item -Recurse -Force $extractPath
    } catch {
        # If GitHub release is not found, check if local cargo can build it
        if ($PSScriptRoot -and (Get-Command cargo -ErrorAction SilentlyContinue)) {
            Write-Host "Release asset not available online; compiling locally via cargo..." -ForegroundColor Yellow
            cargo build -p quiczilla-cli --release
            Copy-Item -Force -Path $localBin -Destination $TargetExe
        } else {
            Write-Error "Failed to download Quiczilla from $($downloadUrl): $_"
            exit 1
        }
    }
}

# 4. Create 'quic' alias
$AliasExe = Join-Path $InstallDir "quic.exe"
Write-Host "  -> Creating 'quic' shorthand alias..." -ForegroundColor Cyan
Copy-Item -Force -Path $TargetExe -Destination $AliasExe

# 5. Configure User PATH
Write-Host "[4/4] Configuring Environment PATH..." -ForegroundColor Green
$userPath = [Environment]::GetEnvironmentVariable("Path", [EnvironmentVariableTarget]::User)
$pathEntries = $userPath -split ';' | Where-Object { $_ -ne "" }

if ($pathEntries -notcontains $InstallDir) {
    $newUserPath = ($pathEntries + $InstallDir) -join ';'
    [Environment]::SetEnvironmentVariable("Path", $newUserPath, [EnvironmentVariableTarget]::User)
    Write-Host "  -> Added '$InstallDir' to User PATH." -ForegroundColor Cyan
} else {
    Write-Host "  -> '$InstallDir' is already in User PATH." -ForegroundColor DarkGray
}

# Update current session PATH so it works immediately
if (($env:Path -split ';') -notcontains $InstallDir) {
    $env:Path = "$env:Path;$InstallDir"
}

Write-Host ""
Write-Host "============================================================" -ForegroundColor Green
Write-Host " Quiczilla CLI installed successfully!" -ForegroundColor Green
Write-Host " Installed Location: $TargetExe" -ForegroundColor Green
Write-Host " Command Aliases:    quiczilla, quic" -ForegroundColor Green
Write-Host ""
Write-Host " Example usage:" -ForegroundColor White
Write-Host "   quic sample.bin user@remote:/destination/folder/" -ForegroundColor Yellow
Write-Host "   quic sample.bin user@remote:/destination/folder/ --checksum" -ForegroundColor Yellow
Write-Host "   quic sample.bin user@remote:/destination/folder/ --no-progress" -ForegroundColor Yellow
Write-Host "   quic sample.bin user@remote:/destination/folder/ -p 22322" -ForegroundColor Yellow
Write-Host "   quic pipe user@remote `"zfs receive backup/dataset`"" -ForegroundColor Yellow
Write-Host "   quic ... --verbose   # show SSH/bootstrap diagnostics" -ForegroundColor Yellow
Write-Host ""
Write-Host " Tip: Restart your terminal if 'quic' or '$AppName' is not recognized in other windows." -ForegroundColor DarkGray
Write-Host "============================================================" -ForegroundColor Green
Write-Host ""
