#!/bin/bash
set -euo pipefail
umask 077

# Refuses ordinary host execution before touching an installation or credential store.
check_guest() {
  case "$(/usr/sbin/sysctl -n hw.model)" in
    VirtualMac*) ;;
    *) printf '%s\n' 'Run this installer inside the macOS VM.' >&2; exit 1 ;;
  esac
  if [[ "$(/usr/bin/uname -m)" != arm64 || "$(/usr/bin/id -u)" == 0 ]]; then
    printf '%s\n' 'Use a non-root account in an Apple silicon macOS VM.' >&2
    exit 1
  fi
}

# Creates a private directory without following a final symlink or changing existing permissions.
private_dir() {
  if [[ -L "$1" || ( -e "$1" && ! -d "$1" ) ]]; then
    printf '%s\n' 'Installation path is not an ordinary directory.' >&2
    exit 1
  fi
  if [[ ! -d "$1" ]]; then /bin/mkdir -m 700 "$1"; fi
}

# Installs the verified vendor package without sudo, shell-profile edits or automatic login.
install_codex() {
  check_guest
  local media base destination staging
  media="$(cd "$(/usr/bin/dirname "$0")" && pwd -P)"
  cd "$media"
  /usr/bin/shasum -a 256 -c archive.sha256
  private_dir "$HOME"
  private_dir "$HOME/Library"
  private_dir "$HOME/Library/Application Support"
  base="$HOME/Library/Application Support/MiniClaw"
  private_dir "$base"
  private_dir "$base/Runtime"
  destination="$base/Runtime/codex-0.154.0"
  if [[ -L "$destination" || ( -e "$destination" && ! -d "$destination" ) ]]; then
    printf '%s\n' 'Codex destination is not an ordinary directory.' >&2
    exit 1
  fi
  staging="$(/usr/bin/mktemp -d "$base/Runtime/.codex-XXXXXX")"
  /usr/bin/tar -xzf "$media/codex.tar.gz" -C "$staging"
  if [[ "$("$staging/bin/codex" --version)" != 'codex-cli 0.154.0' ]]; then
    printf '%s\n' 'Codex version check failed; staged files retained.' >&2
    exit 1
  fi
  if [[ -d "$destination" ]]; then
    if ! /usr/bin/diff -qr "$staging" "$destination" >/dev/null; then
      printf '%s\n' 'Existing Codex files differ; nothing was replaced.' >&2
      exit 1
    fi
    /bin/rm -rf "$staging"
  else
    /bin/mv -n "$staging" "$destination"
  fi
  printf 'Installed: %s\n' "$destination/bin/codex"
  if [[ -f "$media/computer-use.tar.gz" ]]; then install_computer "$media"; fi
  if [[ -f "$media/worker.json" ]]; then
    install_worker "$media" "$base"
  else
    printf '%s\n' 'Sign-in requires guest network access. No credentials were imported.'
    "$media/socket-probe"
  fi
}

# Installs vendor-signed application code only; guest permissions remain user-controlled.
install_computer() {
  local source destination staging
  local requirement='anchor apple generic and certificate leaf[subject.OU] = "2DC432GLL2" and identifier "com.openai.sky.CUAService"'
  private_dir "$HOME/.codex"
  private_dir "$HOME/.codex/computer-use"
  staging="$(/usr/bin/mktemp -d "$HOME/.codex/computer-use/.install-XXXXXX")"
  /usr/bin/tar -xzf "$1/computer-use.tar.gz" -C "$staging"
  source="$staging/Codex Computer Use.app"
  /usr/bin/codesign --verify --deep --strict -R="$requirement" "$source"
  destination="$HOME/.codex/computer-use/Codex Computer Use.app"
  if [[ -L "$destination" ]]; then exit 1; fi
  if [[ -e "$destination" ]]; then
    /usr/bin/diff -qr "$source" "$destination" >/dev/null || {
      printf '%s\n' 'Existing Computer Use files differ; nothing was replaced.' >&2
      exit 1
    }
  else
    /usr/bin/ditto "$source" "$destination"
  fi
  /usr/bin/codesign --verify --deep --strict -R="$requirement" "$destination"
  /bin/rm -rf "$staging"
  printf '%s\n' 'Computer Use installed. Approve macOS permissions inside this VM when prompted.'
}

# Requires private user ownership for sensitive runtime directories.
private_owned() {
  private_dir "$1"
  if [[ "$(/usr/bin/stat -f '%u' "$1")" != "$(/usr/bin/id -u)" \
    || "$(/usr/bin/stat -f '%Lp' "$1")" != 700 ]]; then
    printf '%s\n' 'Worker directory must be private and user-owned.' >&2
    exit 1
  fi
}

# Verifies a signature; tests replace this with a stub for unsigned fixtures.
codesign_check() {
  /usr/bin/codesign --verify --strict "$1"
}

