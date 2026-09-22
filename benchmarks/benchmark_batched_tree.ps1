[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$SourceDirectory,
    [Parameter(Mandatory)]
    [string]$SshTarget,
    [Parameter(Mandatory)]
    [string]$RemoteDir,
    [string]$QuicPath = "quic",
    [int]$SshPort = 22,
    [ValidateRange(1, 20)]
    [int]$Runs = 3,
    [string]$OutputPath = "quiczilla-batched-benchmark.json",
    [string]$StunServer,
    [string]$RsyncSshCommand,
    [switch]$RsyncCygwinPaths
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$source = (Resolve-Path -LiteralPath $SourceDirectory).Path
$sourceName = Split-Path -Leaf $source
$sourceFiles = @(Get-ChildItem -LiteralPath $source -File -Recurse)
if ($sourceFiles.Count -eq 0) { throw "SourceDirectory contains no files" }
$sourceBytes = [int64](($sourceFiles | Measure-Object Length -Sum).Sum)
$stamp = [DateTime]::UtcNow.ToString("yyyyMMdd-HHmmss")
$remoteBase = "$($RemoteDir.TrimEnd('/'))/quiczilla-batch-$stamp-$PID"
$archive = Join-Path ([IO.Path]::GetTempPath()) "quiczilla-batch-$PID.tar"

foreach ($command in "ssh", "scp", "rsync", "tar") {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "Required command '$command' is not on PATH"
    }
}
if (-not $RsyncSshCommand) { $RsyncSshCommand = "ssh -p $SshPort" }
if (-not $StunServer) { throw "-StunServer is required for the direct-QUIC pipe measurement" }

