[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$SourcePath,
    [Parameter(Mandatory)]
    [string]$SshTarget,
    [Parameter(Mandatory)]
    [string]$RemoteDir,
    [string]$QuicPath = "quic",
    [int]$SshPort = 22,
    [ValidateRange(1, 20)]
    [int]$Runs = 3,
    [string]$OutputPath = "quiczilla-benchmark.json",
    [switch]$SkipRsync,
    [switch]$SkipQcp,
    [switch]$UsePipe,
    [string]$RsyncSshCommand,
    [switch]$RsyncCygwinPaths,
    [switch]$RequireDirectQuic,
    [string]$StunServer
)

Set-StrictMode -Version Latest
$source = (Resolve-Path -LiteralPath $SourcePath).Path
$sourceName = Split-Path -Leaf $source
$sourceBytes = (Get-Item -LiteralPath $source).Length
$sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
$stamp = [DateTime]::UtcNow.ToString("yyyyMMdd-HHmmss")
$remoteBase = "$($RemoteDir.TrimEnd('/'))/quiczilla-benchmark-$stamp-$PID"

if (-not (Get-Command ssh -ErrorAction SilentlyContinue)) {
    throw "OpenSSH 'ssh' is required"
}
if (-not (Get-Command scp -ErrorAction SilentlyContinue)) {
    throw "OpenSSH 'scp' is required"
}
if (-not $SkipRsync -and -not (Get-Command rsync -ErrorAction SilentlyContinue)) {
    Write-Warning "Optional 'rsync' is not installed; continuing with Quiczilla and SCP only."
    $SkipRsync = $true
}
if (-not $SkipQcp -and -not (Get-Command qcp -ErrorAction SilentlyContinue)) {
    Write-Warning "Optional 'qcp' is not installed locally; continuing without it. Install qcp on both endpoints to include it."
    $SkipQcp = $true
}
if (-not $RsyncSshCommand) {
    $RsyncSshCommand = "ssh -p $SshPort"
}

function Get-RsyncSourcePath {
    if (-not $RsyncCygwinPaths) {
        return $source
    }
    if ($source -notmatch "^([A-Za-z]):\\(.*)$") {
        throw "-RsyncCygwinPaths requires an absolute drive-letter source path"
    }
    $drive = $Matches[1].ToLowerInvariant()
    $remainder = $Matches[2] -replace "\\", "/"
    "/cygdrive/$drive/$remainder"
}
$rsyncSource = Get-RsyncSourcePath

function Invoke-Native {
    param([string]$FilePath, [string[]]$Arguments)

    $start = [Diagnostics.Stopwatch]::StartNew()
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $FilePath
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        $info.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $info
    try {
        if (-not $process.Start()) {
            throw "Failed to start $FilePath"
        }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        $stdout = $stdoutTask.GetAwaiter().GetResult()
        $stderr = $stderrTask.GetAwaiter().GetResult()
        [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = $stdout
            Stderr = $stderr
            WallSeconds = $start.Elapsed.TotalSeconds
        }
    }
    finally {
        $process.Dispose()
    }
}

function Invoke-PipeNative {
    param([string]$FilePath, [string[]]$Arguments, [string]$InputPath)

    $start = [Diagnostics.Stopwatch]::StartNew()
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $FilePath
    $info.UseShellExecute = $false
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        $info.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $info
    $input = $null
    try {
        if (-not $process.Start()) {
            throw "Failed to start $FilePath"
        }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $input = [IO.File]::OpenRead($InputPath)
        $input.CopyTo($process.StandardInput.BaseStream)
        $process.StandardInput.Close()
        $process.WaitForExit()
        [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = $stdoutTask.GetAwaiter().GetResult()
            Stderr = $stderrTask.GetAwaiter().GetResult()
            WallSeconds = $start.Elapsed.TotalSeconds
        }
    }
    finally {
        if ($null -ne $input) { $input.Dispose() }
        $process.Dispose()
    }
}

function Invoke-Ssh {
    param([string]$Command)
    Invoke-Native ssh @("-p", "$SshPort", $SshTarget, $Command)
}

function New-RemoteDirectory {
    param([string]$Path)
    $result = Invoke-Ssh "mkdir -p -- '$Path'"
    if ($result.ExitCode -ne 0) {
        throw "Could not create remote directory '$Path': $($result.Stderr.Trim())"
    }
}

