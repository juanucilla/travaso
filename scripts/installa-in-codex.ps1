# Installa Travaso nel Codex di Giovanni, caricando la memoria ereditata da Claude.
# ~/.codex/claude-memory resta intatto; Travaso ne fa anche il suo archivio in ~/.travaso/archivio.
#   powershell -ExecutionPolicy Bypass -File scripts\installa-in-codex.ps1
$ErrorActionPreference = "Stop"
$repo  = Split-Path $PSScriptRoot -Parent
$codex = Join-Path $env:USERPROFILE ".codex"
$dest  = Join-Path $codex "travaso"

New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item "$repo\travaso.py" $dest -Force
New-Item -ItemType Directory -Force "$codex\skills\travaso" | Out-Null
Copy-Item "$repo\skill\SKILL.md" "$codex\skills\travaso\SKILL.md" -Force

$env:TRAVASO_DIR = Join-Path $env:USERPROFILE ".travaso"
python "$dest\travaso.py" --importa "$codex\claude-memory"

$cfg = Join-Path $codex "config.toml"
Copy-Item $cfg "$cfg.bak-pre-travaso" -Force
if (-not (Select-String -Path $cfg -Pattern '^\[mcp_servers\.travaso\]' -Quiet)) {
  Add-Content $cfg "`n[mcp_servers.travaso]`ncommand = 'python'`nargs = ['$dest\travaso.py']`nstartup_timeout_sec = 30`n" -Encoding UTF8
}
python "$dest\travaso.py" --stato
Write-Host "Fatto. In Codex scrivi: 'travasa tutta la memoria con la skill travaso'."
