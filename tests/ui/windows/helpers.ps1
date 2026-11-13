# The Windows smoke tests' helper programs, compiled from their C# sources with Windows PowerShell's
# Add-Type. driver.ps1 and admin.ps1 dot-source this file. Keep it ASCII (see driver.ps1).
#
# A test machine prepared for the tests keeps them compiled in $HelperStore (admin.ps1 helpers), each
# in a folder named for its source's SHA-256, so a run doesn't compile them again. When a source has
# changed since, its folder isn't there, and the run compiles it into ROOT\helpers.
$HelperStore = 'C:\SmokeWinHelpers'
# Source: the program's file name (the process name the tests look for), its kind, and the
# assemblies it needs beyond the defaults.
$HelperKinds = @{
  'desktop.cs' = @('desktop.dll', 'Library', @('System.Windows.Forms', 'System.Drawing'))
  'fake-claude.cs' = @('claude.exe', 'ConsoleApplication', @())
  'racepad.cs' = @('racepad.exe', 'WindowsApplication', @())
}
function Helper-Folder($root, $source) {
  $hash = (Get-FileHash "$root\$source" -Algorithm SHA256).Hash.Substring(0, 16).ToLowerInvariant()
  "$([IO.Path]::GetFileNameWithoutExtension($source))-$hash"
}
# Compiles ROOT\SOURCE into a folder of INTO, unless it's already there; its path.
function Helper-Build($root, $source, $into) {
  $file, $kind, $refs = $HelperKinds[$source]
  $dir = "$into\$(Helper-Folder $root $source)"
  if (-not (Test-Path "$dir\$file")) {
    New-Item -ItemType Directory -Force $dir | Out-Null
    $code = Get-Content -Raw "$root\$source"
    if ($refs.Count) { Add-Type -TypeDefinition $code -OutputAssembly "$dir\$file" -OutputType $kind -ReferencedAssemblies $refs }
    else { Add-Type -TypeDefinition $code -OutputAssembly "$dir\$file" -OutputType $kind }
  }
  "$dir\$file"
}
# The compiled program for ROOT\SOURCE: the kept one when its source is unchanged, else compiled now.
function Helper-Path($root, $source) {
  $kept = "$HelperStore\$(Helper-Folder $root $source)\$($HelperKinds[$source][0])"
  if (Test-Path $kept) { $kept } else { Helper-Build $root $source "$root\helpers" }
}
