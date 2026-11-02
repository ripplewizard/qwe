#!/usr/bin/env bash
set -euo pipefail

case "${1:-fail}" in
  fail)
    printf 'QWE-CHILD-Crash pid=%s code=42\n' "$$" >&2
    exit 42
    ;;
  success)
    printf 'QWE-CHILD-SUCCESS pid=%s code=0\n' "$$" >&2
    ;;
  *)
    printf 'usage: %s [fail|success]\n' "$0" >&2
    exit 2
    ;;
esac

