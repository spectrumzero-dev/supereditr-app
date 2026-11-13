# The Windows smoke tests' steps that need an administrator, run over SSH by
# scripts/ui-smoke-windows.sh. Keep this file ASCII (see driver.ps1).
#
# Only for a disposable test machine: it turns Smart App Control off, connects a locked session to
# the console through a SYSTEM task without its password, and changes the display scale and restarts.
#
#   admin.ps1 smart-app-control-off       turns Smart App Control off, so an unsigned installer runs
#   admin.ps1 start ROOT USER [ONLY]      runs ROOT\driver.ps1 in USER's desktop session (ONLY: cases, comma-separated)
#   admin.ps1 state ROOT                  "done", "running", "asking <what>" (the driver waits for the
#                                         controller to do something) or "stopped (<task result>)"
#   admin.ps1 stop                        ends the driver's task and removes it
#   admin.ps1 session ROOT USER           "yes" when USER is signed in at the console
#   admin.ps1 unlock ROOT USER            disconnects USER's locked console session and connects it to
#                                         the console again, which skips the password (test machines only)
#   admin.ps1 scale ROOT USER PERCENT     sets USER's display scale and restarts the machine; it takes
#                                         effect at the next sign-in (autologon)
#   admin.ps1 scale-is ROOT USER          USER's display scale, in percent
#   admin.ps1 boot-time                   when Windows last started
#   admin.ps1 helpers ROOT                compiles the helper programs from ROOT into the folder where
#                                         every run finds them (helpers.ps1), for a snapshot prepared
#                                         for the tests
#   admin.ps1 clock -Seconds N            sets the clock to N seconds after 1970 (UTC): a snapshot taken
#                                         with its memory comes back with the clock it had then
param([string]$Action, [string]$Root = 'C:\SmokeWin', [string]$User = '', [string]$Only = '', [long]$Seconds = 0)
$ErrorActionPreference = 'Stop'
$Task = 'SuperEditrSmoke'

