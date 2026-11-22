#!/bin/bash
# Install or update Roomy: curl -fsSL https://roomy.lh3k.dev/install.sh | bash
set -euo pipefail

url='https://github.com/Nyaughh/roomy/releases/latest/download/Roomy-macos.dmg'
dest='/Applications/Roomy.app'
work=''
stage=''
lock=''
mounted=''
needs_admin=false
replacing=false

fail() { printf 'Roomy: %s\n' "$*" >&2; exit 1; }

in_applications() {
  if [ "$needs_admin" = true ]; then
    sudo -n -- "$@" </dev/null
  else
    "$@"
  fi
}

detach() {
  hdiutil detach "$mounted" >/dev/null || hdiutil detach -force "$mounted" >/dev/null
}

discard_tree() {
  local path=$1 device
  shift
  device=$("$@" stat -f %d "${path%/*}") || return 1
  [ "$("$@" stat -f %d "$path")" = "$device" ] || return 1
  # Repair only directories, without following bundle symlinks. Finish this pass
  # before deleting any files so an unrepairable backup can be recovered intact.
  # shellcheck disable=SC2016 # The child shell expands its own loop variable.
  "$@" find -P "$path" -xdev -type d -exec sh -c '
    device=$1
    shift
    for dir do
      if [ "$(stat -f %d "$dir")" != "$device" ]; then
        printf "Roomy: Eject the volume mounted at %s before removing this backup.\n" "$dir" >&2
        exit 1
      fi
      if [ ! -r "$dir" ] || [ ! -w "$dir" ] || [ ! -x "$dir" ]; then
        chmod u+rwx "$dir" || exit 1
      fi
    done
  ' sh "$device" {} + || return 1
  "$@" find -P "$path" -xdev -depth -delete && [ ! -e "$path" ]
}

cleanup() {
  local status=$?
  if [ -n "$mounted" ] && ! detach; then
    printf 'Roomy: Could not eject %s. Eject it in Finder.\n' "$mounted" >&2
    status=1
  fi
  if [ -n "$work" ]; then
    # Never recursively delete a directory that may still contain a mounted disk.
    if [ -d "$work/Roomy.app" ]; then discard_tree "$work/Roomy.app" || status=1; fi
    rm -f "$work/Roomy-macos.dmg" || status=1
    if [ -d "$work/volume" ]; then rmdir "$work/volume" || status=1; fi
    rmdir "$work" || status=1
  fi
  if [ -n "$stage" ]; then
    if [ "$replacing" = true ] && [ ! -e "$dest" ] && ! in_applications mv "$stage/previous.app" "$dest"; then
      printf 'Roomy: Restore the previous copy from %s/previous.app.\n' "$stage" >&2
      status=1
    elif ! discard_tree "$stage" in_applications; then
      printf 'Roomy: Could not remove %s. Open that folder in Finder to recover or discard its contents, then retry.\n' "$stage" >&2
      status=1
    fi
  fi
  if [ -n "$lock" ]; then in_applications rmdir "$lock" || status=1; fi
  if [ "$status" -eq 0 ]; then printf 'Installed Roomy. Open it from Applications.\n'; fi
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
  trap cleanup EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM

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
  ditto "$mounted/Roomy.app" "$work/Roomy.app"
  detach || fail 'Could not eject Roomy-macos.dmg.'
  mounted=''
  xattr -rds com.apple.quarantine "$work/Roomy.app" || fail 'Could not clear the download flag on Roomy.app.'

  if [ ! -w /Applications ]; then
    printf 'Writing to /Applications requires administrator permission. Using sudo for the installation.\n' >&2
    # sudo prompts on the controlling terminal. Keep the piped script off its stdin.
    sudo -v </dev/null ||
      fail 'Could not obtain administrator permission. Run the installer in Terminal from an administrator account.'
    needs_admin=true
  fi

  # Keep the lock empty so even a backup that needs manual recovery cannot block a retry.
  in_applications mkdir -m 700 /Applications/.roomy-install ||
    fail 'Could not create /Applications/.roomy-install. Check write access and whether another install is in progress.'
  lock=/Applications/.roomy-install
  stage=$(in_applications mktemp -d /Applications/.roomy-install.XXXXXX)
  in_applications mv "$work/Roomy.app" "$stage/Roomy.app"

  if [ -e "$dest" ] || [ -L "$dest" ]; then
    is_roomy "$dest" || fail "A different item already exists at $dest."
  fi
  quit_roomy
  if [ -d "$dest" ]; then
    replacing=true
    in_applications mv "$dest" "$stage/previous.app"
  fi
  in_applications mv "$stage/Roomy.app" "$dest"
}

# Wait for the complete function definitions before running a piped download.
main