function Test-RemoteCommand {
    param([string]$Command, [string]$DisplayName)
    $result = Invoke-Ssh "command -v -- '$Command'"
    if ($result.ExitCode -ne 0) {
        throw "$DisplayName is not available on the remote host. Install it on both endpoints or use the matching -Skip switch."
    }
}

function Get-RemoteHash {
    param([string]$Path)
    $result = Invoke-Ssh "sha256sum -- '$Path'"
    if ($result.ExitCode -ne 0) {
        throw "Could not hash remote file '$Path': $($result.Stderr.Trim())"
    }
    $match = [regex]::Match($result.Stdout, "(?i)\b[0-9a-f]{64}\b")
    if (-not $match.Success) {
        throw "Remote hash command returned no SHA-256 for '$Path'"
    }
    $match.Value.ToLowerInvariant()
}

function Invoke-BenchmarkRun {
    param(
        [string]$Tool,
        [int]$Run,
        [string[]]$Arguments,
        [string]$RemotePath
    )

    $result = Invoke-Native $Tool $Arguments
    if ($result.ExitCode -ne 0) {
        throw "$Tool run $Run failed with exit code $($result.ExitCode): $($result.Stderr.Trim())"
    }
    $remoteHash = Get-RemoteHash $RemotePath
    if ($remoteHash -ne $sourceHash) {
        throw "$Tool run $Run produced a SHA-256 mismatch"
    }

    $payloadSeconds = $null
    $transport = $Tool
    if ($Tool -eq $QuicPath) {
        $receipt = $result.Stdout.Trim() | ConvertFrom-Json
        if ($receipt.status -ne "completed") {
            throw "Quiczilla did not return a completed receipt"
        }
        $payloadSeconds = [double]$receipt.duration_seconds
        $transport = [string]$receipt.transport
        if ($RequireDirectQuic -and $transport -notin @("direct-quic", "stun-quic", "manual-quic")) {
            throw "Quiczilla run $Run used '$transport' instead of direct QUIC. Re-run without -RequireDirectQuic only when measuring SSH fallback intentionally."
        }
    }

    [pscustomobject]@{
        tool = [IO.Path]::GetFileName($Tool)
        run = $Run
        bytes = $sourceBytes
        wall_seconds = [math]::Round($result.WallSeconds, 3)
        payload_seconds = if ($null -eq $payloadSeconds) { $null } else { [math]::Round($payloadSeconds, 3) }
        wall_mib_per_second = [math]::Round($sourceBytes / 1MB / $result.WallSeconds, 3)
        transport = $transport
        sha256_verified = $true
        remote_path = $RemotePath
    }
}

function Invoke-PipeBenchmarkRun {
    param(
        [int]$Run,
        [string[]]$Arguments,
        [string]$RemotePath
    )

    $result = Invoke-PipeNative $QuicPath $Arguments $source
    if ($result.ExitCode -ne 0) {
        throw "quic pipe run $Run failed with exit code $($result.ExitCode): $($result.Stderr.Trim())"
    }
    $remoteHash = Get-RemoteHash $RemotePath
    if ($remoteHash -ne $sourceHash) {
        throw "quic pipe run $Run produced a SHA-256 mismatch"
    }
    $transportMatch = [regex]::Match($result.Stderr, "\[quiczilla pipe\] ([a-z-]+) mTLS connected")
    if (-not $transportMatch.Success) {
        throw "quic pipe run $Run did not report its selected transport"
    }
    $transport = $transportMatch.Groups[1].Value
    if ($RequireDirectQuic -and $transport -notin @("direct-quic", "stun-quic", "manual-quic")) {
        throw "quic pipe run $Run used '$transport' instead of direct QUIC"
    }

    [pscustomobject]@{
        tool = "quic-pipe"
        run = $Run
        bytes = $sourceBytes
        wall_seconds = [math]::Round($result.WallSeconds, 3)
        payload_seconds = $null
        wall_mib_per_second = [math]::Round($sourceBytes / 1MB / $result.WallSeconds, 3)
        transport = $transport
        sha256_verified = $true
        remote_path = $RemotePath
    }
}

