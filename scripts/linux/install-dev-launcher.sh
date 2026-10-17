#!/usr/bin/env bash
# One-time install of a "Striper (dev)" entry in the application launcher
# (KDE Plasma and other freedesktop menus) that runs this checkout through
# striper-dev-launch.sh: update main, rebuild if needed, start. Run it as your
# normal user; re-running it is safe. To remove it:
#   rm ~/.local/share/applications/striper-dev.desktop \
#      ~/.local/share/icons/hicolor/*/apps/striper-dev.png
set -euo pipefail

if [[ $EUID -eq 0 ]]; then
  echo "Run this as your normal desktop user, not with sudo." >&2
  exit 1
fi
# Keep stray Qt libraries (e.g. Topaz Video AI) away from kbuildsycoca6.
unset LD_LIBRARY_PATH LD_PRELOAD

repo="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd)"
launcher="$repo/scripts/linux/striper-dev-launch.sh"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
chmod +x "$launcher"

echo "==> Installing icons"
icons="$repo/src-tauri/icons"
for pair in 32:32x32.png 64:64x64.png 128:128x128.png 256:128x128@2x.png; do
  size="${pair%%:*}"
  install -Dm644 "$icons/${pair#*:}" "$data/icons/hicolor/${size}x${size}/apps/striper-dev.png"
done

echo "==> Writing $data/applications/striper-dev.desktop"
mkdir -p "$data/applications"
# Exec arguments are quoted per the Desktop Entry spec so paths with spaces work.
exec_path="\"${launcher//\"/\\\"}\""
cat >"$data/applications/striper-dev.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Striper (dev)
GenericName=Speech Dictation
Comment=Update and run the development version of Striper from $repo
Exec=$exec_path
Icon=striper-dev
Terminal=false
StartupNotify=false
Categories=Utility;Accessibility;AudioVideo;
Keywords=dictation;speech;voice;transcription;
Actions=OpenLog;

[Desktop Action OpenLog]
Name=Open launch log
Exec=$exec_path --open-log
EOF

echo "==> Refreshing the application menu"
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$data/icons/hicolor" || true
if command -v kbuildsycoca6 >/dev/null; then
  kbuildsycoca6 >/dev/null 2>&1 || true
fi

echo "Done. Search for \"Striper (dev)\" in the launcher. Launch logs go to"
echo "${XDG_STATE_HOME:-$HOME/.local/state}/striper/dev-launch.log"
