param(
    [Parameter(Mandatory = $true)]
    [string]$Target,
    [int]$SizeMB = 10
)

$ErrorActionPreference = "Stop"

Write-Host "=== Quiczilla Automated Transfer Benchmark ===" -ForegroundColor Cyan
Write-Host "Target: $Target"
Write-Host "File Size: $SizeMB MB"

# 1. Create temporary test file
$testFileName = "quiczilla_test_${SizeMB}MB_$([System.Guid]::NewGuid().ToString('N').Substring(0, 8)).bin"
$testFilePath = Join-Path $PSScriptRoot "..\$testFileName"

Write-Host "Generating $SizeMB MB random payload..."
$bytes = New-Object byte[] ($SizeMB * 1024 * 1024)
(New-Object System.Random).NextBytes($bytes)
[System.IO.File]::WriteAllBytes($testFilePath, $bytes)
$localHash = (Get-FileHash -Path $testFilePath -Algorithm SHA256).Hash.ToLower()
Write-Host "Local SHA256: $localHash" -ForegroundColor DarkGray

try {
    # 2. Run CLI transfer
    Write-Host "`nExecuting transfer..." -ForegroundColor Yellow
    $cliExe = Join-Path $PSScriptRoot "..\target\release\quiczilla-cli.exe"
    if (-not (Test-Path $cliExe)) {
        Write-Host "Building quiczilla-cli in release mode..."
        cargo build -p quiczilla-cli --release
    }

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $prevEAP = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $output = & $cliExe $testFilePath $Target 2>&1
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $prevEAP
    $sw.Stop()

    $output | ForEach-Object { Write-Host $_ }

    if ($exitCode -ne 0) {
        Write-Error "quiczilla-cli failed with exit code $exitCode"
        exit $exitCode
    }

    # 3. Extract metrics from CLI output
    $bootstrapDuration = ($output | Select-String "Bootstrap [Dd]uration: ([0-9\.]+)s" | ForEach-Object { $_.Matches.Groups[1].Value } | Select-Object -First 1)
    $handshakeDuration = ($output | Select-String "established in ([0-9\.]+)ms" | ForEach-Object { $_.Matches.Groups[1].Value } | Select-Object -First 1)
    $transferDuration = ($output | Select-String "Transfer Duration: ([0-9\.]+)s" | ForEach-Object { $_.Matches.Groups[1].Value } | Select-Object -First 1)
    $transferSpeed = ($output | Select-String "Average Transfer Speed: ([0-9\.]+) MB/s" | ForEach-Object { $_.Matches.Groups[1].Value } | Select-Object -First 1)

    Write-Host "`n--- Benchmark Metrics Summary ---" -ForegroundColor Green
    Write-Host "Bootstrap Duration : $bootstrapDuration s"
    Write-Host "QUIC Handshake     : $handshakeDuration ms"
    Write-Host "Transfer Duration  : $transferDuration s"
    Write-Host "Transfer Speed     : $transferSpeed MB/s"
    Write-Host "Total Run Duration : $($sw.Elapsed.TotalSeconds.ToString('F2')) s"

    # 4. Verify remote hash
    $sshHost = $Target.Split(':')[0]
    $remoteFolder = $Target.Split(':')[1]
    $remoteFile = "$remoteFolder/$testFileName".Replace('//', '/')

    Write-Host "`nVerifying remote checksum via SSH..."
    $remoteCheck = ssh -o BatchMode=yes $sshHost "sha256sum $remoteFile"
    $remoteHash = ($remoteCheck -split '\s+')[0].ToLower()

    if ($remoteHash -eq $localHash) {
        Write-Host "SUCCESS: Remote SHA256 matches local checksum!" -ForegroundColor Green
        Write-Host "Checksum: $remoteHash"
    } else {
        Write-Error "FAILURE: Checksum mismatch! Local: $localHash vs Remote: $remoteHash"
    }
}
finally {
    # Cleanup local file
    if (Test-Path $testFilePath) {
        Remove-Item -Force $testFilePath
    }
}
