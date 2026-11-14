# The Windows smoke tests' driver. scripts/ui-smoke-windows.sh copies this folder to the test
# machine and runs it inside the desktop user's own session (admin.ps1 start registers it as a
# scheduled task there): a Windows service or an SSH session has no desktop to drive. It installs SuperEditr,
# then drives real reviews with SendInput key presses, reads windows through UI Automation and the
# browsers' pages through their DevTools ports, and reads what SuperEditr did from its capture.log.
#
#   driver.ps1 -Root C:\SmokeWin [-Only case,case]
#   driver.ps1 -Root C:\SmokeWin -Only warm-up     only opens the apps the tests use, once (--prepare)
#
# Root holds this folder's files, the composer page's bundle and setup.exe, the installer under
# test. Results go to Root\out: results.tsv (case, PASS or FAIL, check, milliseconds into the case,
# detail), summary.txt, shots\*.png, capture.log, and done.txt last.
#
# No AI and no sign-ins: reviews use the built-in rule-based checker (Proofread on English), and the
# Claude Code that Settings finds is fake-claude.cs, which is signed out and can't sign in.
#
# Keep this file ASCII: Windows PowerShell reads a script without a byte-order mark as the system
# code page.
param([string]$Root = 'C:\SmokeWin', [string]$Only = '')
$ErrorActionPreference = 'Continue'
$Only = $Only -replace ',', ' '
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$Out = "$Root\out"
$Shots = "$Out\shots"
New-Item -ItemType Directory -Force $Shots | Out-Null
Add-Type -AssemblyName System.Windows.Forms, System.Drawing, UIAutomationClient, UIAutomationTypes
# The helper programs: kept compiled on a prepared test machine, or compiled now (helpers.ps1).
. "$Root\helpers.ps1"
Add-Type -Path (Helper-Path $Root 'desktop.cs')
# DPI-aware, so screenshots and window sizes are in physical pixels at any display scaling.
[Desk]::SetProcessDPIAware() | Out-Null
[Desk]::ReleaseModifiers()

$App = "$env:LOCALAPPDATA\SuperEditr\supereditr.exe"
$Trace = "$env:LOCALAPPDATA\SuperEditr\Logs\capture.log"
$Settings = "$env:APPDATA\app.supereditr\settings.json"
$Sample = "Their going to love this, and it work good. See you tomorow."
$Chrome = 'C:\Program Files\Google\Chrome\Application\chrome.exe'
$Edge = 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
$Arrow = [string][char]0x2192

# Results ------------------------------------------------------------------------------------

$script:Case = ''
$script:CaseClock = [Diagnostics.Stopwatch]::StartNew()
$script:Step = 0
$script:Passed = 0
$script:Failed = 0
function Say($line) { $line | Add-Content "$Out\summary.txt" -Encoding UTF8 }
function Begin-Case($name, $what) {
  $script:Case = $name
  $script:CaseClock = [Diagnostics.Stopwatch]::StartNew()
  Say ''; Say "== ${name}: $what"
}
function End-Case { Say "   ($script:Case took $([int]$script:CaseClock.Elapsed.TotalSeconds) s)" }
# One line, short, with no tabs or line breaks, for the results file.
function Flat($text, $max = 200) {
  $t = ("$text" -replace '[\t\r\n]+', ' ').Trim()
  if ($t.Length -gt $max) { $t = $t.Substring(0, $max) + '...' }
  $t
}
function Check($name, $ok, $detail = '') {
  $status = if ($ok) { 'PASS' } else { 'FAIL' }
  if ($ok) { $script:Passed++ } else { $script:Failed++ }
  $d = Flat $detail 300
  Say ("   $status $name" + $(if ($d) { " - $d" } else { '' }))
  "$script:Case`t$status`t$name`t$([int]$script:CaseClock.Elapsed.TotalMilliseconds)`t$d" | Add-Content "$Out\results.tsv" -Encoding UTF8
}
function Note($text) { Say "   NOTE $(Flat $text 400)" }
function Wants($name) { -not $Only -or (" $Only " -like "* $name *") }
function Shot($label) {
  $script:Step++
  $name = ('{0:D2}-' -f $script:Step) + ($label -replace '[^A-Za-z0-9]+', '-').Trim('-')
  try { [Desk]::Shot("$Shots\$name.png") } catch { Note "no screenshot ${name}: $_" }
}

# capture.log --------------------------------------------------------------------------------

function Log-Lines { if (Test-Path $Trace) { @(Get-Content $Trace -Encoding UTF8) } else { @() } }
function Log-Mark { (Log-Lines).Count }
function Log-Since($mark) { @(Log-Lines | Select-Object -Skip $mark) }
# The first line after MARK that matches PATTERN, waiting up to SECS; '' when none.
function Wait-Log($mark, $pattern, $secs) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  do {
    $line = Log-Since $mark | Where-Object { $_ -match $pattern } | Select-Object -First 1
    if ($line) { return "$line" }
    Start-Sleep -Milliseconds 100
  } while ($sw.Elapsed.TotalSeconds -lt $secs)
  ''
}
# A log line without its timestamp.
function Bare($line) { "$line" -replace '^\d+\.\d+ ', '' }

# The clipboard ------------------------------------------------------------------------------

function Clip-Set($text) {
  for ($i = 0; $i -lt 20; $i++) { try { [Windows.Forms.Clipboard]::SetText($text); return } catch { Start-Sleep -Milliseconds 100 } }
  Note "couldn't set the clipboard"
}
function Clip-Text {
  for ($i = 0; $i -lt 20; $i++) { try { return [Windows.Forms.Clipboard]::GetText() } catch { Start-Sleep -Milliseconds 100 } }
  '(clipboard unreadable)'
}

# Windows and UI Automation ------------------------------------------------------------------

$Tree = [Windows.Automation.TreeScope]::Descendants
$Any = [Windows.Automation.Condition]::TrueCondition
function Ui-All($hwnd) {
  try { @([Windows.Automation.AutomationElement]::FromHandle($hwnd).FindAll($Tree, $Any)) } catch { @() }
}
# Every name in a window's UI Automation tree, joined: what the window shows.
function Ui-Text($hwnd) {
  if ($hwnd -eq [IntPtr]::Zero) { return '' }
  (Ui-All $hwnd | ForEach-Object { try { $_.Current.Name } catch { } } | Where-Object { $_ }) -join ' | '
}
function Ui-Find($hwnd, $pattern, $type) {
  Ui-All $hwnd | Where-Object { try { $_.Current.Name -like $pattern -and (-not $type -or $_.Current.ControlType -eq $type) } catch { $false } } | Select-Object -First 1
}
function Ui-Invoke($el) {
  try { $el.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke(); $true } catch { Note "invoke failed: $_"; $false }
}
$ButtonType = [Windows.Automation.ControlType]::Button
$EditType = [Windows.Automation.ControlType]::Edit

