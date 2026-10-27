$ErrorActionPreference = 'Stop'
$clockRepo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$clockCli = Join-Path $clockRepo 'target/release/udtk.exe'
function Invoke-ClockJs([string]$expression) {
    $result = & $clockCli gui "js:$expression"
    if ($LASTEXITCODE -ne 0 -or ($result -join "`n") -match '\[1\] js\s+(error|FAIL)') { throw ($result -join "`n") }
    return $result
}
$clockCore = [IO.File]::ReadAllText((Join-Path $clockRepo 'core/ui/unidtk-core.js'))
$clockStart = $clockCore.IndexOf('function rectSnapshot(')
$clockEnd = $clockCore.IndexOf('async function toggleWidget(', $clockStart)
$clockFixture = 'const {motionTimeline,EASINGS}=deps.motion;' + $clockCore.Substring($clockStart, $clockEnd - $clockStart)
$clockFixture += "`n" + [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'ui.js'))
Invoke-ClockJs "window.__clockFixtureSource='';true" | Out-Null
for ($offset = 0; $offset -lt $clockFixture.Length; $offset += 8000) {
    $chunk = $clockFixture.Substring($offset, [Math]::Min(8000, $clockFixture.Length - $offset)) | ConvertTo-Json -Compress
    Invoke-ClockJs "window.__clockFixtureSource+=$chunk;true" | Out-Null
}
$clockOutput = Invoke-ClockJs @'
(async()=>{
  try {
    const execute=new (Object.getPrototypeOf(async()=>{}).constructor)('deps',window.__clockFixtureSource);
    const [clock,motion]=await Promise.all([import('http://tauri.localhost/modules/clock/widget.js'),import('http://tauri.localhost/motion.js')]);
    return await execute({clock,motion});
  } finally {delete window.__clockFixtureSource;}
})()
'@
$clockText = $clockOutput -join "`n"
Write-Output $clockText
if ($clockText -notmatch '"passed":(\d+),"total":(\d+)' -or $Matches[1] -ne $Matches[2]) { throw 'Clock fixture failed' }
