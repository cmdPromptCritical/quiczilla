param(
    [string]$CliPath = "target/debug/quiczilla-cli.exe",
    [string]$WorkerPath = "target/debug/quiczilla-worker.exe"
)

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false
$workspace = (Get-Location).Path
$cli = (Resolve-Path -LiteralPath $CliPath).Path
$worker = (Resolve-Path -LiteralPath $WorkerPath).Path
$tempBase = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$testRoot = Join-Path $tempBase "quiczilla-daemon-e2e-$PID"
$daemon = $null
$testThumbprints = [Collections.Generic.List[string]]::new()

try {
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    $clientIdentity = Join-Path $testRoot "client-identity"
    $serverIdentity = Join-Path $testRoot "server-identity"
    $rogueIdentity = Join-Path $testRoot "rogue-identity"
    $receiveDir = Join-Path $testRoot "received"
    New-Item -ItemType Directory -Path $receiveDir | Out-Null

    $clientThumbprint = (& $cli identity --identity-dir $clientIdentity 2>$null | Select-Object -Last 1).Trim()
    if ($clientThumbprint -notmatch "^[0-9A-F]{40}$") {
        throw "Invalid client thumbprint: $clientThumbprint"
    }
    $testThumbprints.Add($clientThumbprint)

    $allowFile = Join-Path $testRoot "authorized_thumbprints"
    Set-Content -LiteralPath $allowFile -Value @("# authorized CI client", $clientThumbprint)

    $portProbe = [Net.Sockets.UdpClient]::new(0)
    $daemonPort = ([Net.IPEndPoint]$portProbe.Client.LocalEndPoint).Port
    $portProbe.Dispose()

    $stdoutLog = Join-Path $testRoot "daemon.stdout.log"
    $stderrLog = Join-Path $testRoot "daemon.stderr.log"
    $daemonArgs = @(
        "--daemon", "--port", "$daemonPort",
        "--allow-thumbprints", $allowFile,
        "--save-dir", $receiveDir,
        "--identity-dir", $serverIdentity,
        "--on-conflict", "refuse"
    )
    $daemon = Start-Process -FilePath $worker -ArgumentList $daemonArgs `
        -RedirectStandardOutput $stdoutLog -RedirectStandardError $stderrLog `
        -WindowStyle Hidden -PassThru

    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    $readyLine = $null
    do {
        Start-Sleep -Milliseconds 100
        if (Test-Path -LiteralPath $stdoutLog) {
            $readyLine = Get-Content -LiteralPath $stdoutLog -ErrorAction SilentlyContinue |
                Select-Object -First 1
        }
    } until ($readyLine -or $daemon.HasExited -or [DateTime]::UtcNow -gt $deadline)
    if (-not $readyLine) {
        $daemonLog = Get-Content -LiteralPath $stderrLog -Raw -ErrorAction SilentlyContinue
        throw "Daemon did not become ready. $daemonLog"
    }

    $ready = $readyLine | ConvertFrom-Json
    $serverThumbprint = $ready.thumbprint.Trim()
    $testThumbprints.Add($serverThumbprint)

    $first = Join-Path $testRoot "first.bin"
    $second = Join-Path $testRoot "second.bin"
    $firstBytes = New-Object byte[] 1048576
    $secondBytes = New-Object byte[] 524288
    [Security.Cryptography.RandomNumberGenerator]::Fill($firstBytes)
    [Security.Cryptography.RandomNumberGenerator]::Fill($secondBytes)
    [IO.File]::WriteAllBytes($first, $firstBytes)
    [IO.File]::WriteAllBytes($second, $secondBytes)

    foreach ($source in @($first, $second)) {
        & $cli direct "127.0.0.1:$daemonPort" $source --thumbprint $serverThumbprint `
            --identity-dir $clientIdentity --checksum --no-progress
        if ($LASTEXITCODE -ne 0) {
            throw "Authorized direct transfer failed with exit code $LASTEXITCODE"
        }
        $destination = Join-Path $receiveDir (Split-Path -Leaf $source)
        if ((Get-FileHash -LiteralPath $source).Hash -ne (Get-FileHash -LiteralPath $destination).Hash) {
            throw "Hash mismatch for $(Split-Path -Leaf $source)"
        }
    }

    $firstDestination = Join-Path $receiveDir "first.bin"
    $firstHashBefore = (Get-FileHash -LiteralPath $firstDestination -Algorithm SHA256).Hash
    & $cli direct "127.0.0.1:$daemonPort" $first --thumbprint $serverThumbprint `
        --identity-dir $clientIdentity --no-progress
    if ($LASTEXITCODE -eq 0) {
        throw "Daemon accepted a transfer over an existing destination"
    }
    $firstHashAfter = (Get-FileHash -LiteralPath $firstDestination -Algorithm SHA256).Hash
    if ($firstHashAfter -ne $firstHashBefore) {
        throw "Refused transfer changed the existing destination"
    }

    $rogueThumbprint = (& $cli identity --identity-dir $rogueIdentity 2>$null | Select-Object -Last 1).Trim()
    $testThumbprints.Add($rogueThumbprint)
    $unauthorized = Join-Path $testRoot "unauthorized.bin"
    [IO.File]::WriteAllBytes($unauthorized, [byte[]](1, 2, 3, 4))
    & $cli direct "127.0.0.1:$daemonPort" $unauthorized --thumbprint $serverThumbprint `
        --identity-dir $rogueIdentity --no-progress
    if ($LASTEXITCODE -eq 0 -or (Test-Path -LiteralPath (Join-Path $receiveDir "unauthorized.bin"))) {
        throw "Daemon accepted an unauthorized client identity"
    }

    $wrongPin = "0000000000000000000000000000000000000000"
    & $cli direct "127.0.0.1:$daemonPort" $unauthorized --thumbprint $wrongPin `
        --identity-dir $clientIdentity --no-progress
    if ($LASTEXITCODE -eq 0) {
        throw "Client accepted an incorrect daemon thumbprint"
    }
    if ($daemon.HasExited) {
        throw "Daemon exited before the multi-session test completed"
    }

    Write-Host "Windows daemon E2E passed: two verified transfers and mutual rejection checks."
}
finally {
    if ($daemon -and -not $daemon.HasExited) {
        Stop-Process -Id $daemon.Id -Force -ErrorAction SilentlyContinue
    }
    foreach ($thumbprint in $testThumbprints) {
        $certificate = Get-ChildItem -LiteralPath "Cert:\CurrentUser\My\$thumbprint" -ErrorAction SilentlyContinue
        if ($certificate) {
            Remove-Item -LiteralPath $certificate.PSPath -Force
        }
    }
    if (Test-Path -LiteralPath $testRoot) {
        $resolvedRoot = (Resolve-Path -LiteralPath $testRoot).Path
        $resolvedBase = (Resolve-Path -LiteralPath $tempBase).Path
        if (-not $resolvedRoot.StartsWith($resolvedBase, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to remove unexpected test path: $resolvedRoot"
        }
        Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
    }
    Set-Location -LiteralPath $workspace
}

# The two negative authentication checks intentionally return nonzero from the
# CLI. Do not let their final native exit code turn a successful test into a
# failed GitHub Actions step.
exit 0