function Panel { [Desk]::Exact('supereditr', 'SuperEditr') }
function Panel-Text { Ui-Text (Panel) }
# Milliseconds until the panel shows with the keyboard, or -1.
function Wait-Panel($secs) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  do {
    $p = Panel
    if ($p -ne [IntPtr]::Zero -and [Desk]::GetForegroundWindow() -eq $p) { return [int]$sw.Elapsed.TotalMilliseconds }
    Start-Sleep -Milliseconds 30
  } while ($sw.Elapsed.TotalSeconds -lt $secs)
  -1
}
# The panel's text once it matches PATTERN, or its last text.
function Wait-PanelText($pattern, $secs) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  do {
    $t = Panel-Text
    if ($t -match $pattern) { return $t }
    Start-Sleep -Milliseconds 200
  } while ($sw.Elapsed.TotalSeconds -lt $secs)
  $t
}
function Wait-Closed($secs) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  do { if ((Panel) -eq [IntPtr]::Zero) { return $true }; Start-Sleep -Milliseconds 50 } while ($sw.Elapsed.TotalSeconds -lt $secs)
  $false
}
function Wait-Window($process, $prefix, $secs) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  do {
    $w = @([Desk]::Windows($process, $prefix))
    if ($w.Count) { return $w[0] }
    Start-Sleep -Milliseconds 300
  } while ($sw.Elapsed.TotalSeconds -lt $secs)
  [IntPtr]::Zero
}
function Close-App($name) {
  Get-Process $name -ErrorAction SilentlyContinue | Stop-Process -Force
  for ($i = 0; $i -lt 30 -and (Get-Process $name -ErrorAction SilentlyContinue); $i++) { Start-Sleep -Milliseconds 200 }
}
function Key($chord) { [Desk]::Press($chord); Start-Sleep -Milliseconds 150 }

# SuperEditr ---------------------------------------------------------------------------------

function App-Start { Start-Process $App | Out-Null }
function App-Quit { Close-App supereditr }
function Settings-Window { [Desk]::Exact('supereditr', 'SuperEditr Settings') }
# Presses a button in Settings by its name, waiting for the page to show it.
function Settings-Press($pattern, $secs = 10) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  do {
    $b = Ui-Find (Settings-Window) $pattern $ButtonType
    if ($b) { return (Ui-Invoke $b) }
    Start-Sleep -Milliseconds 500
  } while ($sw.Elapsed.TotalSeconds -lt $secs)
  Note "no button '$pattern' in Settings"
  $false
}
function Wait-SettingsText($pattern, $secs) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  do {
    $t = Ui-Text (Settings-Window)
    if ($t -like $pattern) { return $t }
    Start-Sleep -Milliseconds 500
  } while ($sw.Elapsed.TotalSeconds -lt $secs)
  $t
}

# review LABEL IMAGE SHOT [apply|copy]: presses the review shortcut over the selection in the window
# in front, waits for a fix in the panel, then presses Alt+Enter. In copy mode Alt+Enter must say
# nothing was replaced and leave the clipboard alone, and Ctrl+C copies the fix. Sets $script:Read
# (capture.log's line for reading the selection), $script:Shown (the panel's text with the fix) and
# $script:Result (capture.log's line for the replace).
function Review($label, $image, $shot, $mode = 'apply', $beforeApply = $null) {
  $script:Read = ''; $script:Result = ''; $script:Shown = ''
  $img = [regex]::Escape($image)
  $mark = Log-Mark
  Key 'ctrl+shift+f2'
  $ms = Wait-Panel 6
  Check "${label}: the review shortcut opens the panel with the keyboard" ($ms -ge 0) "after $ms ms"
  $script:Read = Bare (Wait-Log $mark " shortcut ${img}: " 4)
  if ($ms -lt 0) { Shot "$shot-no-panel"; return }
  $text = Wait-PanelText '\d+ changes?' 15
  $script:Shown = $text
  Check "${label}: the panel shows a fix" ($text -match '\d+ changes?') $text
  Shot "$shot-review"
  if ($mode -eq 'copy') {
    $before = Clip-Text
    Key 'alt+enter'
    $text = Wait-PanelText 'Nothing replaced' 3
    Check "${label}: Alt+Enter says nothing was replaced" ($text -match 'Nothing replaced') $text
    Shot "$shot-alt-enter"
    $after = Clip-Text
    Check "${label}: Alt+Enter leaves the clipboard alone" ($after -eq $before) "clipboard: $after"
    Key 'ctrl+c'
    $text = Wait-PanelText 'Copied' 3
    Check "${label}: Ctrl+C in the panel copies the fix" ($text -match 'Copied') $text
    Key 'esc'
    $script:Result = 'copy'
  } else {
    if ($beforeApply) { & $beforeApply }
    Key 'alt+enter'
    # A focus that didn't come back ends the apply before anything is replaced.
    $script:Result = Bare (Wait-Log $mark " replace ${img}: |^\d+\.\d+ focus not back" 6)
  }
  Wait-Closed 3 | Out-Null
  Start-Sleep -Milliseconds 400
}

# Cases --------------------------------------------------------------------------------------

function Install-Case {
  Begin-Case install 'install SuperEditr per user, with a signed-out Claude Code to find'
  App-Quit
  $old = "$env:LOCALAPPDATA\SuperEditr\uninstall.exe"
  if (Test-Path $old) {
    Note 'SuperEditr was already installed: uninstalling it first'
    Start-Process $old -ArgumentList '/S' -Wait
    Start-Sleep 3
  }
  Remove-Item -Recurse -Force "$env:APPDATA\app.supereditr", "$env:LOCALAPPDATA\app.supereditr", "$env:LOCALAPPDATA\SuperEditr" -ErrorAction SilentlyContinue
  # The Claude desktop app's own copy of Claude Code lives here; the stand-in is signed out.
  $claude = "$env:APPDATA\Claude\claude-code\2.1.0"
  Remove-Item -Recurse -Force "$env:APPDATA\Claude" -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force $claude | Out-Null
  try { Copy-Item (Helper-Path $Root 'fake-claude.cs') "$claude\claude.exe" -ErrorAction Stop }
  catch { Note "couldn't build the stand-in Claude Code: $_" }
  Check 'a signed-out Claude Code is where the Claude app keeps its own' (Test-Path "$claude\claude.exe") "$claude\claude.exe"
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $p = Start-Process "$Root\setup.exe" -ArgumentList '/S' -Wait -PassThru
  $version = (Get-Item $App -ErrorAction SilentlyContinue).VersionInfo.ProductVersion
  Check 'the installer installs per user without asking' ($p.ExitCode -eq 0 -and (Test-Path $App)) "exit $($p.ExitCode) in $([int]$sw.Elapsed.TotalSeconds) s, version $version"
  Check 'the install is recorded for this user only' ((Test-Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\SuperEditr') -and -not (Test-Path 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\SuperEditr'))
  Start-Sleep 2
  App-Quit
  End-Case
  Test-Path $App
}

