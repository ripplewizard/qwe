#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"
load_state

ensure_session manual-t03 nvim -u NONE "$QWE_MANUAL_FIXTURE" >/dev/null

printf '%s\n' \
  'T03 checklist:' \
  '  - Confirm full-screen Neovim rendering and the 200-line file.' \
  '  - Use gg/G, Ctrl-d/Ctrl-u, :set number, :syntax on.' \
  '  - Insert Chinese and emoji, then verify width and cursor placement.' \
  '  - Run :split and switch windows with Ctrl-w w.' \
  '  - Resize the terminal and check redraw without corruption.' \
  '  - Detach with Ctrl-] after leaving insert/visual mode.' \
  '  - Re-run this script to reattach; then quit with :q!.' \
  ''

attach_session manual-t03
