#!/bin/sh
set -eu

umask 077
state_dir="$HOME/Library/Application Support/QWE"
mkdir -p "$state_dir"
chmod 700 "$state_dir"

exec "$HOME/.local/bin/qwed" \
  --socket "${TMPDIR:-/tmp}/qwe.sock" \
  --store "$state_dir/qwe.sqlite" \
  --keychain-service-prefix qwe
