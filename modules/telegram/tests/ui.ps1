$ErrorActionPreference = 'Stop'
$tgRepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$tgCli = Join-Path $tgRepoRoot 'target/release/udtk.exe'
# Windows PowerShell 5.1 mangles quotes, spaces and commas in native arguments, so the
# expression travels as base64 (no such characters) and the page decodes and evaluates it.
function Invoke-TgFixtureJs([string]$expression) {
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($expression))
    $result = & $tgCli gui "js:(0,eval)(new TextDecoder().decode(Uint8Array.from(atob('$encoded'),c=>c.charCodeAt(0))))"
    if ($LASTEXITCODE -ne 0 -or ($result -join "`n") -match '\[1\] js\s+(error|FAIL)') {
        throw ($result -join "`n")
    }
    return $result
}
$tgSource = [IO.File]::ReadAllText((Join-Path $tgRepoRoot 'modules/telegram/ui/widget.js'))
$tgSource = $tgSource.Replace("import { invoke, convertFileSrc, listen, openPath } from '@unidtk/bridge';", "const fixtureCalls = []; const fixtureReplies = new Map(); const invoke = async (command, args) => { fixtureCalls.push({ command, args }); if (fixtureReplies.has(command)) return await fixtureReplies.get(command); return command === 'telegram_messages' ? [] : {}; }; const convertFileSrc = p => p, listen = async () => (() => {}), openPath = async () => {}; ")
$tgSource = $tgSource.Replace("import { saveConfig, showDebugMessage, logFrontend, repairMojibake, editMode, widgetsCanvas, config, defaultWidgetPosition, cachedTelegramCard, escapeHtml, placeOnCanvas } from '../../unidtk-core.js';", 'const { saveConfig, showDebugMessage, logFrontend, repairMojibake, editMode, widgetsCanvas, config, defaultWidgetPosition, cachedTelegramCard, escapeHtml, placeOnCanvas } = { ...deps.core, saveConfig: async () => {} };')
$tgSource = $tgSource.Replace("import { surfaceManager } from '../../surface-manager.js';", 'const { surfaceManager } = deps.surface;')
$tgSource = $tgSource.Replace("import { calculateDuration, getDuration, prefersReducedMotion, EASINGS, motionTimeline } from '../../motion.js';", 'const { calculateDuration, getDuration, EASINGS, motionTimeline } = deps.motion; let fixtureReducedMotion = false; const prefersReducedMotion = () => fixtureReducedMotion || deps.motion.prefersReducedMotion();')
$tgSource = $tgSource.Replace("import { getFileIconName, getFileTypeName } from '../../file-icons.js';", 'const { getFileIconName, getFileTypeName } = deps.icons;')
if ($tgSource -match '(?m)^import .*$') {
    throw "widget.js import not rewritten for the fixture: $($Matches[0])"
}
$tgSource += "`n" + [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'ui.js'))
$tgSource = [regex]::Replace($tgSource, '(?m)^export \{[^}]+\};\r?$', '')
$tgSource = $tgSource.Replace('export async function runTelegramUiTests()', 'async function runTelegramUiTests()')
$tgSource += "`nreturn await runTelegramUiTests();"
$tgCss = [IO.File]::ReadAllText((Join-Path $tgRepoRoot 'core/ui/styles.css'))
Invoke-TgFixtureJs "window.__tgFixtureSource='';window.__tgFixtureCss='';true" | Out-Null
foreach ($item in @(@{ key = '__tgFixtureSource'; content = $tgSource }, @{ key = '__tgFixtureCss'; content = $tgCss })) {
    for ($offset = 0; $offset -lt $item.content.Length; $offset += 4000) {
        $chunk = $item.content.Substring($offset, [Math]::Min(4000, $item.content.Length - $offset)) | ConvertTo-Json -Compress
        Invoke-TgFixtureJs "window.$($item.key)+=$chunk;true" | Out-Null
    }
}
$tgFixtureOutput = Invoke-TgFixtureJs @'
(async()=>{
  const previousRuntime=window.__telegramRuntime, previousRender=window.renderTelegramWidget;
  const sheets=Array.from(document.querySelectorAll('link[rel="stylesheet"]')).filter(s=>s.href.includes('/styles.css')).map(s=>({s,disabled:s.disabled}));
  sheets.forEach(({s})=>s.disabled=true);
  const style=document.createElement('style');
  style.textContent=window.__tgFixtureCss;
  document.head.appendChild(style);
  try {
    const execute=new (Object.getPrototypeOf(async()=>{}).constructor)('deps',window.__tgFixtureSource);
    const [core,surface,motion,icons]=await Promise.all(['unidtk-core.js','surface-manager.js','motion.js','file-icons.js'].map(p=>import('http://tauri.localhost/'+p)));
    return await execute({core,surface,motion,icons});
  } finally { style.remove();sheets.forEach(({s,disabled})=>s.disabled=disabled);window.__telegramRuntime=previousRuntime;window.renderTelegramWidget=previousRender;delete window.__tgFixtureSource;delete window.__tgFixtureCss; }
})()
'@
$tgFixtureText = $tgFixtureOutput -join "`n"
Write-Output $tgFixtureText
if ($tgFixtureText -notmatch '"passed":(\d+),"total":(\d+)') {
    throw 'Fixture did not return a check count'
}
if ($Matches[1] -ne $Matches[2]) {
    throw "Fixture checks failed: $($Matches[1])/$($Matches[2])"
}
