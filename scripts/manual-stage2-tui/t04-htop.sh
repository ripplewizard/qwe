#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"
load_state

ensure_session manual-t04 htop >/dev/null

printf '%s\n' \
  'T04 checklist:' \
  '  - Confirm CPU, memory, Tasks, and process rows are complete.' \
  '  - Confirm values refresh continuously.' \
  '  - Test sorting/function-key interaction.' \
  '  - Resize the terminal and check the layout.' \
  '  - Detach with Ctrl-], then re-run this script to reattach.' \
  '  - Confirm input works, press q, and verify the Session exits.' \
  ''

attach_session manual-t04
