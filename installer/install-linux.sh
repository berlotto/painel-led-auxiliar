#!/bin/sh
# Instala o Painel LED para o usuário atual, sem root (versão .tar.gz).
#   ./install.sh            instala em ~/.local
#   ./install.sh --remover  desinstala
set -e
here=$(cd "$(dirname "$0")" && pwd)
bin="$HOME/.local/bin"
apps="$HOME/.local/share/applications"
icons="$HOME/.local/share/icons/hicolor/256x256/apps"

if [ "$1" = "--remover" ]; then
    pkill -x painel-led 2>/dev/null || true
    rm -f "$bin/painel-led" "$bin/painel-led-mcp" "$apps/painel-led.desktop" "$icons/painel-led.png"
    rm -f "$HOME/.config/autostart/painel-led.desktop"
    echo "Painel LED removido. Suas configurações ficam em ~/.config/painel-led."
    exit 0
fi

pkill -x painel-led 2>/dev/null || true
mkdir -p "$bin" "$apps" "$icons"
install -m 755 "$here/painel-led" "$here/painel-led-mcp" "$bin/"
install -m 644 "$here/painel-led.png" "$icons/"
sed "s|^Exec=.*|Exec=$bin/painel-led|" "$here/painel-led.desktop" > "$apps/painel-led.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true

echo "Instalado em $bin."
case ":$PATH:" in *":$bin:"*) ;; *) echo "Aviso: $bin não está no PATH." ;; esac
echo "Abra pelo menu de aplicativos (Painel LED) ou rode: painel-led &"
