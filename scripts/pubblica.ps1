# Pubblica Travaso su GitHub (account già autenticato con gh). Esegui dalla cartella del repo:
#   powershell -ExecutionPolicy Bypass -File scripts\pubblica.ps1
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

Write-Host "1/4 Test..."
python tests\test_travaso.py
if ($LASTEXITCODE -ne 0) { throw "Test falliti: non pubblico." }

Write-Host "2/4 Controllo che non ci siano dati personali..."
$sospetti = Get-ChildItem -Recurse -File | Select-String -Pattern 'giovanni\.natella@|\+39 ?3\d\d|AKIA[0-9A-Z]{16}|gho_|ghp_|990597228965' -List
if ($sospetti) { $sospetti | ForEach-Object { $_.Path }; throw "Trovati possibili dati personali: fermo." }

Write-Host "3/4 Git..."
if (-not (Test-Path .git)) { git init -b main | Out-Null }
git add -A
git commit -m "Travaso 1.0.0: MCP che travasa la memoria da un'IA all'altra" | Out-Null

Write-Host "4/4 GitHub..."
gh repo create juanucilla/travaso --public --source . --push --description "MCP che travasa la memoria da un'IA all'altra: conserva, scarica, svuota" --homepage "https://github.com/juanucilla/travaso"
gh release create v1.0.0 --title "Travaso 1.0.0" --notes "Prima versione: conserva, scarica, svuota."
Write-Host "Fatto: https://github.com/juanucilla/travaso"
