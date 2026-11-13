# One command per release: sets the version, builds the ZIP, commits, tags, pushes and
# publishes a GitHub release that the app's updater (src\update.rs) finds.
#
#   powershell -ExecutionPolicy Bypass -File rust\release\release.ps1 2.1.0
#
# Needs: a clean working tree on main, the gh CLI signed in, rust\vendor\ffmpeg.exe and yt-dlp.exe.
# The release is v<version> on sverre200/JondisBoard with one asset, JondisBoard-<version>-win64.zip.
# The updater only replaces program files (.exe, .txt); sounds\ and config.json are never touched.
param([Parameter(Mandatory = $true)][string]$Version)
$ErrorActionPreference = "Stop"
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "version must look like 2.1.0" }
$rust = Split-Path -Parent $PSScriptRoot
$repo = Split-Path -Parent $rust
Push-Location $repo
try {
    if ((git branch --show-current) -ne "main") { throw "releases are cut from main" }
    if (git status --porcelain) { throw "commit or stash your changes first" }
    if (git tag --list "v$Version") { throw "v$Version already exists" }

    # the version lives in Cargo.toml (the app reads it at build time)
    $toml = Join-Path $rust "Cargo.toml"
    $text = [IO.File]::ReadAllText($toml)
    # the package's own version only: the first match (dependencies have version lines too)
    $new = ([regex]'(?m)^version = "[^"]+"').Replace($text, "version = `"$Version`"", 1)
    if (([regex]'(?m)^version = ').Matches($new).Count -ne ([regex]'(?m)^version = ').Matches($text).Count) { throw "Cargo.toml changed unexpectedly" }
    [IO.File]::WriteAllText($toml, $new, (New-Object Text.UTF8Encoding $false))

    & powershell -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "make_release.ps1")
    if ($LASTEXITCODE -ne 0) { throw "the build failed" }
    $zip = Join-Path $rust "dist\JondisBoard-$Version-win64.zip"
    if (-not (Test-Path $zip)) { throw "$zip wasn't built" }

    git add rust/Cargo.toml rust/Cargo.lock
    git commit -m "Release v$Version"
    git tag "v$Version"
    git push origin main "v$Version"
    gh release create "v$Version" $zip --title "JondisBoard $Version" --notes "Download JondisBoard-$Version-win64.zip, unzip it anywhere and read READ ME FIRST.txt. Already have JondisBoard? It offers this update by itself: click Update at the top."
    "released v$Version"
}
finally { Pop-Location }
