#!/bin/sh
set -eu

profile_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
source_profile="$profile_dir/qwe-bwrap"
target_profile=/etc/apparmor.d/qwe-bwrap

if [ "$(id -u)" -ne 0 ]; then
  printf 'run this installer with sudo/root privileges\n' >&2
  exit 1
fi
command -v apparmor_parser >/dev/null || {
  printf 'apparmor_parser is unavailable\n' >&2
  exit 1
}

install -D -m 0644 "$source_profile" "$target_profile"
apparmor_parser -r "$target_profile"
printf 'installed and loaded %s\n' "$target_profile"
