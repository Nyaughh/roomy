#!/bin/bash
# Install or update Roomy: curl -fsSL https://roomy.lh3k.dev/install.sh | bash
set -euo pipefail

url='https://github.com/Nyaughh/roomy/releases/latest/download/Roomy-macos.dmg'
dest='/Applications/Roomy.app'
work=''
stage=''
mounted=''

fail() { printf 'Roomy: %s\n' "$*" >&2; exit 1; }

detach() {
  hdiutil detach "$mounted" >/dev/null || hdiutil detach -force "$mounted" >/dev/null
}

cleanup() {
  local status=$?
  if [ -n "$mounted" ] && ! detach; then
    printf 'Roomy: Could not eject %s. Eject it in Finder.\n' "$mounted" >&2
    status=1
  fi
  if [ -n "$work" ]; then
    # Never recursively delete a directory that may still contain a mounted disk.
    rm -f "$work/Roomy-macos.dmg" || status=1
    if [ -d "$work/volume" ]; then rmdir "$work/volume" || status=1; fi
    rmdir "$work" || status=1
  fi
  if [ -n "$stage" ]; then
    if [ -d "$stage/previous.app" ] && [ ! -e "$dest" ]; then
      mv "$stage/previous.app" "$dest" || fail "Restore the previous copy from $stage/previous.app."
    fi
    rm -rf "$stage" || status=1
  fi
  exit "$status"
}

is_roomy() {
  [ -d "$1" ] && [ ! -L "$1" ] &&
    [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$1/Contents/Info.plist")" = dev.lh3k.roomy ]
}

quit_roomy() {
  if pgrep -ix roomy >/dev/null; then
    printf 'Quitting Roomy…\n'
    osascript -e 'tell application id "dev.lh3k.roomy" to quit' >/dev/null ||
      fail 'Please quit Roomy and run the installer again.'
    local deadline=$((SECONDS + 15))
    while pgrep -ix roomy >/dev/null; do
      [ "$SECONDS" -lt "$deadline" ] || fail 'Roomy is still running. Quit it and try again.'
      sleep 1
    done
  fi
}

main() {
  [ "$(uname -s)" = Darwin ] || fail 'This installer requires macOS.'
  [ -w /Applications ] || fail 'Installing in /Applications requires administrator permission. Run this installer from an account that can write there.'
  trap cleanup EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM

  # Keep both bundles on the destination volume and prevent two installers swapping them at once.
  mkdir -m 700 /Applications/.roomy-install ||
    fail 'Could not create /Applications/.roomy-install. Check write access and whether another install is in progress.'
  stage=/Applications/.roomy-install
  work=$(mktemp -d "${TMPDIR:-/tmp}/roomy.XXXXXX")
  printf 'Downloading the latest Roomy for macOS…\n'
  curl -fsSL --proto '=https' --retry 3 -o "$work/Roomy-macos.dmg" "$url" ||
    fail "Could not download Roomy-macos.dmg from the latest GitHub release."
  mounted="$work/volume"
  mkdir "$mounted"
  if ! hdiutil attach -readonly -nobrowse -mountpoint "$mounted" "$work/Roomy-macos.dmg" </dev/null >/dev/null; then
    mounted=''
    fail 'Could not mount Roomy-macos.dmg.'
  fi
  [ -d "$mounted/Roomy.app" ] || fail 'Roomy.app is missing from the disk image.'
  is_roomy "$mounted/Roomy.app" || fail 'The disk image does not contain the expected Roomy app.'
  ditto "$mounted/Roomy.app" "$stage/Roomy.app"
  detach || fail 'Could not eject Roomy-macos.dmg.'
  mounted=''
  xattr -rds com.apple.quarantine "$stage/Roomy.app" || fail 'Could not clear the download flag on Roomy.app.'

  if [ -e "$dest" ] || [ -L "$dest" ]; then
    is_roomy "$dest" || fail "A different item already exists at $dest."
  fi
  quit_roomy
  if [ -d "$dest" ]; then mv "$dest" "$stage/previous.app"; fi
  mv "$stage/Roomy.app" "$dest"
  printf 'Installed Roomy. Open it from Applications.\n'
}

# Wait for the complete function definitions before running a piped download.
main
