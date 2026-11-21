# Run in an x64 Visual Studio Developer PowerShell with stable MSVC Rust on PATH.
$ErrorActionPreference = 'Stop'

function Invoke-BuildTool([string]$name) {
    $tool = Get-Command $name -CommandType Application
    # Windows PowerShell may turn redirected compiler warnings into errors.
    $ErrorActionPreference = 'Continue'
    & $tool @args
    if ($LASTEXITCODE -ne 0) { throw "$name exited with $LASTEXITCODE" }
}

$previousLog = $env:TOMBOX_LOG
$previousLimit = $env:MDBG_LIMIT
Push-Location (Join-Path $PSScriptRoot '..\..')
try {
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
    Invoke-BuildTool cargo build --locked --release -p xaudio2_9 --target-dir $target

    $out = (New-Item -Force -ItemType Directory -Path (Join-Path $target ("guest-pcm\" + [guid]::NewGuid()))).FullName
    $guest = Join-Path $out 'pcm_guest.exe'
    $tracer = Join-Path $out 'mdbg.exe'
    $imports = Join-Path $out 'guest-xaudio.lib'
    Copy-Item -LiteralPath (Join-Path $target 'release\XAudio2_9.dll') -Destination $out
    Invoke-BuildTool rustc --edition=2021 -O tools/dev/mdbg.rs -o $tracer

    Invoke-BuildTool lib.exe /nologo /machine:x64 /def /name:XAudio2_9.dll '/export:GuestXAudio2Create,@1,NONAME' "/out:$imports"
    Invoke-BuildTool cl.exe /nologo /O1 /W4 /EHsc /MD "/Fo$out\pcm_guest.obj" "/Fe$guest" tools/verify/xaudio_pcm_guest.cpp $imports

    $env:TOMBOX_LOG = Join-Path $out 'compat.log'
    $env:MDBG_LIMIT = '20'
    $stdout = Join-Path $out 'guest.stdout.log'
    $stderr = Join-Path $out 'guest.stderr.log'
    Write-Output "Guest and launch logs in $out"
    $process = Start-Process -FilePath $tracer -ArgumentList ('"{0}"' -f $guest) -WindowStyle Hidden -PassThru -Wait `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $output = Get-Content -LiteralPath $stdout
    $exitEvents = Get-Content -LiteralPath $stderr | Where-Object { $_ -match '^\[trace\] EXIT_PROCESS ' }
    $output | Where-Object { $_ -match '^\[ODS\] PCM ' }
    $exitEvents

    $shim = Join-Path $out 'XAudio2_9.dll'
    $backend = Join-Path ([Environment]::SystemDirectory) 'XAudio2_9.dll'
    $route = if (Test-Path -LiteralPath $env:TOMBOX_LOG) {
        Select-String -LiteralPath $env:TOMBOX_LOG -SimpleMatch "compat module $shim delegating to host $backend"
    }
    $route.Line
    if (-not $route -or $process.ExitCode -ne 0 -or $exitEvents -notcontains '[trace] EXIT_PROCESS code=0 (0x0)' -or
        -not ($output -match '^\[ODS\] PCM submitted_frames=960 context=([0-9a-f]+) OnBufferEnd=\1 OnStreamEnd=1 wait=0$')) {
        throw "PCM guest failed. See $out"
    }
} finally {
    $env:TOMBOX_LOG = $previousLog
    $env:MDBG_LIMIT = $previousLimit
    Pop-Location
}
