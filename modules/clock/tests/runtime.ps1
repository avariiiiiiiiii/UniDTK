$ErrorActionPreference = 'Stop'
$clockRepo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$clockCli = Join-Path $clockRepo 'target/release/udtk.exe'
$expression = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'runtime.js'))
$clockResult = & $clockCli gui "js:$expression"
$clockText = $clockResult -join "`n"
Write-Output $clockText
if ($LASTEXITCODE -ne 0 -or $clockText -notmatch '"passed":(\d+)') { throw 'Installed clock runtime checks failed' }
$clockPassed = $Matches[1]
if ($clockText -notmatch '"total":(\d+)' -or $clockPassed -ne $Matches[1]) { throw 'Installed clock runtime checks failed' }
