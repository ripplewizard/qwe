#!/usr/bin/env bash
set -euo pipefail

count="${1:-10000}"

while [ "$count" -gt 0 ]; do
  printf 'QWE-LINE-%08d\n' "$count"
  count=$((count - 1))
done