# Installs or updates the capture helper. It lives in MiniClaw's private folder, so a
# differing copy is an older release and is replaced through a rename.
install_desktop_helper() {
  local media="$1" base="$2" staged
  local desktop="$base/Runtime/miniclaw-desktop-stream"
  if [[ -L "$desktop" ]]; then exit 1; fi
  if [[ ! -e "$desktop" ]] || ! /usr/bin/cmp -s "$media/miniclaw-desktop-stream" "$desktop"; then
    staged="$(/usr/bin/mktemp "$base/Runtime/.desktop-stream-XXXXXX")"
    # The setup disk adds Finder metadata that a strict signature check refuses, so the
    # copy is made without it and the copy is what gets verified.
    /bin/cp -X "$media/miniclaw-desktop-stream" "$staged"
    /bin/chmod 700 "$staged"
    if ! codesign_check "$staged"; then /bin/rm -f "$staged"; exit 1; fi
    /bin/mv -f "$staged" "$desktop"
  fi
  codesign_check "$desktop"
}

# Installs the verified worker and starts its per-user service without sudo.
install_worker() {
  local media="$1" base="$2" digest binary config agent staged job
  cd "$media"
  /usr/bin/shasum -a 256 -c worker.sha256
  private_owned "$base"
  private_owned /Users/Shared/MiniClaw
  private_owned /Users/Shared/MiniClaw/Workspace
  digest="$(/usr/bin/shasum -a 256 miniclaw-worker)"
  digest="${digest%% *}"
  binary="$base/Runtime/worker-$digest"
  if [[ -L "$binary" ]]; then exit 1; fi
  if [[ -e "$binary" ]]; then
    /usr/bin/cmp "$media/miniclaw-worker" "$binary"
  else
    /bin/cp -n "$media/miniclaw-worker" "$binary"
    /bin/chmod 700 "$binary"
  fi
  install_desktop_helper "$media" "$base"
  # macOS charges screen capture to the worker job, so the worker asks for it when
  # MiniClaw first shows this desktop. A request from this Terminal would name Terminal.
  config="$base/worker.json"
  if [[ -L "$config" ]]; then exit 1; fi
  if [[ -e "$config" ]]; then
    /usr/bin/cmp "$media/worker.json" "$config"
  else
    /bin/cp -n "$media/worker.json" "$config"
    /bin/chmod 600 "$config"
  fi
  private_dir "$HOME/Library/LaunchAgents"
  agent="$HOME/Library/LaunchAgents/dev.miniclaw.worker.plist"
  job="gui/$(/usr/bin/id -u)/dev.miniclaw.worker"
  if [[ -L "$agent" ]]; then exit 1; fi
  if [[ -e "$agent" ]]; then
    case "$(/usr/bin/plutil -extract ProgramArguments.0 raw "$agent")" in
      "$base/Runtime/worker-"*) ;;
      *) printf '%s\n' 'Existing worker job is not owned by this installer.' >&2; exit 1 ;;
    esac
    /bin/launchctl bootout "$job" 2>/dev/null || true
  fi
  staged="$(/usr/bin/mktemp "$HOME/Library/LaunchAgents/.miniclaw-XXXXXX")"
  write_agent "$staged" "$binary" "$config" "$base"
  /bin/mv "$staged" "$agent"
  /bin/launchctl bootstrap "gui/$(/usr/bin/id -u)" "$agent"
  printf '%s\n' 'Worker installed. Continue sign-in in the MiniClaw setup wizard.'
}

# Builds the guest-user job with throttled crash recovery and no elevated identity.
write_agent() {
  local staged="$1" binary="$2" config="$3" base="$4"
  /usr/bin/plutil -create xml1 "$staged"
  /usr/bin/plutil -insert Label -string dev.miniclaw.worker "$staged"
  /usr/bin/plutil -insert ProgramArguments -array "$staged"
  /usr/bin/plutil -insert ProgramArguments.0 -string "$binary" "$staged"
  /usr/bin/plutil -insert ProgramArguments.1 -string "$config" "$staged"
  /usr/bin/plutil -insert RunAtLoad -bool true "$staged"
  /usr/bin/plutil -insert KeepAlive -dictionary "$staged"
  /usr/bin/plutil -insert KeepAlive.SuccessfulExit -bool false "$staged"
  /usr/bin/plutil -insert ThrottleInterval -integer 10 "$staged"
  /usr/bin/plutil -insert EnvironmentVariables -dictionary "$staged"
  /usr/bin/plutil -insert EnvironmentVariables.HOME -string "$HOME" "$staged"
  /usr/bin/plutil -insert StandardOutPath -string "$base/worker.log" "$staged"
  /usr/bin/plutil -insert StandardErrorPath -string "$base/worker-error.log" "$staged"
  /bin/chmod 600 "$staged"
}

install_codex