function FirstRun-Case {
  Begin-Case first-run 'first launch, Settings with no AI, Find my AI accounts, and the next start'
  App-Start
  $s = [IntPtr]::Zero
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($s -eq [IntPtr]::Zero -and $sw.Elapsed.TotalSeconds -lt 20) { Start-Sleep -Milliseconds 300; $s = Settings-Window }
  Check 'the first launch opens Settings' ($s -ne [IntPtr]::Zero) "after $([int]$sw.Elapsed.TotalMilliseconds) ms"
  if ($s -ne [IntPtr]::Zero) {
    [Desk]::Front($s) | Out-Null
    Settings-Press 'Styles' 20 | Out-Null
    $t = Wait-SettingsText '*Styles other than Proofread need an AI*' 10
    # On a slow first launch the page's text can show a moment before its button.
    $setUp = $null
    for ($i = 0; $i -lt 20 -and -not $setUp; $i++) { $setUp = Ui-Find $s 'Set up AI' $ButtonType; if (-not $setUp) { Start-Sleep -Milliseconds 250 } }
    Check 'with no AI, Styles says only Proofread works, and offers Set up AI' ($t -like '*Styles other than Proofread need an AI*' -and $setUp) $(if ($t -like '*need an AI*') { 'Styles other than Proofread need an AI.' } else { $t })
    Shot 'styles-no-ai'
    Settings-Press 'Get Started' | Out-Null
    $t = Wait-SettingsText '*No AI set up*' 10
    Check 'with no AI, Get Started says so and offers Set up AI' ($t -like '*No AI set up*' -and $t -like '*Set up AI*') $(if ($t -like '*No AI set up*') { ($t -split ' \| ' | Where-Object { $_ -like '*No AI set up*' } | Select-Object -First 1) } else { $t })
    Shot 'get-started-no-ai'
    # Find my AI accounts finds the stand-in. Its Use saves it and checks it, which asks Claude Code
    # whether it's signed in; it isn't, so the card offers to sign in instead of putting it in use.
    $found = Settings-Press 'Find my AI accounts*'
    Check "Get Started's Find my AI accounts can be pressed" $found
    $t = Wait-SettingsText '*Claude Code from the Claude app*' 20
    Check "Models finds the Claude app's own Claude Code" ($t -like '*Claude Code from the Claude app*') $(if ($t -like '*Claude Code from the Claude app*') { 'Claude Code from the Claude app' } else { $t })
    Settings-Press 'Use' | Out-Null
    $t = Wait-SettingsText '*signed in to your Claude account*' 20
    $signIn = Ui-Find $s 'Sign in to Claude' $ButtonType
    Check "the found Claude Code's card says it isn't signed in" ($t -like '*signed in to your Claude account*') $(if ($t -like '*signed in to your Claude*') { ($t -split ' \| ' | Where-Object { $_ -like '*signed in to your Claude*' } | Select-Object -First 1) } else { $t })
    Check 'the card offers Sign in to Claude' ([bool]$signIn) $(if ($signIn) { "button '$($signIn.Current.Name)'" } else { '' })
    Shot 'models-claude-sign-in-card'
    $calls = @(Get-Content "$env:APPDATA\Claude\claude-code\2.1.0\calls.log" -ErrorAction SilentlyContinue)
    Check 'Claude Code was asked whether it is signed in, and nothing tried to sign in' (($calls -match 'auth status') -and -not ($calls -match 'login')) "the stand-in was run as: $(($calls | ForEach-Object { ($_ -split ' ', 2)[1] } | Sort-Object -Unique) -join '; ')"
    # Done on Get Started ends the first run.
    Settings-Press 'Get Started' | Out-Null
    Start-Sleep 1
    Check "Get Started's Done can be pressed" (Settings-Press 'Done')
    Start-Sleep 1
  }
  # The tests need Windows Terminal reviewed (copy-only) rather than skipped as an excluded app, and
  # capture.log on.
  App-Quit
  $j = if (Test-Path $Settings) { Get-Content -Raw $Settings -Encoding UTF8 | ConvertFrom-Json } else { New-Object PSObject }
  $j | Add-Member -Force NoteProperty diagnostic_log $true
  if ($j.PSObject.Properties['excluded_apps']) {
    $j.excluded_apps = @($j.excluded_apps | Where-Object { $_ -ne 'WindowsTerminal.exe' })
  } else {
    $defaults = @('1Password.exe', 'Bitwarden.exe', 'KeePassXC.exe', 'KeePass.exe', 'NordPass.exe', 'WindowsTerminal.exe', 'OpenConsole.exe',
      'conhost.exe', 'pwsh.exe', 'powershell.exe', 'cmd.exe', 'wezterm-gui.exe', 'alacritty.exe', 'mintty.exe')
    $j | Add-Member -Force NoteProperty excluded_apps @($defaults | Where-Object { $_ -ne 'WindowsTerminal.exe' })
    $j | Add-Member -Force NoteProperty offered_exclusions $defaults
  }
  New-Item -ItemType Directory -Force (Split-Path $Settings) | Out-Null
  [IO.File]::WriteAllText($Settings, ($j | ConvertTo-Json -Depth 50), (New-Object Text.UTF8Encoding $false))
  App-Start
  Start-Sleep 6
  $s = Settings-Window
  Check 'the next start doesn''t open Settings' ($s -eq [IntPtr]::Zero)
  if ($s -ne [IntPtr]::Zero) { [Desk]::Close($s) }
  End-Case
}

function Notepad-Open($text) {
  Close-App notepad
  # Notepad reopens earlier tabs and their unsaved text unless its saved session is gone.
  Remove-Item "$env:LOCALAPPDATA\Packages\Microsoft.WindowsNotepad_8wekyb3d8bbwe\LocalState\TabState\*", "$env:LOCALAPPDATA\Packages\Microsoft.WindowsNotepad_8wekyb3d8bbwe\LocalState\WindowState\*" -Force -ErrorAction SilentlyContinue
  [IO.File]::WriteAllText("$Root\smoke-sample.txt", $text, (New-Object Text.UTF8Encoding $false))
  Start-Process notepad -ArgumentList "$Root\smoke-sample.txt"
  # Notepad's first start after the machine was rolled back took over 15 s once.
  $w = Wait-Window notepad 'smoke-sample' 40
  Start-Sleep 1
  [Desk]::Front($w) | Out-Null
  # Its document answers UI Automation once it has loaded: until then a review's read times out.
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ((Notepad-Text $w) -ne $text -and $sw.Elapsed.TotalSeconds -lt 10) { Start-Sleep -Milliseconds 250 }
  $w
}
# Notepad's document text, through UI Automation.
function Notepad-Text($w) {
  try {
    $c = New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::ClassNameProperty), 'RichEditD2DPT'
    $doc = [Windows.Automation.AutomationElement]::FromHandle($w).FindFirst($Tree, $c)
    $doc.GetCurrentPattern([Windows.Automation.TextPattern]::Pattern).DocumentRange.GetText(-1).TrimEnd("`r", "`n")
  } catch { "(unreadable: $_)" }
}

