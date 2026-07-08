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
$elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Write-Host "== MacKey E2E on $([System.Environment]::OSVersion.VersionString) (elevated=$elevated) =="

# test mode via marker file — env vars don't reliably survive the launch of
# an elevated (requireAdministrator) process
$diagLog = Join-Path $env:TEMP "mackey-e2e.log"
Remove-Item $diagLog -ErrorAction SilentlyContinue
New-Item -ItemType File -Path (Join-Path $env:TEMP "mackey-test-mode") -Force | Out-Null
$env:MACKEY_TEST_TREAT_INJECTED = "1"

function Dump-Diag {
    Write-Host "---- mackey-e2e.log ----"
    if (Test-Path $diagLog) { Get-Content $diagLog | Select-Object -Last 80 | Write-Host }
    else { Write-Host "(no diagnostic log was written - test mode never activated?)" }
    Write-Host "------------------------"
}
trap { Dump-Diag; break }

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
Add-Type -AssemblyName System.Drawing
Add-Type -Namespace Native -Name Win -MemberDefinition @"
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
[DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, System.Text.StringBuilder s, int n);
"@

$form = New-Object System.Windows.Forms.Form
$form.Text = "MacKey E2E"
$form.TopMost = $true
$tb = New-Object System.Windows.Forms.TextBox
$tb.Multiline = $true
$tb.Dock = "Fill"
$form.Controls.Add($tb)
$form.Show()

function Pump([int]$ms) {
    $end = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $end) {
        [System.Windows.Forms.Application]::DoEvents()
        Start-Sleep -Milliseconds 50
    }
}

function Get-ForegroundTitle {
    $sb = New-Object System.Text.StringBuilder 256
    [Native.Win]::GetWindowText([Native.Win]::GetForegroundWindow(), $sb, 256) | Out-Null
    $sb.ToString()
}

# CI desktops don't hand out foreground focus willingly: retry, and fall back
# to physically clicking into the textbox (a real click always grants focus).
function Ensure-Focus {
    for ($i = 0; $i -lt 25; $i++) {
        [Native.Win]::SetForegroundWindow($form.Handle) | Out-Null
        $form.Activate()
        $null = $tb.Focus()
        Pump 150
        if ([Native.Win]::GetForegroundWindow() -eq $form.Handle -and $tb.Focused) { return }
        if ($i -ge 5) {
            $pt = $tb.PointToScreen([System.Drawing.Point]::new(20, 20))
            [Native.Win]::SetCursorPos($pt.X, $pt.Y) | Out-Null
            [Native.Win]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero) # left down
            [Native.Win]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero) # left up
            Pump 200
        }
    }
    throw "FAIL(setup): could not focus test window (foreground='$(Get-ForegroundTitle)')"
}

Ensure-Focus

# positive control: plain typing must reach the textbox before we blame the
# remapper for anything
$tb.Text = ""
[System.Windows.Forms.SendKeys]::SendWait("ping")
Pump 400
if ($tb.Text -ne "ping") {
    throw "FAIL(setup): SendKeys not reaching textbox (text='$($tb.Text)', foreground='$(Get-ForegroundTitle)')"
}
Write-Host "PASS: test window focused, synthetic input reaches it"

# 3a. Alt+A → Ctrl+A (select all), Alt+C → Ctrl+C (copy) : clipboard proof
$tb.Text = "hello-mackey"
$tb.SelectionStart = 0; $tb.SelectionLength = 0
[System.Windows.Forms.Clipboard]::Clear()
Ensure-Focus
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
Ensure-Focus
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
Ensure-Focus
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
Remove-Item (Join-Path $env:TEMP "mackey-test-mode") -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "== ALL E2E CHECKS PASSED on real Windows =="
