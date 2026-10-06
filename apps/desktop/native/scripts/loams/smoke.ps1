# Windows smoke test for the Loams commands against the in-process mock.
# `loams-desktop.exe` is a GUI-subsystem binary, so output is captured with
# Start-Process redirection rather than a pipeline (see test-windows-startup.ps1).
param([Parameter(Mandatory = $true)][string]$Exe)
$ErrorActionPreference = 'Stop'
$env:LOAMS_MOCK = '1'
$exe = (Resolve-Path -LiteralPath $Exe).Path
$work = Join-Path $env:RUNNER_TEMP 'loams-smoke'
New-Item -ItemType Directory -Force -Path $work | Out-Null

function Invoke-Loams Desktop([string[]]$Arguments, [string]$StdinText = $null) {
    $out = Join-Path $work 'out.txt'
    $err = Join-Path $work 'err.txt'
    $params = @{
        FilePath = $exe; ArgumentList = $Arguments; Wait = $true; PassThru = $true
        RedirectStandardOutput = $out; RedirectStandardError = $err; NoNewWindow = $true
    }
    if ($StdinText) {
        $in = Join-Path $work 'in.txt'
        [IO.File]::WriteAllText($in, $StdinText)
        $params.RedirectStandardInput = $in
    }
    $p = Start-Process @params
    $text = (Get-Content -Raw -LiteralPath $out)
    if ($p.ExitCode -ne 0) { throw "loams-desktop $Arguments exited $($p.ExitCode): $(Get-Content -Raw -LiteralPath $err)" }
    return $text
}

$status = Invoke-Loams Desktop @('loams', 'status')
Write-Host $status
if ($status -notmatch 'Loams \(mock\)') { throw 'status did not show the mock instance' }

$bot = Invoke-Loams Desktop @('loams', 'bot', 'file an issue for the checkout 500s')
Write-Host $bot
if ($bot -notmatch 'you said: file an issue') { throw 'bot did not answer' }

$lines = @(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}',
    '{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":".","mcpServers":[]}}',
    '{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"loams-bot-1","prompt":[{"type":"text","text":"hello"}]}}'
) -join "`n"
$acp = Invoke-Loams Desktop @('loams', 'bot-acp') ($lines + "`n")
Write-Host $acp
if ($acp -notmatch '"stopReason":"end_turn"') { throw 'ACP prompt did not finish' }
Write-Host 'smoke ok'
