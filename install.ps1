# Instala o Painel LED no Windows:
#  - compila em modo release
#  - copia painel-led.exe e painel-led-mcp.exe para %LOCALAPPDATA%\Programs\painel-led
#  - registra o servidor MCP no Claude Code (escopo do usuário) e no Claude Desktop
#  - abre o painel
#
# Uso:  pwsh -File install.ps1            (tudo)
#       pwsh -File install.ps1 -SemMcp    (só instala os executáveis)
param([switch]$SemMcp)
$ErrorActionPreference = "Stop"

$dest = Join-Path $env:LOCALAPPDATA "Programs\painel-led"
$panel = Join-Path $dest "painel-led.exe"
$mcp = Join-Path $dest "painel-led-mcp.exe"

Write-Host "==> Compilando (release)..." -ForegroundColor Cyan
Push-Location $PSScriptRoot
try { cargo build --release; if ($LASTEXITCODE -ne 0) { throw "cargo build falhou" } } finally { Pop-Location }

Write-Host "==> Fechando instâncias abertas..." -ForegroundColor Cyan
Get-Process painel-led, painel-led-mcp -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

Write-Host "==> Copiando para $dest" -ForegroundColor Cyan
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item (Join-Path $PSScriptRoot "target\release\painel-led.exe") $panel -Force
Copy-Item (Join-Path $PSScriptRoot "target\release\painel-led-mcp.exe") $mcp -Force

if (-not $SemMcp) {
    # --- Claude Code (CLI, IDEs e aba Code do Desktop)
    if (Get-Command claude -ErrorAction SilentlyContinue) {
        Write-Host "==> Registrando MCP no Claude Code (escopo do usuário)..." -ForegroundColor Cyan
        claude mcp remove painel-led -s user 2>$null | Out-Null
        claude mcp add painel-led -s user -- $mcp
    } else {
        Write-Host "   (CLI 'claude' não encontrada: pulando Claude Code)" -ForegroundColor DarkGray
    }

    # --- Claude Desktop (chat)
    $desktopDir = Join-Path $env:APPDATA "Claude"
    if (Test-Path $desktopDir) {
        Write-Host "==> Registrando MCP no Claude Desktop..." -ForegroundColor Cyan
        $cfgPath = Join-Path $desktopDir "claude_desktop_config.json"
        if (Test-Path $cfgPath) {
            Copy-Item $cfgPath "$cfgPath.bak-painel-led" -Force
            $cfg = Get-Content $cfgPath -Raw | ConvertFrom-Json -AsHashtable
        } else {
            $cfg = @{}
        }
        if (-not $cfg.ContainsKey("mcpServers")) { $cfg["mcpServers"] = @{} }
        $cfg["mcpServers"]["painel-led"] = @{ command = $mcp; args = @() }
        $cfg | ConvertTo-Json -Depth 100 | Set-Content $cfgPath -Encoding utf8
        Write-Host "   Backup do arquivo anterior: $cfgPath.bak-painel-led" -ForegroundColor DarkGray
        Write-Host "   Reinicie o Claude Desktop para carregar o MCP." -ForegroundColor Yellow
    }
}

Write-Host "==> Abrindo o painel" -ForegroundColor Cyan
Start-Process $panel
Write-Host "Pronto. O ícone do painel fica na bandeja do sistema (perto do relógio)." -ForegroundColor Green