function Notepad-Case {
  Begin-Case notepad 'Notepad: applied, verified, the clipboard put back, one Ctrl+Z undoes it'
  $w = Notepad-Open $Sample
  Check 'Notepad opens with the text' ((Notepad-Text $w) -eq $Sample) (Notepad-Text $w)
  Key 'ctrl+a'
  Clip-Set 'Brynlee copied this earlier.'
  Review 'Notepad' 'Notepad.exe' 'notepad'
  Check 'Notepad: read through UI Automation, known safe' ($script:Read -match 'via UI Automation.*KnownSafe') $script:Read
  Check 'Notepad: Alt+Enter applies, verified' ($script:Result -match ': applied') $script:Result
  $t = Notepad-Text $w
  Check 'Notepad: the document holds the fix' ($t -like "*They're going*" -and $t -like '*tomorrow*') $t
  Check 'Notepad: the clipboard is put back' ((Clip-Text) -eq 'Brynlee copied this earlier.') (Clip-Text)
  Shot 'notepad-applied'
  [Desk]::Front($w) | Out-Null
  Key 'ctrl+z'
  Start-Sleep -Milliseconds 500
  $t = Notepad-Text $w
  Check 'Notepad: one Ctrl+Z puts the original back' ($t -eq $Sample) $t
  Shot 'notepad-undone'
  End-Case
}

# Where the panel opened against where the selection is, in physical pixels at the screen's scale:
# the card (inside the window's transparent margin) starts at the selection's left edge, just below it.
function Placement-Check($w) {
  $p = Panel
  $scale = [Desk]::GetDpiForWindow($p) / 96.0
  try {
    $sel = (Notepad-Pattern $w).GetSelection()[0].GetBoundingRectangles()[0]
  } catch { $sel = $null }
  $r = New-Object Desk+RECT
  [Desk]::GetWindowRect($p, [ref]$r) | Out-Null
  $card = @{ X = $r.Left + [Math]::Round(26 * $scale); Y = $r.Top + [Math]::Round(16 * $scale) }
  $detail = "scale $scale; window $($r.Right - $r.Left)x$($r.Bottom - $r.Top) at $($r.Left),$($r.Top); card at $($card.X),$($card.Y); selection $(if ($sel) { "$($sel.Left),$($sel.Top) to $($sel.Right),$($sel.Bottom)" })"
  Note "placement: $detail"
  Check "the panel is 548 logical pixels wide at the screen's scale" ([Math]::Abs(($r.Right - $r.Left) - [Math]::Round(548 * $scale)) -le 1) $detail
  Check 'the card opens at the selection''s left edge, just below it' ($sel -and [Math]::Abs($card.X - $sel.Left) -le 2 -and [Math]::Abs($card.Y - ($sel.Bottom + 8 * $scale)) -le 2) $detail
}

function Keys-Case {
  Begin-Case keys "the panel's keys: Tab, Space, digits, / and Esc while typing an instruction"
  $w = Notepad-Open $Sample
  Key 'ctrl+a'
  Key 'ctrl+shift+f2'
  $ms = Wait-Panel 6
  Check 'the review shortcut opens the panel with the keyboard' ($ms -ge 0) "after $ms ms"
  $text = Wait-PanelText '\d+ changes?' 15
  Check 'the panel shows a fix' ($text -match '\d+ changes?') $text
  Placement-Check $w
  # A change shows as the old and new text side by side ("TheirThey're" in the tree); stepping to it
  # with Tab adds a line that describes it, and Space keeps the original ("Their" alone).
  Key 'tab'
  $text = Wait-PanelText "Did you mean|$Arrow" 3
  Check 'Tab steps to a change and describes it' ($text -match "Did you mean|$Arrow") $text
  Shot 'keys-tab'
  Key 'space'
  $text = Wait-PanelText '\| Their \|' 3
  Check 'Space keeps the original for that change' ($text -match '\| Their \|' -and $text -notmatch "TheirThey're") $text
  Shot 'keys-space'
  Key 'space'
  $text = Wait-PanelText "TheirThey're" 3
  Check 'Space again takes the suggestion back' ($text -match "TheirThey're") $text
  Key '2'
  Start-Sleep 2
  Note "with no AI, 2 (Polish) shows: $(Panel-Text)"
  Shot 'keys-digit-2'
  Key '1'
  Start-Sleep 1
  $text = Wait-PanelText '\d+ changes?' 15
  Check '1 runs Proofread again and shows a fix' ($text -match '\d+ changes?') $text
  Shot 'keys-digit-1'
  Key 'slash'
  Start-Sleep -Milliseconds 500
  $edit = Ui-Find (Panel) '*' $EditType
  Check '/ opens the instruction box' ([bool]$edit) $(if ($edit) { $edit.Current.Name })
  [Desk]::Type('make it shorter')
  Start-Sleep -Milliseconds 400
  $typed = try { $edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value } catch { '' }
  Check 'typing goes into the instruction box' ($typed -eq 'make it shorter') "box holds '$typed'"
  Shot 'keys-typing-instruction'
  Key 'esc'
  Start-Sleep -Milliseconds 600
  $panel = Panel
  $edit = if ($panel -ne [IntPtr]::Zero) { Ui-Find $panel '*' $EditType }
  Check 'Esc while typing an instruction cancels it' ($panel -ne [IntPtr]::Zero -and -not $edit) $(if ($edit) { 'the box is still there' })
  Check "Esc while typing an instruction doesn't close the panel" ($panel -ne [IntPtr]::Zero -and [Desk]::IsWindowVisible($panel)) (Panel-Text)
  Shot 'keys-esc-cancelled-instruction'
  Key 'esc'
  Check 'Esc again closes the panel' (Wait-Closed 3)
  $t = Notepad-Text $w
  Check 'nothing reached Notepad' ($t -eq $Sample) $t
  # The text may have been selected for something else: closing the panel leaves it selected.
  Start-Sleep -Milliseconds 500
  $still = try { (Notepad-Pattern $w).GetSelection()[0].GetText(-1).TrimEnd("`r", "`n") } catch { "(unreadable: $_)" }
  Check 'Esc leaves the text selected in Notepad' ($still -eq $Sample) "selected: '$still'"
  Shot 'keys-closed'
  Close-App notepad
  End-Case
}

# Browsers -----------------------------------------------------------------------------------

$script:Port = 9222
function Cdp($expression) {
  try {
    # Windows PowerShell passes a JSON array on as one object: unroll it before filtering.
    $page = Invoke-RestMethod "http://127.0.0.1:$script:Port/json/list" -TimeoutSec 5 | ForEach-Object { $_ } | Where-Object { $_.type -eq 'page' -and $_.url -like '*fields.html*' } | Select-Object -First 1
    if (-not $page) { return '(no page)' }
    $answer = [Desk]::Evaluate($page.webSocketDebuggerUrl, $expression, 15000) | ConvertFrom-Json
    $answer.result.result.value
  } catch {
    $e = $_.Exception
    while ($e.InnerException) { $e = $e.InnerException }
    "(cdp failed: $($e.Message))"
  }
}
function Cdp-Select($id) {
  Cdp "(() => { const f = document.getElementById('$id'); f.scrollIntoView({block: 'center'}); f.focus();
    if ('select' in f && f.tagName !== 'DIV') f.select();
    else { const r = document.createRange(); r.selectNodeContents(f); const s = getSelection(); s.removeAllRanges(); s.addRange(r); }
    return document.activeElement === f || f.contains(document.activeElement); })()"
}
function Cdp-Value($id) { Cdp "(() => { const f = document.getElementById('$id'); return 'value' in f && f.tagName !== 'DIV' ? f.value : f.innerText; })()" }

