#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"
load_state

session_id="$(ensure_session manual-t11 bash)"

printf '%s\n' \
  "Session: $session_id" \
  'T11 checklist:' \
  '  Run in the attached Bash session:' \
  '    sleep 300' \
  '    Ctrl-Z        # expect Stopped' \
  '    jobs' \
  '    bg            # expect Running' \
  '    jobs' \
  '    fg            # task returns to foreground' \
  '    Ctrl-C        # expect the shell prompt' \
  '    sleep 300 &' \
  '    jobs          # expect Running' \
  '  Detach with Ctrl-], re-run this script, then confirm jobs still shows Running.' \
  '  Finish with: kill %1; jobs' \
  ''

attach_session "$session_id"