function Quote-Posix([string]$Value) {
    "'" + $Value.Replace("'", "'`"'`"'") + "'"
}

function Invoke-Native([string]$FilePath, [string[]]$Arguments) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $FilePath; $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true; $info.RedirectStandardError = $true
    foreach ($argument in $Arguments) { [void]$info.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::new(); $process.StartInfo = $info
    try {
        if (-not $process.Start()) { throw "Failed to start $FilePath" }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync(); $stderrTask = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        [pscustomobject]@{ ExitCode=$process.ExitCode; Stdout=$stdoutTask.GetAwaiter().GetResult(); Stderr=$stderrTask.GetAwaiter().GetResult(); WallSeconds=$timer.Elapsed.TotalSeconds }
    } finally { $process.Dispose() }
}

function Invoke-Pipe([string[]]$Arguments) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $QuicPath; $info.UseShellExecute = $false
    $info.RedirectStandardInput = $true; $info.RedirectStandardOutput = $true; $info.RedirectStandardError = $true
    foreach ($argument in $Arguments) { [void]$info.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::new(); $process.StartInfo = $info; $input = $null
    try {
        if (-not $process.Start()) { throw "Failed to start $QuicPath" }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync(); $stderrTask = $process.StandardError.ReadToEndAsync()
        $input = [IO.File]::OpenRead($archive)
        $input.CopyTo($process.StandardInput.BaseStream)
        $process.StandardInput.Close(); $process.WaitForExit()
        [pscustomobject]@{ ExitCode=$process.ExitCode; Stdout=$stdoutTask.GetAwaiter().GetResult(); Stderr=$stderrTask.GetAwaiter().GetResult(); WallSeconds=$timer.Elapsed.TotalSeconds }
    } finally { if ($null -ne $input) { $input.Dispose() }; $process.Dispose() }
}

function Invoke-Ssh([string]$Command) { Invoke-Native "ssh" @("-p", "$SshPort", $SshTarget, $Command) }

function New-RemoteDirectory([string]$Path) {
    $result = Invoke-Ssh "mkdir -p -- $(Quote-Posix $Path)"
    if ($result.ExitCode -ne 0) { throw "Could not create remote directory: $($result.Stderr.Trim())" }
}

function Get-LocalManifest {
    $rows = foreach ($file in $sourceFiles) {
        $relative = [IO.Path]::GetRelativePath($source, $file.FullName).Replace("\", "/")
        $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        [pscustomobject]@{ RelativePath = $relative; Line = "$hash  ./$relative" }
    }
    $text = (($rows | Sort-Object RelativePath | Select-Object -ExpandProperty Line) -join "`n") + "`n"
    $bytes = [Text.Encoding]::UTF8.GetBytes($text)
    ([Security.Cryptography.SHA256]::HashData($bytes) | ForEach-Object ToString x2) -join ""
}

function Get-RemoteManifest([string]$Path) {
    $quoted = Quote-Posix $Path
    $command = "cd $quoted && find . -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum | awk '{print `$1}'"
    $result = Invoke-Ssh $command
    if ($result.ExitCode -ne 0) { throw "Could not create remote manifest: $($result.Stderr.Trim())" }
    $result.Stdout.Trim().ToLowerInvariant()
}

function Assert-RemoteTree([string]$Path) {
    $remoteManifest = Get-RemoteManifest $Path
    if ($remoteManifest -ne $localManifest) { throw "Remote manifest mismatch for $Path" }
}

function Get-RsyncSource {
    if (-not $RsyncCygwinPaths) { return "$source/" }
    if ($source -notmatch "^([A-Za-z]):\\(.*)$") { throw "-RsyncCygwinPaths requires a drive-letter source path" }
    "/cygdrive/$($Matches[1].ToLowerInvariant())/$($Matches[2].Replace('\', '/'))/"
}

function Add-Record([Collections.Generic.List[object]]$Records, [string]$Tool, [int]$Run, [object]$Result, [string]$Transport, [string]$RemotePath) {
    if ($Result.ExitCode -ne 0) { throw "$Tool run $Run failed: $($Result.Stderr.Trim())" }
    Assert-RemoteTree $RemotePath
    $Records.Add([pscustomobject]@{ tool=$Tool; run=$Run; bytes=$sourceBytes; wall_seconds=[math]::Round($Result.WallSeconds,3); wall_mib_per_second=[math]::Round($sourceBytes/1MB/$Result.WallSeconds,3); transport=$Transport; sha256_verified=$true })
}

$localManifest = Get-LocalManifest
try {
    $tar = Invoke-Native "tar" @("-cf", $archive, "-C", $source, ".")
    if ($tar.ExitCode -ne 0) { throw "Could not create pipe archive: $($tar.Stderr.Trim())" }
    New-RemoteDirectory $remoteBase
    $records = [Collections.Generic.List[object]]::new()
    $rsyncSource = Get-RsyncSource
    for ($run = 1; $run -le $Runs; $run++) {
        $pipeDir = "$remoteBase/pipe-$run"; New-RemoteDirectory $pipeDir
        $pipeCommand = "mkdir -p $(Quote-Posix $pipeDir) && tar -xf - -C $(Quote-Posix $pipeDir)"
        $pipeArgs = @("pipe", $SshTarget, $pipeCommand, "-p", "$SshPort", "--checksum", "--no-progress", "--transport", "stun", "--stun-server", $StunServer)
        Add-Record $records "quic-pipe-tar" $run (Invoke-Pipe $pipeArgs) "stun-quic" $pipeDir

        $scpDir = "$remoteBase/scp-$run"; New-RemoteDirectory $scpDir
        Add-Record $records "scp-recursive" $run (Invoke-Native "scp" @("-r", "-P", "$SshPort", $source, "${SshTarget}:$scpDir/")) "scp" "$scpDir/$sourceName"

        $rsyncDir = "$remoteBase/rsync-$run"; New-RemoteDirectory $rsyncDir
        Add-Record $records "rsync-recursive" $run (Invoke-Native "rsync" @("-a", "--checksum", "-e", $RsyncSshCommand, $rsyncSource, "${SshTarget}:$rsyncDir/")) "rsync" $rsyncDir
    }
    $records | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $OutputPath -Encoding utf8
    $records | Format-Table tool,run,transport,wall_seconds,wall_mib_per_second,sha256_verified
} finally {
    Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
}
