# Builds the portable release: rust\dist\JondisBoard-<version>.zip containing
#   JondisBoard\JondisBoard.exe, ffmpeg.exe, yt-dlp.exe, READ ME FIRST.txt, sounds\ (empty)
#
#   powershell -ExecutionPolicy Bypass -File rust\release\make_release.ps1
#
# ffmpeg.exe:  -Ffmpeg <path>, else rust\vendor\ffmpeg.exe, else the one on PATH.
# yt-dlp.exe:  -YtDlp <path>, else rust\vendor\yt-dlp.exe (the standalone build from
#              https://github.com/yt-dlp/yt-dlp/releases; the pip launcher won't work).
# The build runs at below-normal priority so a game running at the same time doesn't stutter.
param([string]$Ffmpeg = "", [string]$YtDlp = "", [switch]$WithoutYtDlp)
# -WithoutYtDlp: a provisional ZIP without yt-dlp.exe (imports then say it is missing)
$ErrorActionPreference = "Stop"
$rust = Split-Path -Parent $PSScriptRoot
Push-Location $rust
try {
    $p = Start-Process cargo -ArgumentList "build", "--release" -NoNewWindow -PassThru
    $null = $p.Handle   # (without this, ExitCode stays empty)
    $p.PriorityClass = "BelowNormal"
    $p.WaitForExit()
    if ($p.ExitCode -ne 0) { throw "cargo build failed" }
    $version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value

    if (-not $Ffmpeg) { $Ffmpeg = Join-Path $rust "vendor\ffmpeg.exe" }
    if (-not (Test-Path $Ffmpeg)) { $Ffmpeg = (Get-Command ffmpeg.exe -ErrorAction SilentlyContinue).Source }
    if (-not $Ffmpeg) { throw "ffmpeg.exe not found (pass -Ffmpeg)" }
    if (-not $YtDlp) { $YtDlp = Join-Path $rust "vendor\yt-dlp.exe" }
    if (-not $WithoutYtDlp) {
        if (-not (Test-Path $YtDlp)) { throw "yt-dlp.exe not found: put the standalone build in rust\vendor\ (or pass -YtDlp)" }
        if ((Get-Item $YtDlp).Length -lt 5MB) { throw "$YtDlp looks like a pip launcher, not the standalone build" }
    }

    $dist = Join-Path $rust "dist"
    $stage = Join-Path $dist "JondisBoard"
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Force (Join-Path $stage "sounds") | Out-Null
    Copy-Item target\release\JondisBoard.exe $stage
    Copy-Item $Ffmpeg (Join-Path $stage "ffmpeg.exe")
    if (-not $WithoutYtDlp) { Copy-Item $YtDlp (Join-Path $stage "yt-dlp.exe") }
    Copy-Item (Join-Path $PSScriptRoot "READ ME FIRST.txt") $stage
    # ffmpeg is GPL: its licence goes with it
    $lic = Join-Path (Join-Path $rust "vendor") "ffmpeg-LICENSE.txt"
    if (Test-Path $lic) { Copy-Item $lic (Join-Path $stage "ffmpeg-LICENSE.txt") }
    $suffix = if ($WithoutYtDlp) { "-no-yt-dlp" } else { "" }
    $zip = Join-Path $dist "JondisBoard-$version$suffix.zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Compress-Archive -Path $stage -DestinationPath $zip -CompressionLevel Optimal
    Get-Item $zip | Select-Object FullName, @{n = "MB"; e = { [math]::Round($_.Length / 1MB, 1) } }
}
finally { Pop-Location }
