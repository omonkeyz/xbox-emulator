# Run in an x64 Visual Studio Developer PowerShell with stable MSVC Rust on PATH.
$ErrorActionPreference = 'Stop'

function Test-GuestExit([string]$name, [string]$commandLine, [int]$expectedExit, [string]$debugPattern) {
    $stdout = Join-Path $out "$name.stdout.log"
    $stderr = Join-Path $out "$name.stderr.log"
    $process = Start-Process -FilePath $tracer -ArgumentList $commandLine -WindowStyle Hidden -PassThru -Wait `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $output = Get-Content -LiteralPath $stdout
    $exitEvents = Get-Content -LiteralPath $stderr | Where-Object { $_ -match '^\[trace\] EXIT_PROCESS ' }
    if ($debugPattern) { $output }
    $exitEvents

    # mdbg's own exit code can be zero even if its child failed.
    $expectedEvent = '[trace] EXIT_PROCESS code={0} (0x{0:x})' -f $expectedExit
    if ($process.ExitCode -ne 0 -or $exitEvents -notcontains $expectedEvent -or
        ($debugPattern -and -not ($output -match $debugPattern))) {
        throw "$name failed. See $stdout and $stderr"
    }
}

Push-Location (Join-Path $PSScriptRoot '..\..')
try {
    $out = (New-Item -ItemType Directory -Force -Path ('target\guest-launch\' + [guid]::NewGuid())).FullName
    $guest = Join-Path $out 'xaudio_guest.exe'
    $tracer = Join-Path $out 'mdbg.exe'

    cargo build --locked --release -p gcxaudioext --target-dir target
    if ($LASTEXITCODE -ne 0) { throw 'Failed to build the GCXAudioExt compatibility DLL' }
    rustc --edition=2021 -O tools/dev/mdbg.rs -o $tracer
    if ($LASTEXITCODE -ne 0) { throw 'Failed to build mdbg' }
    cl.exe /nologo /O1 /W4 /MD "/Fo$out\xaudio_guest.obj" "/Fe$guest" tools/verify/xaudio_guest.c /link /Brepro target/release/gcxaudioext.dll.lib
    if ($LASTEXITCODE -ne 0) { throw 'Failed to build the guest executable' }
    Copy-Item -LiteralPath 'target/release/gcxaudioext.dll' -Destination $out
    Write-Output "Guest and launch logs in $out"

    $mask = '^\[ODS\] guest pid=\d+ GetXAudioDefaultProcessorForCurrentPlatform\(\)=0x00000020$'
    foreach ($run in 1..2) {
        Test-GuestExit "guest-$run" ('"{0}"' -f $guest) 0 $mask
    }
    $cmd = Join-Path $env:SystemRoot 'System32\cmd.exe'
    Test-GuestExit 'exit-37' ('"{0}" /d /c exit 37' -f $cmd) 37
} finally {
    Pop-Location
}
