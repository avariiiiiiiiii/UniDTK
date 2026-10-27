# Integration probe: refresh through the real Windows session host, without sleeping the PC.
$ErrorActionPreference = 'Stop'
$taskCli = Join-Path $env:ProgramFiles 'UniDTK/udtk.exe'
function Read-Weather { & $taskCli invoke weather_current | Out-String | ConvertFrom-Json }
$taskBefore = Read-Weather
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class UniDtkResumeProbe {
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls,string title);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd,out uint pid);
 [DllImport("user32.dll",SetLastError=true)] [return:MarshalAs(UnmanagedType.Bool)] public static extern bool PostMessage(IntPtr hwnd,uint msg,UIntPtr wp,IntPtr lp);
}
'@
$taskWindow = [UniDtkResumeProbe]::FindWindow('Shell_SessionEnd','Shell session-end listener')
if ($taskWindow -eq [IntPtr]::Zero) { throw 'Resume host absent' }
[uint32]$taskWindowPid = 0
[void][UniDtkResumeProbe]::GetWindowThreadProcessId($taskWindow,[ref]$taskWindowPid)
$taskProc = Get-CimInstance Win32_Process -Filter "ProcessId=$taskWindowPid"
if ($taskProc.ExecutablePath -ne (Join-Path $env:ProgramFiles 'UniDTK/unidtk.exe')) { throw 'Unexpected resume host owner' }
if (-not [UniDtkResumeProbe]::PostMessage($taskWindow,0x218,[UIntPtr]0x12,[IntPtr]::Zero)) { throw 'Resume notification failed' }
Start-Sleep -Milliseconds 100
$taskInvalidated = Read-Weather
$taskDeadline = [DateTime]::UtcNow.AddSeconds(30)
do {
    Start-Sleep -Seconds 1
    $taskAfter = Read-Weather
    $taskRefreshed = $taskAfter.ipLocation.resolvedAt -gt $taskBefore.ipLocation.resolvedAt -and
        $taskAfter.observedAt -gt $taskBefore.observedAt -and -not $taskAfter.ipLocation.stale -and
        -not $taskAfter.stale -and $taskAfter.locationNameStatus -eq 'available'
} until ($taskRefreshed -or [DateTime]::UtcNow -ge $taskDeadline)
[pscustomobject]@{
    beforeIP = $taskBefore.ipLocation.resolvedAt; beforeWeather = $taskBefore.observedAt
    invalidatedIP = $taskInvalidated.ipLocation.stale; invalidatedWeather = $taskInvalidated.stale
    afterIP = $taskAfter.ipLocation.resolvedAt; afterWeather = $taskAfter.observedAt
    freshIP = -not $taskAfter.ipLocation.stale; freshWeather = -not $taskAfter.stale
    placeStatus = $taskAfter.locationNameStatus; automaticRefresh = $taskRefreshed
} | ConvertTo-Json
if (-not $taskRefreshed) { throw 'Automatic resume refresh did not finish' }
