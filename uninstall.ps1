# Remove a WitDiff install, without needing the binary.
#
#   .\uninstall.ps1                    # what would be removed
#   .\uninstall.ps1 -Execute          # remove it
#   .\uninstall.ps1 -KeepIntegrations
#
# Why this exists: `witdiff uninstall` needs the binary, and the documented
# install path is `curl ... | sh`, which leaves no copy of the installer either.
# So a user whose binary is broken, or whose alpha predates the `uninstall`
# subcommand, has no documented way back to a clean state. This script is the
# fallback, and it needs nothing but PowerShell.
#
# Like install.sh, the paths here are restricted to the locations the installer
# writes. Anything else is reported rather than deleted.
[CmdletBinding()]
param(
  [switch]$Execute,
  [switch]$KeepIntegrations
)

$ErrorActionPreference = 'Stop'

function Show-Usage {
  Write-Host @'
Remove a WitDiff install.

  .\uninstall.ps1                    # report what would be removed
  .\uninstall.ps1 -Execute          # remove it
  .\uninstall.ps1 -KeepIntegrations # remove only the binary

With no -Execute nothing is deleted, so the dry run is the default: a
destructive script that acts the moment you run it is one you cannot inspect
first. Both witdiff uninstall and install.sh --uninstall exist and remove more
than this does; reach for those when they work.
'@
}

foreach ($arg in $args) {
  if ($arg -in @('-h', '--help', '-?', '/?')) { Show-Usage; exit 0 }
}

$removed = 0
$refused = 0

# Never follow a symlink out of the directories this script owns, and never
# remove anything the installer would not have written. The allowed list below
# is the same one install.sh enforces.
function Remove-KnownBinary {
  param([string]$Path)

  $home_ = if ($env:HOME) { $env:HOME } else { $env:USERPROFILE }
  $allowed = @(
    (Join-Path $home_ '.local\bin\witdiff.exe'),
    (Join-Path $home_ '.local\bin\witdiff'),
    (Join-Path $env:LOCALAPPDATA 'Programs\witdiff\witdiff.exe'),
    (Join-Path $env:ProgramFiles 'witdiff\witdiff.exe')
  )

  if ($allowed -notcontains $Path) {
    Write-Host "  REFUSED           : $Path (outside the known install locations)"
    $script:refused = $script:refused + 1
    return
  }
  if (-not (Test-Path -LiteralPath $Path)) { return }

  if ($Execute) {
    Remove-Item -LiteralPath $Path -Force
    Write-Host "  removed            : $Path"
  } else {
    Write-Host "  would remove       : $Path"
  }
  $script:removed = $script:removed + 1
}

function Remove-KnownPath {
  param([string]$Path)

  if (-not (Test-Path -LiteralPath $Path)) { return }
  $item = Get-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
  if ($item -and $item.PSIsContainer -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    Write-Host "  REFUSED           : $Path is a link, not a directory we wrote"
    $script:refused = $script:refused + 1
    return
  }

  if ($Execute) {
    Remove-Item -LiteralPath $Path -Recurse -Force
    Write-Host "  removed            : $Path"
  } else {
    Write-Host "  would remove       : $Path"
  }
  $script:removed = $script:removed + 1
}

$home_ = if ($env:HOME) { $env:HOME } else { $env:USERPROFILE }

Write-Host 'WitDiff uninstall'
Write-Host ''

Write-Host 'Binary'
foreach ($p in @(
    (Join-Path $home_ '.local\bin\witdiff.exe'),
    (Join-Path $home_ '.local\bin\witdiff'),
    (Join-Path $env:LOCALAPPDATA 'Programs\witdiff\witdiff.exe'),
    (Join-Path $env:ProgramFiles 'witdiff\witdiff.exe')
  )) { Remove-KnownBinary -Path $p }

if (-not $KeepIntegrations) {
  Write-Host ''
  Write-Host 'Agent integrations'
  foreach ($p in @(
      (Join-Path $home_ '.claude\skills\witdiff'),
      (Join-Path $home_ '.config\opencode\plugins\witdiff.ts'),
      (Join-Path $home_ '.config\opencode\skills\witdiff'),
      (Join-Path $home_ '.cursor\rules\witdiff.mdc')
    )) { Remove-KnownPath -Path $p }

  # Left in place: other plugins and rules may share these directories, and
  # they are not ours to delete. Removed only when they end up empty.
  foreach ($dir in @(
      (Join-Path $home_ '.claude\skills'),
      (Join-Path $home_ '.config\opencode\plugins'),
      (Join-Path $home_ '.config\opencode\skills'),
      (Join-Path $home_ '.cursor\rules')
    )) {
    if ((Test-Path -LiteralPath $dir) -and
        -not (Get-ChildItem -LiteralPath $dir -Force | Select-Object -First 1)) {
      if ($Execute) { Remove-Item -LiteralPath $dir -Force }
    }
  }

  Write-Host ''
  Write-Host 'Pi keeps its own record. Inspect it with:'
  Write-Host '  grep -A2 packages .pi/settings.json    # or ~/.pi/agent/settings.json'
  Write-Host 'and remove the entry with `pi remove <source>`.'
}

Write-Host ''
if ($refused -gt 0) {
  Write-Host 'Some paths were outside the known install locations; remove them yourself.'
  exit 1
}
if (-not $Execute) {
  Write-Host 'Dry run: nothing was removed. Re-run with -Execute to remove it.'
  exit 0
}
Write-Host "Done: $removed removed."
if ($removed -eq 0) {
  Write-Host 'Nothing was found. WitDiff does not appear to be installed.'
}
