# Build JondisBoard.exe and "Uninstall JondisBoard.exe" from src\ and put them in the project folder.
#
#   powershell -ExecutionPolicy Bypass -File scripts\build.ps1
#
# The builds run at low priority, so a game you're playing doesn't stutter. If JondisBoard is open,
# the new JondisBoard.exe waits and swaps in when you close it (see install_build.ps1).
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

function Build($name, $script, $extra) {
    # --specpath build keeps PyInstaller's generated spec file in build\, so the icon paths are
    # relative to build\ (hence ..\assets)
    $pyinstaller = @(
        "-m", "PyInstaller", "--noconfirm", "--onefile", "--windowed", "--name", "`"$name`"",
        "--icon", "..\assets\icon.ico", "--add-data", "..\assets\icon.ico;.", "--add-data", "..\assets\icon.png;.",
        "--collect-data", "customtkinter"
    ) + $extra + @("--specpath", "build", "--distpath", "build_out", $script)
    $build = Start-Process -FilePath "$root\.venv\Scripts\python.exe" -ArgumentList $pyinstaller -NoNewWindow -PassThru
    $null = $build.Handle   # (without touching the handle, PowerShell never reports the exit code)
    try { $build.PriorityClass = "BelowNormal" } catch {}
    $build.WaitForExit()
    if ($build.ExitCode -ne 0) {
        "Building $name failed (exit code $($build.ExitCode)); nothing was replaced."
        exit $build.ExitCode
    }
}

Build "JondisBoard" "src\app.py" @("--collect-all", "yt_dlp_ejs")
# (the uninstaller only needs the window and the logo: leave out audio, downloads and numpy)
Build "Uninstall JondisBoard" "src\uninstall.py" @("--exclude-module", "numpy", "--exclude-module", "yt_dlp",
    "--exclude-module", "sounddevice", "--exclude-module", "soundfile", "--exclude-module", "keyboard")
# the uninstaller isn't running while you build, so it can go straight in
Copy-Item "$root\build_out\Uninstall JondisBoard.exe" "$root\Uninstall JondisBoard.exe" -Force
& "$PSScriptRoot\install_build.ps1" "$root\build_out\JondisBoard.exe"
