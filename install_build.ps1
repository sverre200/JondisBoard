# Put a freshly built JondisBoard.exe in place without breaking a running copy.
#
#   powershell -ExecutionPolicy Bypass -File install_build.ps1 <path\to\new\JondisBoard.exe>
#
# A running JondisBoard.exe keeps reading its own code from the exe file on disk, so
# replacing that file while it runs makes the running app freeze or fail imports.
# If it's running, the new build waits as JondisBoard.update.exe and a hidden watcher
# swaps it in as soon as every copy of JondisBoard has closed.
param([Parameter(Mandatory = $true)][string]$NewExe)

$dir = $PSScriptRoot
$target = Join-Path $dir "JondisBoard.exe"
$staged = Join-Path $dir "JondisBoard.update.exe"

if (-not (Get-Process JondisBoard -ErrorAction SilentlyContinue)) {
    Copy-Item $NewExe $target -Force
    Remove-Item $staged -ErrorAction SilentlyContinue
    "installed: $target"
    exit 0
}

Copy-Item $NewExe $staged -Force
$watcher = @"
while (Get-Process JondisBoard -ErrorAction SilentlyContinue) { Start-Sleep -Seconds 1 }
Start-Sleep -Milliseconds 500
Move-Item -LiteralPath '$staged' -Destination '$target' -Force
"@
Start-Process powershell -WindowStyle Hidden -ArgumentList "-NoProfile", "-Command", $watcher
"JondisBoard is running: staged as $staged; it replaces JondisBoard.exe when you close the app"
