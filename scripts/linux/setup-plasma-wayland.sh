#!/usr/bin/env bash
# One-time setup of Striper's text-insertion helpers on KDE Plasma 6 (Wayland),
# for Ubuntu/Kubuntu releases that ship ydotool 0.1.x (e.g. Ubuntu 25.10).
#
#   - wl-clipboard: clipboard access for paste-based insertion
#   - ydotool + ydotoold 0.1.x: key/typing injection through /dev/uinput
#   - kdotool (Plasma 6 build): foreground-window detection for correction
#
# The ydotool 0.1.x daemon hard-codes /tmp/.ydotool_socket with mode 0600, so
# it must run as the desktop user, not root. This script grants the logged-in
# seat user access to /dev/uinput (udev "uaccess", the same mechanism used for
# game controllers) and runs ydotoold as a systemd user service so it survives
# reboots. Run it as your normal user; it calls sudo where needed.
set -euo pipefail

if [[ $EUID -eq 0 ]]; then
  echo "Run this as your normal desktop user, not with sudo." >&2
  exit 1
fi

# Third-party apps (e.g. Topaz Video AI) can leave incompatible libraries on
# LD_LIBRARY_PATH, which breaks Qt/KDE tools and can break linking kdotool.
unset LD_LIBRARY_PATH LD_PRELOAD

echo "==> Installing packages"
sudo apt-get update
sudo apt-get install -y wl-clipboard ydotool ydotoold libdbus-1-dev pkg-config acl

echo "==> Loading uinput now and at every boot"
echo uinput | sudo tee /etc/modules-load.d/striper-uinput.conf >/dev/null
sudo modprobe uinput

echo "==> Granting the logged-in desktop user access to /dev/uinput"
# Must sort before 73-seat-late.rules, which applies "uaccess" ACLs.
sudo tee /etc/udev/rules.d/60-striper-uinput.rules >/dev/null <<'RULE'
KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess", OPTIONS+="static_node=uinput"
RULE
sudo udevadm control --reload-rules
sudo udevadm trigger --name-match=uinput
# Apply the ACL to the current session immediately as well.
sudo setfacl -m "u:$(id -un):rw" /dev/uinput

echo "==> Installing ydotoold as a user service"
unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
mkdir -p "$unit_dir"
cat >"$unit_dir/striper-ydotoold.service" <<'UNIT'
[Unit]
Description=ydotool 0.1.x input daemon (used by Striper for text insertion)
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=/usr/bin/ydotoold
Restart=on-failure
RestartSec=2

[Install]
WantedBy=graphical-session.target
UNIT
systemctl --user daemon-reload
systemctl --user enable --now striper-ydotoold.service

echo "==> Building kdotool v0.3.0 (Plasma 6) and installing it to /usr/local/bin"
if ! command -v cargo >/dev/null 2>&1; then
  if [[ -x "$HOME/.cargo/bin/cargo" ]]; then
    export PATH="$HOME/.cargo/bin:$PATH"
  else
    echo "cargo not found. Install Rust from https://rustup.rs, then re-run this script." >&2
    exit 1
  fi
fi
build_root="$(mktemp -d)"
trap 'rm -rf "$build_root"' EXIT
cargo install --git https://github.com/jinliu/kdotool --tag v0.3.0 --locked --root "$build_root"
# /usr/local/bin is on PATH for apps started from the Plasma launcher too,
# unlike ~/.cargo/bin.
sudo install -m 0755 "$build_root/bin/kdotool" /usr/local/bin/kdotool

echo "==> Checking"
sleep 1
ok=true
if systemctl --user is-active --quiet striper-ydotoold.service && [[ -S /tmp/.ydotool_socket ]]; then
  echo "ydotoold: running"
else
  echo "ydotoold: NOT running. See: journalctl --user -u striper-ydotoold -n 20" >&2
  ok=false
fi
if window_id="$(kdotool getactivewindow 2>/dev/null)" && [[ -n "$window_id" ]]; then
  echo "kdotool: active window $window_id"
else
  echo "kdotool: could not read the active window (are you in a Plasma 6 Wayland session?)" >&2
  ok=false
fi
command -v wl-copy >/dev/null && echo "wl-clipboard: installed"

if $ok; then
  echo
  echo "Done. Restart Striper, then check Vocabulary -> Correction compatibility and test dictation in Kate."
else
  exit 1
fi
