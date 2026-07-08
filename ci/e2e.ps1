# End-to-end test on a real Windows machine (GitHub Actions windows runner).
#
# Starts mackey.exe in test mode (MACKEY_TEST_TREAT_INJECTED=1 lets the hook
# treat SendKeys input as physical — normal builds ignore injected events),
# then drives a real WinForms textbox with mac-style chords and asserts on
# what actually happened.
#
# Usage: powershell -File ci\e2e.ps1 <path-to-mackey.exe>

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms

$exePath = Resolve-Path $args[0]
Write-Host "== MacKey E2E on $([System.Environment]::OSVersion.VersionString) =="

$env:MACKEY_TEST_TREAT_INJECTED = "1"
$proc = Start-Process -FilePath $exePath -PassThru
Start-Sleep -Seconds 4

# -- 1. process alive → SetWindowsHookEx succeeded (install() asserts) --
if ($proc.HasExited) { throw "FAIL: mackey.exe exited early (code $($proc.ExitCode)) - hook install failed?" }
Write-Host "PASS: process resident, low-level hooks installed"

# -- 2. zero-config autostart: first run registered the scheduled task --
$null = schtasks /Query /TN MacKey 2>&1
if ($LASTEXITCODE -ne 0) { throw "FAIL: scheduled task 'MacKey' was not registered on first run" }
# the run level only shows up in the XML definition, not in /V text output
$taskXml = schtasks /Query /TN MacKey /XML | Out-String
if ($taskXml -notmatch "<RunLevel>HighestAvailable</RunLevel>") {
    throw "FAIL: task exists but RunLevel is not HighestAvailable:`n$taskXml"
}
Write-Host "PASS: Task Scheduler autostart registered with highest privileges"

# -- 3. live remapping through a real focused textbox --
$form = New-Object System.Windows.Forms.Form
$form.Text = "MacKey E2E"
$form.TopMost = $true
$tb = New-Object System.Windows.Forms.TextBox
$tb.Multiline = $true
$tb.Dock = "Fill"
$form.Controls.Add($tb)
$form.Show()
$form.Activate()
$tb.Focus()
[System.Windows.Forms.Application]::DoEvents()
Start-Sleep -Milliseconds 500

function Pump([int]$ms) {
    $end = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $end) {
        [System.Windows.Forms.Application]::DoEvents()
        Start-Sleep -Milliseconds 50
    }
}

# 3a. Alt+A → Ctrl+A (select all), Alt+C → Ctrl+C (copy) : clipboard proof
$tb.Text = "hello-mackey"
$tb.SelectionStart = 0; $tb.SelectionLength = 0
[System.Windows.Forms.Clipboard]::Clear()
Pump 300
[System.Windows.Forms.SendKeys]::SendWait("%a")   # Alt+A
Pump 400
[System.Windows.Forms.SendKeys]::SendWait("%c")   # Alt+C
Pump 600
$clip = [System.Windows.Forms.Clipboard]::GetText()
if ($clip -ne "hello-mackey") { throw "FAIL: Alt+A/Alt+C - clipboard='$clip' (expected 'hello-mackey')" }
Write-Host "PASS: Alt+A -> select all, Alt+C -> copy (clipboard verified)"

# 3b. Alt+V → Ctrl+V (paste at end)
$tb.SelectionStart = $tb.Text.Length; $tb.SelectionLength = 0
[System.Windows.Forms.SendKeys]::SendWait("%v")   # Alt+V
Pump 600
if ($tb.Text -ne "hello-mackeyhello-mackey") { throw "FAIL: Alt+V paste - text='$($tb.Text)'" }
Write-Host "PASS: Alt+V -> paste"

# 3c. Alt+Left → Home (caret to line start)
$tb.Text = "line-start-test"
$tb.SelectionStart = $tb.Text.Length
Pump 300
[System.Windows.Forms.SendKeys]::SendWait("%{LEFT}")   # Alt+Left
Pump 500
if ($tb.SelectionStart -ne 0) { throw "FAIL: Alt+Left - caret at $($tb.SelectionStart), expected 0" }
Write-Host "PASS: Alt+Left -> Home"

