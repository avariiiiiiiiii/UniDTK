$ErrorActionPreference = 'Stop'
$clockRepo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$clockCli = Join-Path $clockRepo 'target/release/udtk.exe'
function Invoke-ClockJs([string]$expression) {
    $result = & $clockCli gui "js:$expression"
    if ($LASTEXITCODE -ne 0 -or ($result -join "`n") -match '\[1\] js\s+(error|FAIL)') { throw ($result -join "`n") }
    return $result
}
$clockFixture = [IO.File]::ReadAllBytes((Join-Path $PSScriptRoot 'ui.js'))
# Quote-free transport: Windows PowerShell 5.1 splits an argument at the double quotes inside it.
Invoke-ClockJs "window.__clockFixtureSource='';true" | Out-Null
for ($offset = 0; $offset -lt $clockFixture.Length; $offset += 6000) {
    $chunk = [Convert]::ToBase64String($clockFixture, $offset, [Math]::Min(6000, $clockFixture.Length - $offset))
    Invoke-ClockJs "window.__clockFixtureBytes=(window.__clockFixtureBytes||[]).concat(Array.from(atob('$chunk'),c=>c.charCodeAt(0)));true" | Out-Null
}
Invoke-ClockJs "window.__clockFixtureSource=new TextDecoder().decode(Uint8Array.from(window.__clockFixtureBytes));delete window.__clockFixtureBytes;true" | Out-Null
$clockOutput = Invoke-ClockJs @'
(async()=>{
  try {
    const execute=new (Object.getPrototypeOf(async()=>{}).constructor)('deps',window.__clockFixtureSource);
    const [clock,motion,surface]=await Promise.all([import('http://tauri.localhost/modules/clock/widget.js'),import('http://tauri.localhost/motion.js'),import('http://tauri.localhost/surface-manager.js')]);
    return await execute({clock,motion,sm:surface.surfaceManager});
  } finally {delete window.__clockFixtureSource;}
})()
'@
$clockText = $clockOutput -join "`n"
Write-Output $clockText
if ($clockText -notmatch '"passed":(\d+),"total":(\d+)' -or $Matches[1] -ne $Matches[2]) { throw 'Clock fixture failed' }