# Opens the composers page in a browser with a fresh profile and its DevTools port, and builds its
# accessibility tree; its window.
function Browser-Open($name, $exe, $process, $port) {
  Close-App $process
  $script:Port = $port
  $prof = "$env:TEMP\smoke-$name-profile"
  Remove-Item -Recurse -Force $prof -ErrorAction SilentlyContinue
  $url = 'file:///' + ($Root -replace '\\', '/') + '/fields.html'
  Start-Process $exe -ArgumentList "--remote-debugging-port=$port", "--user-data-dir=$prof", '--no-first-run', '--no-default-browser-check', '--disable-sync', '--remote-allow-origins=*', $url
  $ready = ''
  for ($i = 0; $i -lt 60 -and $ready -ne 'complete'; $i++) { Start-Sleep -Milliseconds 500; $ready = Cdp 'document.readyState' }
  $w = Wait-Window $process 'SuperEditr smoke fields' 15
  Check "$name opens the test page" ($ready -eq 'complete' -and $w -ne [IntPtr]::Zero) "page $ready; window $([Desk]::Title($w))"
  Start-Sleep 2
  # The browser builds its accessibility tree when a client first asks. On this machine, without a
  # GPU, that took up to 3 s, past the first review's read, which then fell back to a copy. These
  # cases are about applying fixes, so the tree is built first, as any accessibility client would.
  [Desk]::Front($w) | Out-Null
  Cdp-Select 'plain' | Out-Null
  $sw = [Diagnostics.Stopwatch]::StartNew()
  do {
    $type = try { [Windows.Automation.AutomationElement]::FocusedElement.Current.ControlType } catch { $null }
    if ($type -eq $EditType) { break }
    Start-Sleep -Milliseconds 200
  } while ($sw.Elapsed.TotalSeconds -lt 15)
  Note "$name's accessibility answered with the focused field after $([int]$sw.Elapsed.TotalMilliseconds) ms"
  $w
}
function Browser-Case($name, $exe, $process, $image, $port) {
  Begin-Case $name "the composers page in $name, read through UI Automation and pasted; a password field refused"
  $w = Browser-Open $name $exe $process $port
  Shot "$name-page"
  foreach ($id in 'plain', 'rich', 'code', 'markdown', 'tiptap', 'lexical') {
    $clip = "Kyndal's own copy before $id."
    Clip-Set $clip
    [Desk]::Front($w) | Out-Null
    $selected = Cdp-Select $id
    if ($selected -ne $true) { Note "${id}: couldn't select the field ($selected)" }
    Start-Sleep -Milliseconds 500
    Review "$name $id" $image "$name-$id"
    Check "$name ${id}: read through UI Automation, known safe" ($script:Read -match 'via UI Automation.*KnownSafe') $script:Read
    Check "$name ${id}: Alt+Enter applies, verified" ($script:Result -match ': applied') $script:Result
    $value = Cdp-Value $id
    if ($id -in 'plain', 'rich') {
      Check "$name ${id}: the field holds the fix" ($value -like "*They're going*" -and $value -notlike '*Their going*') $value
    } else {
      Check "$name ${id}: both fixes land" ($value -like "*They're going to love*" -and $value -like '*really run*') $value
      $state = Cdp "document.getElementById('$id-state').textContent"
      Note "${id}: the page describes the field as $state"
      # A plain-text paste into ProseMirror or Lexical takes the inline code at the selection's start,
      # which turned the whole fix into code; the fix also goes on the clipboard as HTML to prevent it.
      $code = if ("$state" -match 'code=([^;]*)') { $Matches[1] } else { '' }
      Check "$name ${id}: the fix isn't turned into code" ("$state" -match 'code=' -and $code -notmatch 'love this|See you soon') "code: '$code'"
    }
    Check "$name ${id}: the clipboard is put back" ((Clip-Text) -eq $clip) (Clip-Text)
    Shot "$name-$id-applied"
  }
  # Refused: a password field. Nothing is read, so the panel says why and the clipboard stays.
  Clip-Set 'Rhyan copied this.'
  [Desk]::Front($w) | Out-Null
  Cdp-Select 'password' | Out-Null
  Start-Sleep -Milliseconds 500
  $mark = Log-Mark
  Key 'ctrl+shift+f2'
  $text = Wait-PanelText 'password' 5
  $line = Bare (Wait-Log $mark " shortcut $([regex]::Escape($image)): " 3)
  Shot "$name-password"
  Check "$name password: the review is refused" ($text -match 'password' -and $text -notmatch '\d+ changes?' -and $line -match 'password') "panel: $text; log: $line"
  Key 'esc'
  Wait-Closed 3 | Out-Null
  Check "$name password: the clipboard is untouched" ((Clip-Text) -eq 'Rhyan copied this.') (Clip-Text)
  Close-App $process
  End-Case
}

function Terminal-Case {
  Begin-Case terminal 'Windows Terminal: read with a guarded copy, copy-only'
  $before = @(Get-Process WindowsTerminal -ErrorAction SilentlyContinue | ForEach-Object Id)
  Start-Process wt.exe -ArgumentList '--title', 'SmokeTerminal', 'cmd.exe', '/k', 'echo Their going to love this, and it work good.'
  $w = Wait-Window WindowsTerminal '' 15
  Start-Sleep 3
  [Desk]::Front($w) | Out-Null
  Check 'Windows Terminal opens' ($w -ne [IntPtr]::Zero -and [Desk]::GetForegroundWindow() -eq $w) "window '$([Desk]::Title($w))'"
  # Windows Terminal's own Select all.
  Key 'ctrl+shift+a'
  Start-Sleep -Milliseconds 500
  Clip-Set 'Rhyan copied this before the terminal.'
  $mark = Log-Mark
  Review 'Windows Terminal' 'WindowsTerminal.exe' 'terminal' 'copy'
  Check 'Windows Terminal: read, copy-only' ($script:Read -match 'read \d+ characters' -and $script:Read -match 'copy-only|CopyOnly') $script:Read
  # The spaces that pad each line to the window's width aren't changes.
  $count = if ($script:Shown -match '(\d+) changes?') { [int]$Matches[1] } else { -1 }
  Check "Windows Terminal: only the sentence's fix counts as a change" ($count -eq 1) "$count changes"
  $clip = Clip-Text
  Check 'Windows Terminal: Ctrl+C copied the fix' ($clip -like "*They're going*") "clipboard: $clip"
  $replaced = @(Log-Since $mark | Where-Object { $_ -match ' replace WindowsTerminal\.exe' })
  Check 'Windows Terminal: nothing is pasted into the terminal' ($replaced.Count -eq 0) ($replaced -join '; ')
  Shot 'terminal-after'
  Get-Process WindowsTerminal -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id } | Stop-Process -Force
  End-Case
}

# Races --------------------------------------------------------------------------------------