if (-not $SkipRsync) {
    Test-RemoteCommand "rsync" "rsync"
}
if (-not $SkipQcp) {
    Test-RemoteCommand "qcp" "qcp"
}

# qcp reads OpenSSH-style configuration itself. An ephemeral, restrictive
# profile lets its SSH bootstrap use the requested non-default port without
# relying on or changing the user's personal SSH configuration.
$targetMatch = [regex]::Match($SshTarget, "^(?:(?<user>[^@]+)@)?(?<host>.+)$")
if (-not $targetMatch.Success) {
    throw "Could not derive a qcp SSH profile from '$SshTarget'"
}
$qcpHostAlias = "quiczilla-qcp-benchmark-$PID"
$qcpSshConfig = Join-Path ([IO.Path]::GetTempPath()) "$qcpHostAlias.conf"
$qcpConfig = @(
    "Host $qcpHostAlias",
    "    HostName $($targetMatch.Groups['host'].Value)",
    "    Port $SshPort",
    "    BatchMode yes"
)
if ($targetMatch.Groups['user'].Success) {
    $qcpConfig += "    User $($targetMatch.Groups['user'].Value)"
}
Set-Content -LiteralPath $qcpSshConfig -Value $qcpConfig -Encoding utf8

try {
    New-RemoteDirectory $remoteBase
    $records = [Collections.Generic.List[object]]::new()
    for ($run = 1; $run -le $Runs; $run++) {
    $quicDir = "$remoteBase/quic-$run"
    New-RemoteDirectory $quicDir
    if ($UsePipe) {
        $pipeRemotePath = "$quicDir/$sourceName"
        $pipeCommand = "cat > '$pipeRemotePath'"
        $pipeArguments = @(
            "pipe", $SshTarget, $pipeCommand, "-p", "$SshPort", "--checksum", "--no-progress"
        )
        if ($StunServer) {
            $pipeArguments += @("--transport", "stun", "--stun-server", $StunServer)
        }
        $records.Add((Invoke-PipeBenchmarkRun $run $pipeArguments $pipeRemotePath))
    } else {
        $quicTarget = "{0}:{1}/" -f $SshTarget, $quicDir
        $quicArguments = @(
            $source, $quicTarget, "-p", "$SshPort", "--checksum", "--json", "--no-progress"
        )
        if ($StunServer) {
            $quicArguments += @("--transport", "stun", "--stun-server", $StunServer)
        }
        $records.Add((Invoke-BenchmarkRun $QuicPath $run $quicArguments "$quicDir/$sourceName"))
    }

    $scpDir = "$remoteBase/scp-$run"
    New-RemoteDirectory $scpDir
    $scpTarget = "{0}:{1}/" -f $SshTarget, $scpDir
    $records.Add((Invoke-BenchmarkRun "scp" $run @(
        "-P", "$SshPort", $source, $scpTarget
    ) "$scpDir/$sourceName"))

    if (-not $SkipRsync) {
        $rsyncDir = "$remoteBase/rsync-$run"
        New-RemoteDirectory $rsyncDir
        $rsyncTarget = "{0}:{1}/" -f $SshTarget, $rsyncDir
        $records.Add((Invoke-BenchmarkRun "rsync" $run @(
            "-a", "--checksum", "-e", $RsyncSshCommand, $rsyncSource, $rsyncTarget
        ) "$rsyncDir/$sourceName"))
    }

    if (-not $SkipQcp) {
        $qcpDir = "$remoteBase/qcp-$run"
        New-RemoteDirectory $qcpDir
        $qcpTarget = "{0}:{1}/" -f $qcpHostAlias, $qcpDir
        $records.Add((Invoke-BenchmarkRun "qcp" $run @(
            "--ssh-config", $qcpSshConfig, $source, $qcpTarget
        ) "$qcpDir/$sourceName"))
    }
}

    $records | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $OutputPath -Encoding utf8
    Write-Host "Benchmark results written to $OutputPath"
    Write-Host "Remote artifacts retained under $($SshTarget):$remoteBase"
    $records | Format-Table tool,run,transport,wall_seconds,payload_seconds,wall_mib_per_second,sha256_verified
}
finally {
    Remove-Item -LiteralPath $qcpSshConfig -Force -ErrorAction SilentlyContinue
}
