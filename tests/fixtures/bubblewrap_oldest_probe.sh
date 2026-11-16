#!/usr/bin/env bash
set -euo pipefail

archive=${1:?archive path is required}
qwed=${2:?qwed path is required}
stage10=${3:?stage10 fixture path is required}
apparmor_probe=${4:?AppArmor probe path is required}
apparmor_profile=${5:?AppArmor profile path is required}
apparmor_installer=${6:?AppArmor installer path is required}

fixture_dir=$(CDPATH= cd -- "$(dirname -- "$stage10")" && pwd)
work=$(mktemp -d /tmp/qwe-bwrap-0.5.XXXXXX)
installed=/usr/local/bin/bwrap
backup="$work/bwrap.previous"
had_previous=0
profile=/etc/apparmor.d/qwe-bwrap
profile_backup="$work/qwe-bwrap.previous"
had_profile=0

cleanup() {
  if [ "$had_previous" -eq 0 ]; then
    sudo rm -f "$installed"
  else
    sudo install -m 0755 "$backup" "$installed"
  fi
  if [ "$had_profile" -eq 0 ]; then
    sudo apparmor_parser -R "$profile" >/dev/null 2>&1 || true
    sudo rm -f "$profile"
  else
    sudo install -m 0644 "$profile_backup" "$profile"
    sudo apparmor_parser -r "$profile" >/dev/null 2>&1 || true
  fi
  find "$work" -depth -delete 2>/dev/null || true
}
trap cleanup EXIT

sudo systemctl stop \
  unattended-upgrades.service apt-daily.service apt-daily-upgrade.service \
  >/dev/null 2>&1 || true
sudo env DEBIAN_FRONTEND=noninteractive apt-get \
  -o Acquire::ForceIPv4=true update -qq
sudo env DEBIAN_FRONTEND=noninteractive apt-get \
  -o Acquire::ForceIPv4=true install -y -qq --no-install-recommends \
  autoconf automake build-essential libcap-dev libtool make pkg-config \
  >/dev/null

if [ -x "$installed" ]; then
  sudo cp -a "$installed" "$backup"
  had_previous=1
fi
if [ -f "$profile" ]; then
  sudo cp -a "$profile" "$profile_backup"
  had_profile=1
fi

tar -xzf "$archive" -C "$work"
source_dir=$(find "$work" -mindepth 1 -maxdepth 1 -type d -name 'bubblewrap-*' -print -quit)
(
  cd "$source_dir"
  ./autogen.sh
  ./configure --disable-man
  make -j"$(getconf _NPROCESSORS_ONLN)"
  sudo make install
)

test "$("$installed" --version)" = "bubblewrap 0.5.0"
root=$(mktemp -d /tmp/qwe-bwrap-matrix.XXXXXX)
export PATH="/usr/local/bin:$PATH"
export PYTHONPATH="$fixture_dir"
if [ "$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null || echo 0)" = "1" ]; then
  python3 "$apparmor_probe" \
    --profile "$apparmor_profile" \
    --installer "$apparmor_installer" \
    --qwed "$qwed" \
    --socket "$root/qwed.sock" \
    --stage10-fixture "$stage10"
else
  python3 "$stage10" --qwed "$qwed" --socket "$root/qwed.sock"
fi
find "$root" -depth -delete 2>/dev/null || true
