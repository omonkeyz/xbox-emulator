[CmdletBinding(PositionalBinding = $false)]
param(
  [Parameter(Position = 0, ValueFromRemainingArguments = $true)]
  [string[]]$Commands,
  [string]$File = '',
  [string]$Dir = '',
  [switch]$Wait
)

$ErrorActionPreference = 'Stop'
if (-not $Dir) {
  if ($env:TOMBOX_PAD_DIR) { $Dir = $env:TOMBOX_PAD_DIR } else { $Dir = Join-Path $env:TEMP 'tombox\pad' }
}
New-Item -ItemType Directory -Force -Path $Dir | Out-Null

$lines = @()
if ($File) { $lines += Get-Content -LiteralPath $File }
if ($Commands) { foreach ($c in $Commands) { $lines += ($c -split ';') } }
$lines = @($lines | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($lines.Count -eq 0) {
  Write-Host "usage: pad.ps1 'press down' 'press a' | pad.ps1 'press a; wait 500; press b' | pad.ps1 -File script.txt [-Wait]"
  Write-Host "verbs: press|tap <btn>[+<btn>] [ms], hold <btn> <ms>, down <btn>, up <btn>, stick L|R <x> <y> [ms], trigger LT|RT <0..1> [ms], wait <ms>, release, connect, disconnect (the pad plugs itself in on the first command)"
  Write-Host "buttons: A B X Y menu view up down left right LB RB LS RS LT RT LS_up LS_down LS_left LS_right RS_up RS_down RS_left RS_right"
  Write-Host "host input (a real XInput pad, keyboard WASD/arrows/Space... while the title window is focused) feeds the same pad; a running script, or a scripted 'down' not yet released, masks it (compat-common pad_host.rs)"
  exit 1
}

$name = '{0:D20}_{1}' -f [DateTime]::UtcNow.Ticks, ([Guid]::NewGuid().ToString('N').Substring(0, 6))
$tmp = Join-Path $Dir "$name.tmp"
$dst = Join-Path $Dir "$name.cmd"
[IO.File]::WriteAllText($tmp, (($lines -join "`n") + "`n"), (New-Object Text.UTF8Encoding $false))
Move-Item -LiteralPath $tmp -Destination $dst
Write-Host "queued $($lines.Count) command(s) -> $dst"

if ($Wait) {
  $deadline = (Get-Date).AddSeconds(60)
  while ((Test-Path -LiteralPath $dst) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
  if (Test-Path -LiteralPath $dst) { Write-Host "not consumed within 60 s (is the title running with the virtual pad installed?)"; exit 2 }
  Write-Host "executed (see [gamepad] lines in the compat log)"
}