# 3d. Alt+Shift+Right → Shift+End (select to line end)
[System.Windows.Forms.SendKeys]::SendWait("%+{RIGHT}") # Alt+Shift+Right
Pump 500
if ($tb.SelectionLength -ne $tb.Text.Length) {
    throw "FAIL: Alt+Shift+Right - selected $($tb.SelectionLength)/$($tb.Text.Length) chars"
}
Write-Host "PASS: Alt+Shift+Right -> Shift+End (selection preserved)"

# 3e. Win+Left → Ctrl+Left (word-wise): caret jumps to previous word boundary
$tb.Text = "alpha beta"
$tb.SelectionStart = $tb.Text.Length
Pump 300
# SendKeys cannot express the Win key; emulate via raw SendInput through user32
Add-Type -Namespace Native -Name Input -MemberDefinition @"
[DllImport("user32.dll")] public static extern uint SendInput(uint n, INPUT[] p, int size);
[StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public KI ki; public long pad; }
[StructLayout(LayoutKind.Sequential)] public struct KI { public ushort vk; public ushort scan; public uint flags; public uint time; public IntPtr extra; }
"@
function Send-VK([ushort]$vk, [bool]$up) {
    $i = New-Object Native.Input+INPUT
    $i.type = 1
    $i.ki = New-Object Native.Input+KI
    $i.ki.vk = $vk
    $i.ki.flags = if ($up) { 2 } else { 0 }
    [Native.Input]::SendInput(1, @($i), [System.Runtime.InteropServices.Marshal]::SizeOf([type][Native.Input+INPUT])) | Out-Null
}
Send-VK 0x5B $false   # Win down
Start-Sleep -Milliseconds 80
Send-VK 0x25 $false   # Left down
Send-VK 0x25 $true    # Left up
Start-Sleep -Milliseconds 80
Send-VK 0x5B $true    # Win up
Pump 500
if ($tb.SelectionStart -ne 6) { throw "FAIL: Win+Left word-move - caret at $($tb.SelectionStart), expected 6" }
Write-Host "PASS: Win+Left -> Ctrl+Left (word-wise move, no Start menu)"

# -- 4. mac-only mode: Windows-native Ctrl shortcuts are gone --

# 4a. Ctrl+A → Home (emacs line start), NOT select-all
$tb.Text = "mac-only-mode"
$tb.SelectionStart = $tb.Text.Length; $tb.SelectionLength = 0
Pump 300
[System.Windows.Forms.SendKeys]::SendWait("^a")   # Ctrl+A
Pump 500
if ($tb.SelectionLength -ne 0) { throw "FAIL: Ctrl+A still selects all (windows-native alive)" }
if ($tb.SelectionStart -ne 0) { throw "FAIL: Ctrl+A - caret at $($tb.SelectionStart), expected 0 (Home)" }
Write-Host "PASS: Ctrl+A -> line start (emacs), not select-all"

# 4b. Ctrl+E → End
[System.Windows.Forms.SendKeys]::SendWait("^e")   # Ctrl+E
Pump 500
if ($tb.SelectionStart -ne $tb.Text.Length) { throw "FAIL: Ctrl+E - caret at $($tb.SelectionStart)" }
Write-Host "PASS: Ctrl+E -> line end (emacs)"

# 4c. Ctrl+C → dead (copy is Cmd/Alt+C now)
[System.Windows.Forms.Clipboard]::Clear()
$tb.SelectAll()
Pump 200
[System.Windows.Forms.SendKeys]::SendWait("^c")   # Ctrl+C
Pump 500
$clip2 = [System.Windows.Forms.Clipboard]::GetText()
if ($clip2 -ne "") { throw "FAIL: Ctrl+C still copies ('$clip2') - windows-native alive" }
Write-Host "PASS: Ctrl+C is dead (copy is Alt+C)"

$form.Close()
Stop-Process -Id $proc.Id -Force
schtasks /Delete /F /TN MacKey 2>&1 | Out-Null

Write-Host ""
Write-Host "== ALL E2E CHECKS PASSED on real Windows =="