$script:PadExe = ''
# Starts racepad.cs's editor titled "RacePad NAME" holding TEXT, with EXTRA arguments; its window.
function Pad-Open($name, $text, [string[]]$extra = @()) {
  if (-not $script:PadExe) {
    try { $script:PadExe = Helper-Path $Root 'racepad.cs' }
    catch { Note "couldn't build the test editor: $_" }
  }
  $file = "$Root\pad-$name.txt"
  [IO.File]::WriteAllText($file, $text, (New-Object Text.UTF8Encoding $false))
  Remove-Item "$file.out" -ErrorAction SilentlyContinue
  Start-Process $PadExe -ArgumentList (@($name, $file) + $extra)
  $w = Wait-Window racepad "RacePad $name" 15
  Start-Sleep -Milliseconds 500
  [Desk]::Front($w) | Out-Null
  $w
}
# What the editor holds, from the file it writes on every change: readable even while it hangs.
function Pad-Text($name) {
  try { [IO.File]::ReadAllText("$Root\pad-$name.txt.out", [Text.Encoding]::UTF8) } catch { '(unreadable)' }
}
function Pad-Close { Close-App racepad }
function Panel-Close {
  if ((Panel) -ne [IntPtr]::Zero) { Key 'esc'; Wait-Closed 3 | Out-Null }
}
function Notepad-Pattern($w) {
  $c = New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::ClassNameProperty), 'RichEditD2DPT'
  [Windows.Automation.AutomationElement]::FromHandle($w).FindFirst($Tree, $c).GetCurrentPattern([Windows.Automation.TextPattern]::Pattern)
}
# Selects the first or the last copy of TEXT in Notepad through UI Automation, as the user would with
# the mouse; returns the selection's text and where it starts, as "text@offset".
function Notepad-Select($w, $text, [switch]$Last) {
  try {
    $tp = Notepad-Pattern $w
    $r = $tp.DocumentRange.FindText($text, [bool]$Last, $false)
    $r.Select()
    Start-Sleep -Milliseconds 200
    $sel = $tp.GetSelection()[0]
    $before = $tp.DocumentRange.Clone()
    $before.MoveEndpointByRange([Windows.Automation.Text.TextPatternRangeEndpoint]::End, $sel, [Windows.Automation.Text.TextPatternRangeEndpoint]::Start)
    "$($sel.GetText(-1))@$($before.GetText(-1).Length)"
  } catch { "(couldn't select: $_)" }
}
# Whether TEXT is the sample with its fixes, once: a second paste would add another copy.
function Fixed-Once($text) {
  [regex]::Matches("$text", "They're going").Count -eq 1 -and "$text" -notmatch 'Their' -and "$text" -match 'tomorrow\.$'
}
# The outcome capture.log gave the latest apply: applied, uncertain, copy-only or clipboard kept.
function Outcome($line) {
  if ($line -match ': applied in ') { 'applied' }
  elseif ($line -match ': uncertain ') { 'uncertain' }
  elseif ($line -match ': clipboard kept ') { 'clipboard kept' }
  elseif ($line -match ': copy-only |focus not back') { 'copy-only' }
  else { "none ($line)" }
}

