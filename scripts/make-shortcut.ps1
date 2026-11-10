$ErrorActionPreference = "Stop"

$exe = Join-Path $PSScriptRoot "..\src-tauri\target\release\postlite.exe"
if (-not (Test-Path $exe)) {
  throw "release exe not found at $exe - run: cargo build --release"
}
$exe = (Resolve-Path $exe).Path

$locations = [ordered]@{
  Desktop   = [Environment]::GetFolderPath("Desktop")
  StartMenu = [Environment]::GetFolderPath("Programs")
}

$shell = New-Object -ComObject WScript.Shell
foreach ($entry in $locations.GetEnumerator()) {
  $link = Join-Path $entry.Value "postlite.lnk"
  $shortcut = $shell.CreateShortcut($link)
  $shortcut.TargetPath = $exe
  $shortcut.WorkingDirectory = Split-Path $exe
  $shortcut.IconLocation = "$exe,0"
  $shortcut.Description = "postlite - HTTP request client"
  $shortcut.Save()
  Write-Output "$($entry.Key): $link"
}