switch ($Action) {
  'smart-app-control-off' {
    # Only on a machine rolled back afterwards: there's no turning it back on without a reinstall.
    Set-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy' -Name VerifiedAndReputablePolicyState -Value 0 -Type DWord
    CiTool.exe --refresh --json | Out-Null
    # A native program's failure doesn't stop the script by itself.
    if ($LASTEXITCODE) { throw "CiTool --refresh failed ($LASTEXITCODE)" }
    $state = (Get-MpComputerStatus).SmartAppControlState
    if ($state -ne 'Off') { throw "Smart App Control is $state, not Off" }
    "Smart App Control: $state, policy state $((Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy').VerifiedAndReputablePolicyState)"
  }
  'helpers' {
    . "$Root\helpers.ps1"
    # Only the current sources' programs: a folder for an older source is never used again.
    Remove-Item -Recurse -Force $HelperStore -ErrorAction SilentlyContinue
    foreach ($source in $HelperKinds.Keys | Sort-Object) { Helper-Build $Root $source $HelperStore }
  }
  'boot-time' {
    # When Windows last started, to tell that a restart happened.
    (Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToString('o')
  }
  'scale-is' {
    # The display scale USER's sessions use, in percent.
    $sid = (New-Object System.Security.Principal.NTAccount($User)).Translate([System.Security.Principal.SecurityIdentifier]).Value
    [int][math]::Round((Get-ItemProperty "Registry::HKEY_USERS\$sid\Control Panel\Desktop").LogPixels * 100 / 96)
  }
  'clock' {
    Set-Date ([DateTimeOffset]::FromUnixTimeSeconds($Seconds).LocalDateTime) | Out-Null
    "clock set to $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
  }
  'session' {
    $who = (Get-CimInstance Win32_ComputerSystem).UserName
    if ($who -and $who -like "*\$User") { 'yes' } else { "no ($who)" }
  }
  'start' {
    New-Item -ItemType Directory -Force "$Root\out" | Out-Null
    icacls $Root /grant '*S-1-5-32-545:(OI)(CI)M' | Out-Null   # Users: the driver writes results there
    Remove-Item "$Root\out\done.txt" -ErrorAction SilentlyContinue
    # conhost --headless keeps the task's console out of sight; otherwise Windows Terminal hosts it
    # in a window that takes the foreground.
    $argv = "--headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File $Root\driver.ps1 -Root $Root"
    if ($Only) { $argv += " -Only $Only" }
    $a = New-ScheduledTaskAction -Execute 'conhost.exe' -Argument $argv
    $p = New-ScheduledTaskPrincipal -UserId "$env:COMPUTERNAME\$User" -LogonType Interactive -RunLevel Limited
    $s = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 45) -AllowStartIfOnBatteries
    Register-ScheduledTask -TaskName $Task -Action $a -Principal $p -Settings $s -Force | Out-Null
    Start-ScheduledTask $Task
    'started'
  }
  'state' {
    if (Test-Path "$Root\out\done.txt") { 'done'; break }
    if ((Test-Path "$Root\out\ask.txt") -and -not (Test-Path "$Root\out\answer.txt")) { "asking $((Get-Content "$Root\out\ask.txt" -Raw).Trim())"; break }
    $t = Get-ScheduledTask $Task -ErrorAction SilentlyContinue
    if (-not $t) { 'stopped (no task)'; break }
    # Not running: the driver may have written done.txt and ended since the first look.
    if ($t.State -eq 'Running') { 'running' }
    elseif (Test-Path "$Root\out\done.txt") { 'done' }
    else { "stopped ($((Get-ScheduledTaskInfo $Task).LastTaskResult))" }
  }
  'stop' {
    Stop-ScheduledTask $Task -ErrorAction SilentlyContinue
    Unregister-ScheduledTask $Task -Confirm:$false -ErrorAction SilentlyContinue
    'stopped'
  }
  'unlock' {
    # tscon run as SYSTEM connects a disconnected session to the console without its password.
    $id = ((query session $User | Select-Object -Skip 1) -split '\s+' | Where-Object { $_ -match '^\d+$' } | Select-Object -First 1)
    foreach ($step in @(@('tsdiscon.exe', "$id"), @('tscon.exe', "$id /dest:console"))) {
      $a = New-ScheduledTaskAction -Execute $step[0] -Argument $step[1]
      $p = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
      Register-ScheduledTask -TaskName "$Task-Unlock" -Action $a -Principal $p -Force | Out-Null
      Start-ScheduledTask "$Task-Unlock"
      for ($i = 0; $i -lt 20 -and (Get-ScheduledTask "$Task-Unlock").State -eq 'Running'; $i++) { Start-Sleep -Milliseconds 250 }
      Start-Sleep 1
      $result = (Get-ScheduledTaskInfo "$Task-Unlock").LastTaskResult
      Unregister-ScheduledTask "$Task-Unlock" -Confirm:$false
      if ($result -ne 0) { "$($step[0]) failed ($result)"; break }
    }
    $state = ((query session $User | Select-Object -Skip 1) -split '\s+' | Where-Object { $_ -in 'Active', 'Disc' } | Select-Object -First 1)
    "session $id $state"
  }
  'scale' {
    # A custom scale for every display, which Windows applies at the next sign-in.
    $percent = [int]$Only
    $sid = (New-Object System.Security.Principal.NTAccount($User)).Translate([System.Security.Principal.SecurityIdentifier]).Value
    $desktop = "Registry::HKEY_USERS\$sid\Control Panel\Desktop"
    Set-ItemProperty $desktop -Name LogPixels -Value ([int](96 * $percent / 100)) -Type DWord
    Set-ItemProperty $desktop -Name Win8DpiScaling -Value 1 -Type DWord
    "scale $percent% for $User; restarting"
    Restart-Computer -Force
  }
  default { throw "unknown action '$Action'" }
}
