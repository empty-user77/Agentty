#!/bin/sh
# After installing or removing the .deb / .rpm: refresh the application menu (agentty:// links come from the
# desktop entry's MimeType) and the icon cache. Both tools are optional.
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t /usr/share/icons/hicolor || true
exit 0