function Races-Case {
  Begin-Case races 'races while a fix is applied: duplicate text, the focus moving, a copy, a late paste, a hung app'

  # Duplicate text: the second of two identical sentences is fixed, and only it.
  $twice = 'Their going to love this. Their going to love this.'
  $w = Notepad-Open $twice
  $sel = Notepad-Select $w 'Their going to love this.' -Last
  Check 'duplicate: the second of two identical sentences is selected' ($sel -eq 'Their going to love this.@26') $sel
  Clip-Set 'Brynlee copied this before the duplicate.'
  Review 'duplicate' 'Notepad.exe' 'race-duplicate'
  Check 'duplicate: the fix is applied' ((Outcome $script:Result) -eq 'applied') $script:Result
  $t = Notepad-Text $w
  Check 'duplicate: only the selected sentence changed' ($t -eq "Their going to love this. They're going to love this.") $t
  Check 'duplicate: the clipboard is put back' ((Clip-Text) -eq 'Brynlee copied this before the duplicate.') (Clip-Text)
  # The same sentence, but the user selects the first copy while the panel is open: nothing is
  # written, and SuperEditr doesn't go looking for the text elsewhere.
  $w = Notepad-Open $twice
  Notepad-Select $w 'Their going to love this.' -Last | Out-Null
  Review 'duplicate moved' 'Notepad.exe' 'race-duplicate-moved' 'apply' {
    $script:Moved = Notepad-Select $w 'Their going to love this.'
  }
  Check 'duplicate moved: the first copy was selected while the panel was open' ($script:Moved -eq 'Their going to love this.@0') $script:Moved
  Check 'duplicate moved: nothing is replaced' ((Outcome $script:Result) -eq 'copy-only' -and $script:Result -match 'changed since') $script:Result
  $t = Notepad-Text $w
  Check 'duplicate moved: neither sentence changed' ($t -eq $twice) $t
  Panel-Close

  # The focus moves while the fix is applied. A decoy window comes to the front as the panel closes,
  # once, then again and again for 1.5 s. Whatever happens, the decoy never gets a paste, and Notepad
  # holds either the fix or its own text.
  $decoyText = 'Kyndal is typing here, and nothing should be pasted into it.'
  $decoy = Pad-Open 'decoy' $decoyText
  foreach ($steal in @(@{ Name = 'once'; Hold = 0 }, @{ Name = 'repeatedly'; Hold = 1500 })) {
    $label = "focus moved $($steal.Name)"
    $w = Notepad-Open $Sample
    Key 'ctrl+a'
    Clip-Set "Rhyan copied this before the focus moved $($steal.Name)."
    $hold = $steal.Hold
    Review $label 'Notepad.exe' "race-focus-$($steal.Name)" 'apply' {
      [Desk]::RaceAfterPanelHides((Panel), 0, $decoy, $hold, [NullString]::Value)
    }
    Start-Sleep -Milliseconds ([Math]::Max(0, $hold - 500))
    $outcome = Outcome $script:Result
    Note "${label}: $([Desk]::RaceReport()); outcome $outcome"
    Check "${label}: nothing is pasted into the window that took the focus" ((Pad-Text 'decoy') -eq $decoyText) (Pad-Text 'decoy')
    $t = Notepad-Text $w
    Check "${label}: Notepad holds the fix only if it was applied, and its own text otherwise" (($outcome -eq 'applied' -and (Fixed-Once $t)) -or ($outcome -eq 'copy-only' -and $t -eq $Sample)) "$outcome; $t"
    Shot "race-focus-$($steal.Name)-after"
    Panel-Close
  }
  Pad-Close

  # The user copies something while the fix is applied, at several moments. Their copy is never
  # pasted, and it's what the clipboard holds afterwards, whatever SuperEditr was doing then.
  foreach ($delay in 0, 20, 40, 70, 150) {
    $label = "copy after $delay ms"
    $w = Notepad-Open $Sample
    Key 'ctrl+a'
    Clip-Set 'Rhyan copied this before the fix.'
    $copy = "Brynlee copied this $delay ms into the fix."
    Review $label 'Notepad.exe' "race-copy-$delay" 'apply' {
      [Desk]::RaceAfterPanelHides((Panel), $delay, [IntPtr]::Zero, 0, $copy)
    }
    Start-Sleep -Milliseconds 500
    $outcome = Outcome $script:Result
    Note "${label}: $([Desk]::RaceReport()); outcome $outcome"
    $t = Notepad-Text $w
    Check "${label}: the copy isn't pasted, and Notepad holds the fix or its own text" ($t -notlike '*Brynlee*' -and (($outcome -eq 'applied' -and (Fixed-Once $t)) -or ($outcome -ne 'applied' -and $t -eq $Sample))) "$outcome; $t"
    Check "${label}: the clipboard keeps the user's copy" ((Clip-Text) -eq $copy) "$outcome; clipboard: $(Clip-Text)"
    Panel-Close
  }
  Close-App notepad

  # A paste the app acts on late: SuperEditr can't confirm it in time, says so, never tries again
  # and doesn't put the old clipboard back, so the late paste brings the fix, once.
  $late = Pad-Open 'late' $Sample @('--paste-delay', '2500')
  Key 'ctrl+a'
  Clip-Set 'Rhyan copied this before the late paste.'
  $lateMark = Log-Mark
  Review 'late paste' 'racepad.exe' 'race-late'
  Check 'late paste: the read is known safe' ($script:Read -match 'via UI Automation.*KnownSafe') $script:Read
  Check 'late paste: the outcome is uncertain' ((Outcome $script:Result) -eq 'uncertain') $script:Result
  $text = Wait-PanelText "couldn.t confirm" 3
  Check 'late paste: the panel says the change couldn''t be confirmed and offers the suggestion' ($text -match "couldn.t confirm" -and $text -match 'Copy suggestion') $text
  Shot 'race-late-uncertain'
  Start-Sleep 3
  Check 'late paste: the late paste brings the fix, once' (Fixed-Once (Pad-Text 'late')) (Pad-Text 'late')
  Check 'late paste: the clipboard still holds the fix, not the old clipboard' (Fixed-Once (Clip-Text)) (Clip-Text)
  $replaces = @(Log-Since $lateMark | Where-Object { $_ -match ' replace racepad\.exe' })
  Check 'late paste: the fix was pasted once and never again' ($replaces.Count -eq 1) (($replaces | ForEach-Object { Bare $_ }) -join '; ')
  Panel-Close
  Pad-Close

  # A window that stops responding, three times over: each review gives up within a bound and says
  # so, the keys it sent don't change the clipboard once the app wakes, and afterwards a review works.
  $hang = "$Root\hang-now.txt"
  $hung = Pad-Open 'hung' $Sample @('--hang-file', $hang)
  Key 'ctrl+a'
  Clip-Set 'Rhyan copied this before the app hung.'
  foreach ($i in 1, 2, 3) {
    [Desk]::Front($hung) | Out-Null
    '4000' | Set-Content $hang
    Start-Sleep -Milliseconds 400
    $mark = Log-Mark
    $sw = [Diagnostics.Stopwatch]::StartNew()
    Key 'ctrl+shift+f2'
    $text = Wait-PanelText 'responding' 6
    $ms = [int]$sw.Elapsed.TotalMilliseconds
    Check "hung $($i): the review says the app isn't responding, within 3 s" ($text -match 'responding' -and $ms -lt 3000) "after $ms ms: $text"
    if ($i -eq 1) { Shot 'race-hung' }
    Panel-Close
    Start-Sleep 5
    Check "hung $($i): once the app answers again, the clipboard is still the user's" ((Clip-Text) -eq 'Rhyan copied this before the app hung.') (Clip-Text)
    Check "hung $($i): nothing changed in the app" ((Pad-Text 'hung') -eq $Sample) (Pad-Text 'hung')
  }
  [Desk]::Front($hung) | Out-Null
  Key 'ctrl+a'
  Review 'after the hangs' 'racepad.exe' 'race-hung-recovered'
  Check 'after the hangs: a review is read and applied' ($script:Read -match 'KnownSafe' -and (Outcome $script:Result) -eq 'applied') "$script:Read; $script:Result"
  Check 'after the hangs: the app holds the fix' (Fixed-Once (Pad-Text 'hung')) (Pad-Text 'hung')
  Pad-Close
  End-Case
}

# WebView2 -----------------------------------------------------------------------------------

# SuperEditr's WebView2 processes: its browser processes and everything they started.
function Wv2-Processes {
  $app = @(Get-Process supereditr -ErrorAction SilentlyContinue | ForEach-Object Id)
  $all = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'")
  $browsers = @($all | Where-Object { $app -contains $_.ParentProcessId })
  @($browsers) + @($all | Where-Object { ($browsers.ProcessId -contains $_.ParentProcessId) })
}

function Webview-Case {
  Begin-Case webview "WebView2's processes stop under SuperEditr, as in a crash or an update: the panel and Settings come back"
  # The pages' renderers first: each page is loaded again.
  $mark = Log-Mark
  $renderers = @(Wv2-Processes | Where-Object { $_.CommandLine -match '--type=renderer' })
  $renderers | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  $line = Wait-Log $mark 'webview: the overlay window.s Renderer process stopped' 10
  Check "the panel's page is loaded again when its renderer stops" ([bool]$line) "$($renderers.Count) renderers stopped; $(Bare $line)"
  Start-Sleep 3
  $w = Notepad-Open $Sample
  Key 'ctrl+a'
  Review 'after the renderers stopped' 'Notepad.exe' 'webview-renderer'
  Check 'after the renderers stopped: a review applies' ((Outcome $script:Result) -eq 'applied') $script:Result
  # Then the browser process, with everything it started: each window is made again.
  $mark = Log-Mark
  $before = @(Wv2-Processes)
  $before | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  $made = Wait-Log $mark 'webview: made the overlay window again' 15
  Check 'the panel is made again when the browser process stops' ([bool]$made) "$($before.Count) processes stopped; $(Bare $made)"
  Start-Sleep 3
  $after = @(Wv2-Processes)
  Note "SuperEditr $(@(Get-Process supereditr -ErrorAction SilentlyContinue).Id -join ','); WebView2 processes: $((@(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'") | ForEach-Object { "$($_.ProcessId)<$($_.ParentProcessId)" }) -join ' ')"
  Check 'a new WebView2 browser process runs' ($after.Count -gt 0 -and -not ($after | Where-Object { $before.ProcessId -contains $_.ProcessId })) "$($after.Count) processes"
  $w = Notepad-Open $Sample
  Key 'ctrl+a'
  Review 'after the browser process stopped' 'Notepad.exe' 'webview-browser'
  Check 'after the browser process stopped: a review applies' ((Outcome $script:Result) -eq 'applied') $script:Result
  $t = Notepad-Text $w
  Check 'after the browser process stopped: Notepad holds the fix' ($t -like "*They're going*") $t
  Close-App notepad
  # A second launch opens Settings, which shows its pages again.
  App-Start
  $t = Wait-SettingsText '*Get Started*' 15
  Check 'Settings shows its pages again' ($t -like '*Get Started*') $(if ($t -like '*Get Started*') { 'Get Started is there' } else { "'$t'" })
  Shot 'webview-settings'
  $s = Settings-Window
  if ($s -ne [IntPtr]::Zero) { [Desk]::Close($s) }
  End-Case
}

