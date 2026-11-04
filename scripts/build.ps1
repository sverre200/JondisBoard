# Build JondisBoard.exe from src\ and put it in the project folder.
#
#   powershell -ExecutionPolicy Bypass -File scripts\build.ps1
#
# The build runs at low priority, so a game you're playing doesn't stutter. If JondisBoard is open,
# the new exe waits and swaps in when you close it (see install_build.ps1).
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
# --specpath build keeps PyInstaller's generated spec file in build\, so the icon paths are relative
# to build\ (hence ..\assets)
$pyinstaller = @(
    "-m", "PyInstaller", "--noconfirm", "--onefile", "--windowed", "--name", "JondisBoard",
    "--icon", "..\assets\icon.ico", "--add-data", "..\assets\icon.ico;.", "--add-data", "..\assets\icon.png;.",
    "--collect-data", "customtkinter", "--collect-all", "yt_dlp_ejs",
    "--specpath", "build", "--distpath", "build_out", "src\app.py"
)
$build = Start-Process -FilePath "$root\.venv\Scripts\python.exe" -ArgumentList $pyinstaller -NoNewWindow -PassThru
$null = $build.Handle   # (without touching the handle, PowerShell never reports the exit code)
try { $build.PriorityClass = "BelowNormal" } catch {}
$build.WaitForExit()
if ($build.ExitCode -ne 0) {
    "Build failed (exit code $($build.ExitCode)); JondisBoard.exe was not changed."
    exit $build.ExitCode
}
& "$PSScriptRoot\install_build.ps1" "$root\build_out\JondisBoard.exe"
