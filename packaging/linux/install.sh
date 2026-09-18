#!/usr/bin/env sh
# Installs Agentty from this folder into ~/.local (or $PREFIX): the binary, the desktop entry
# (application menu, agentty:// links) and the icon. Run `./install.sh --uninstall` to remove it.
set -eu
PREFIX="${PREFIX:-$HOME/.local}"
here="$(cd "$(dirname "$0")" && pwd)"
desktop="$PREFIX/share/applications/run.agentty.Agentty.desktop"
icon="$PREFIX/share/icons/hicolor/256x256/apps/agentty.png"

if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$PREFIX/bin/agentty" "$desktop" "$icon"
  echo "Agentty removed from $PREFIX"
  exit 0
fi

install -Dm755 "$here/bin/agentty" "$PREFIX/bin/agentty"
install -Dm644 "$here/share/applications/run.agentty.Agentty.desktop" "$desktop"
install -Dm644 "$here/share/icons/hicolor/256x256/apps/agentty.png" "$icon"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$PREFIX/share/applications" || true
command -v xdg-mime >/dev/null 2>&1 && xdg-mime default run.agentty.Agentty.desktop x-scheme-handler/agentty || true
echo "Agentty installed to $PREFIX/bin/agentty"
case ":$PATH:" in *":$PREFIX/bin:"*) ;; *) echo "Add $PREFIX/bin to PATH to run 'agentty' from a shell." ;; esac
