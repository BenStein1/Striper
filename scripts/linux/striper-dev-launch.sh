#!/usr/bin/env bash
# Start the development version of Striper from this checkout, for the
# "Striper (dev)" launcher entry that install-dev-launcher.sh creates.
#
#   1. If a dev copy is already running, raise its window instead.
#   2. Fast-forward main from origin, unless the checkout has local changes
#      or is on another branch.
#   3. Run `bun install` when package.json or bun.lock changed.
#   4. Start `bun run tauri dev`.
#
# There is no terminal window, so everything goes to a log file:
#   ${XDG_STATE_HOME:-~/.local/state}/striper/dev-launch.log
# (the previous launch is kept as dev-launch.log.1; `--open-log` opens it).
set -uo pipefail

repo="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd)"
log_dir="${XDG_STATE_HOME:-$HOME/.local/state}/striper"
log="$log_dir/dev-launch.log"
lock="${XDG_RUNTIME_DIR:-/tmp}/striper-dev-$(id -u).lock"

# Third-party apps (e.g. Topaz Video AI) can leave incompatible Qt libraries on
# LD_LIBRARY_PATH, which breaks the WebKitGTK/Qt stack Striper loads.
unset LD_LIBRARY_PATH LD_PRELOAD
# Launcher entries don't inherit the shell profile's PATH.
export PATH="$HOME/.cargo/bin:$HOME/.bun/bin:$PATH"
export GIT_TERMINAL_PROMPT=0

notify() {
  command -v notify-send >/dev/null && notify-send -a "Striper (dev)" -i striper-dev "$@" || true
}

mkdir -p "$log_dir"

if [[ "${1:-}" == --open-log ]]; then
  exec xdg-open "$log"
fi

exec 9>"$lock"
if ! flock -n 9; then
  # Another launch holds the lock: it is either still building or running.
  # Running the built binary hands off to the live instance (single-instance
  # plugin), which shows its window; while still building there is nothing
  # to raise yet.
  if pgrep -u "$(id -u)" -x striper >/dev/null && [[ -x "$repo/src-tauri/target/debug/striper" ]]; then
    exec "$repo/src-tauri/target/debug/striper"
  fi
  notify "Striper is still starting" "The build is in progress. Log: $log"
  exit 0
fi

[[ -f "$log" ]] && mv -f "$log" "$log.1"
exec >>"$log" 2>&1
echo "==> $(date '+%F %T') launching Striper dev from $repo"
cd "$repo" || exit 1

branch="$(git symbolic-ref --quiet --short HEAD || echo detached)"
if [[ "$branch" != main ]]; then
  echo "==> On '$branch', not main: skipping update"
elif [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  echo "==> Local changes present: skipping update"
else
  echo "==> Updating main"
  if ! timeout 60 git pull --ff-only origin main; then
    echo "==> Update failed (offline or diverged); starting the current checkout"
    notify "Striper: update skipped" "Could not fast-forward main. Starting the current version."
  fi
fi
echo "==> At $(git log -1 --format='%h %s')"

deps_hash="$(cat package.json bun.lock 2>/dev/null | sha256sum | cut -d' ' -f1)"
stamp="node_modules/.striper-deps-hash"
if [[ ! -d node_modules || "$(cat "$stamp" 2>/dev/null)" != "$deps_hash" ]]; then
  echo "==> Dependencies changed: bun install"
  if bun install; then
    echo "$deps_hash" >"$stamp"
  else
    notify -u critical "Striper failed to start" "bun install failed. Log: $log"
    exit 1
  fi
fi

notify "Starting Striper (dev)" "Rebuilding if needed; this can take a few minutes after an update."
echo "==> bun run tauri dev"
bun run tauri dev
status=$?
echo "==> $(date '+%F %T') exited with status $status"
if [[ $status -ne 0 ]]; then
  notify -u critical "Striper exited with an error" "Log: $log"
fi
exit "$status"