# Locking ------------------------------------------------------------------------------------

# Asks the controller (ui-smoke-windows.sh) to do WHAT outside this session, and waits for its answer.
function Ask($what, $secs) {
  Remove-Item "$Out\answer.txt" -ErrorAction SilentlyContinue
  $what | Set-Content "$Out\ask.txt"
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while (-not (Test-Path "$Out\answer.txt") -and $sw.Elapsed.TotalSeconds -lt $secs) { Start-Sleep -Milliseconds 500 }
  $answer = if (Test-Path "$Out\answer.txt") { (Get-Content "$Out\answer.txt" -Raw).Trim() } else { "(no answer in $secs s)" }
  Remove-Item "$Out\ask.txt", "$Out\answer.txt" -ErrorAction SilentlyContinue
  $answer
}
function Epoch { [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() / 1000.0 }
function Log-Time($line) { if ("$line" -match '^(\d+\.\d+) ') { [double]$Matches[1] } else { 0 } }

function Lock-Case {
  Begin-Case lock 'the session locks with the panel open: nothing shows on the lock screen or is pasted, and unlocking finds it closed'
  $w = Notepad-Open $Sample
  Key 'ctrl+a'
  Clip-Set 'Rhyan copied this before locking.'
  Key 'ctrl+shift+f2'
  $ms = Wait-Panel 6
  $text = Wait-PanelText '\d+ changes?' 15
  Check 'the panel is open with a fix before locking' ($ms -ge 0 -and $text -match '\d+ changes?') "after $ms ms"
  Shot 'lock-panel-open'
  $mark = Log-Mark
  $locked = Epoch
  rundll32.exe user32.dll,LockWorkStation
  Start-Sleep 3
  Check 'the session is locked: no window of it is in front' ([Desk]::GetForegroundWindow() -eq [IntPtr]::Zero) "front: $([Desk]::GetForegroundWindow())"
  # The controller photographs the console, presses the review shortcut there, and unlocks by
  # connecting the session to the console again.
  $answer = Ask 'lock' 150
  $unlocked = Epoch
  Check 'the controller pressed the shortcut at the lock screen and unlocked' ($answer -match 'unlocked') $answer
  Start-Sleep 2
  $while = @(Log-Since $mark | Where-Object { $t = Log-Time $_; $t -ge $locked -and $t -le $unlocked })
  Check 'no review started or text was replaced while locked' (-not ($while | Where-Object { $_ -match ' shortcut 0x1| replace ' })) (($while | ForEach-Object { Bare $_ }) -join '; ')
  $p = Panel
  Check 'after unlocking, the panel is closed' ($p -eq [IntPtr]::Zero) "panel $p"
  $t = Notepad-Text $w
  Check 'after unlocking, Notepad holds its own text' ($t -eq $Sample) $t
  Check 'after unlocking, the clipboard is the user''s' ((Clip-Text) -eq 'Rhyan copied this before locking.') (Clip-Text)
  Shot 'lock-unlocked'
  # Everything works as before.
  [Desk]::Front($w) | Out-Null
  Key 'ctrl+a'
  Review 'after unlocking' 'Notepad.exe' 'lock-review'
  Check 'after unlocking: a review applies' ((Outcome $script:Result) -eq 'applied') $script:Result
  Close-App notepad
  End-Case
}

# Prepares a test machine for a snapshot that runs start from (ui-smoke-windows.sh --prepare): opens
# each app the tests use once, as they do, so a run doesn't pay for their first start.
function WarmUp-Case {
  Begin-Case warm-up 'Notepad, Edge, Chrome and Windows Terminal opened once'
  $w = Notepad-Open $Sample
  Check 'Notepad opens with the text' ((Notepad-Text $w) -eq $Sample) (Notepad-Text $w)
  Close-App notepad
  foreach ($b in @('Edge', $Edge, 'msedge', 9223), @('Chrome', $Chrome, 'chrome', 9222)) {
    $w = Browser-Open @b
    Check "$($b[0]) answers UI Automation with the focused field" ([Windows.Automation.AutomationElement]::FocusedElement.Current.ControlType -eq $EditType)
    Close-App $b[2]
    Remove-Item -Recurse -Force "$env:TEMP\smoke-$($b[0])-profile" -ErrorAction SilentlyContinue
  }
  $before = @(Get-Process WindowsTerminal -ErrorAction SilentlyContinue | ForEach-Object Id)
  Start-Process wt.exe -ArgumentList '--title', 'SmokeTerminal', 'cmd.exe', '/k', 'echo ready'
  Check 'Windows Terminal opens' ((Wait-Window WindowsTerminal '' 15) -ne [IntPtr]::Zero)
  Start-Sleep 3
  Get-Process WindowsTerminal -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id } | Stop-Process -Force
  End-Case
}

# Run ----------------------------------------------------------------------------------------

Remove-Item "$Out\results.tsv", "$Out\summary.txt", "$Out\done.txt", "$Out\capture.log" -ErrorAction SilentlyContinue
Remove-Item "$Shots\*" -ErrorAction SilentlyContinue
$started = [Diagnostics.Stopwatch]::StartNew()
Say "SuperEditr Windows smoke tests, $([Environment]::OSVersion.VersionString), $(Get-Date -Format 'yyyy-MM-dd HH:mm')"
try {
  if ($Only -eq 'warm-up') { WarmUp-Case }
  elseif ((Install-Case | Select-Object -Last 1) -eq $true) {
    FirstRun-Case
    if (Wants notepad) { Notepad-Case }
    if (Wants keys) { Keys-Case }
    if (Wants edge) { Browser-Case 'Edge' $Edge 'msedge' 'msedge.exe' 9223 }
    if (Wants chrome) { Browser-Case 'Chrome' $Chrome 'chrome' 'chrome.exe' 9222 }
    if (Wants terminal) { Terminal-Case }
    if (Wants races) { Races-Case }
    if (Wants lock) { Lock-Case }
    if (Wants webview) { Webview-Case }
    Begin-Case watchdog "SuperEditr kept running"
    Check 'SuperEditr is still running' ([bool](Get-Process supereditr -ErrorAction SilentlyContinue))
    End-Case
  }
} catch {
  $script:Case = 'driver'
  Check 'the driver ran to the end' $false "$_ at $($_.InvocationInfo.PositionMessage)"
} finally {
  Copy-Item $Trace "$Out\capture.log" -ErrorAction SilentlyContinue
  Say ''; Say "$script:Passed passed, $script:Failed failed in $([int]$started.Elapsed.TotalSeconds) s"
  [Desk]::ReleaseModifiers()
  "$script:Passed $script:Failed" | Set-Content "$Out\done.txt"
}
