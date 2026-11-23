# Builds Photo Editor and installs it like an ordinary desktop app:
#   * the program goes to  %LOCALAPPDATA%\Programs\Photo Editor
#   * a "Photo Editor" shortcut goes on the Desktop
#
# Run it again after changing the code to update the installed copy.
# Usage (from this folder):  powershell -ExecutionPolicy Bypass -File .\install.ps1

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$env:Path += ";$env:USERPROFILE\.cargo\bin"
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = "$env:USERPROFILE\.cache\pe-target" }

Write-Host "Building (release)..."
Push-Location $root
try { cargo build --release -p photo-editor; if ($LASTEXITCODE -ne 0) { throw "build failed" } }
finally { Pop-Location }

$dest = Join-Path $env:LOCALAPPDATA "Programs\Photo Editor"
New-Item -ItemType Directory -Force $dest | Out-Null
if (Get-Process photo-editor -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$dest*" }) {
    throw "Photo Editor is open. Close it, then run this again."
}
Copy-Item (Join-Path $env:CARGO_TARGET_DIR "release\photo-editor.exe") $dest -Force
Copy-Item (Join-Path $root "crates\app\assets\icon.ico") $dest -Force
$exe = Join-Path $dest "photo-editor.exe"

$shell = New-Object -ComObject WScript.Shell
$places = @([Environment]::GetFolderPath("Desktop"))
foreach ($dir in $places) {
    $lnk = $shell.CreateShortcut((Join-Path $dir "Photo Editor.lnk"))
    $lnk.TargetPath = $exe
    $lnk.WorkingDirectory = $dest
    $lnk.IconLocation = (Join-Path $dest "icon.ico") + ",0"
    $lnk.Description = "Offline photo editor for iPhone ProRAW"
    $lnk.Save()
    Write-Host "Shortcut: $(Join-Path $dir 'Photo Editor.lnk')"
}
Write-Host "Installed to $dest"
