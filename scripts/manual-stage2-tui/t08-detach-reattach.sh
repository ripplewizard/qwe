#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"
load_state

session_id="$(ensure_session manual-t08 nvim -u NONE "$QWE_MANUAL_FIXTURE")" ||
  die "failed to create or reuse the T08 Session"

printf '%s\n' \
  "Session: $session_id" \
  'T08 checklist:' \
  '  1. In Neovim Normal mode, run:' \
  '       :lua vim.defer_fn(function() vim.api.nvim_echo({{"QWE_T08_DETACHED_OUTPUT","ErrorMsg"}}, true, {}) end, 5000)' \
  '  2. Press Ctrl-] without waiting.' \
  '  3. Confirm the CLI returns to the local shell while Neovim keeps running.' \
  '  4. Wait at least 6 seconds, then re-run this script.' \
  '  5. Confirm QWE_T08_DETACHED_OUTPUT is visible.' \
  '  6. Run :echo "QWE_T08_REATTACHED_INPUT" and confirm the text appears.' \
  '  7. Confirm input, cursor, scroll, and split behavior still work.' \
  '  8. Quit with :q!.' \
  ''

attach_session "$session_id"
