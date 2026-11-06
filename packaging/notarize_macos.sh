#!/usr/bin/env bash
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  printf 'macOS notarization must run on Darwin\n' >&2
  exit 2
fi

artifact="${1:-}"
identity="${QWE_DEVELOPER_ID:-}"
profile="${QWE_NOTARY_PROFILE:-}"
if [ -z "$artifact" ] || [ ! -f "$artifact" ]; then
  printf 'usage: QWE_DEVELOPER_ID=... QWE_NOTARY_PROFILE=... %s RELEASE.tar.gz\n' \
    "$0" >&2
  exit 2
fi
if [ -z "$identity" ] || [ -z "$profile" ]; then
  printf 'QWE_DEVELOPER_ID and QWE_NOTARY_PROFILE are required\n' >&2
  exit 2
fi

for command in codesign spctl xcrun tar mktemp; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'notarization requires %s\n' "$command" >&2
    exit 2
  }
done

scratch="$(mktemp -d "${TMPDIR:-/tmp}/qwe-notarize.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

submission="$(
  xcrun notarytool submit "$artifact" \
    --keychain-profile "$profile" \
    --wait \
    --output-format json
)"
status="$(printf '%s' "$submission" | python3 -c \
  'import json,sys; print(json.load(sys.stdin)["status"])')"
submission_id="$(printf '%s' "$submission" | python3 -c \
  'import json,sys; print(json.load(sys.stdin)["id"])')"
if [ "$status" != "Accepted" ]; then
  printf 'notarization was not accepted: %s\n' "$submission" >&2
  exit 1
fi

mkdir -p "$scratch/unpacked"
tar -xzf "$artifact" -C "$scratch/unpacked"
package_root="$(
  find "$scratch/unpacked" -mindepth 1 -maxdepth 1 -type d -print -quit
)"
[ -n "$package_root" ] || {
  printf 'release archive does not contain a package directory\n' >&2
  exit 1
}

for binary in qwe qwed qwe-worker qwe-session-exec; do
  path="$package_root/bin/$binary"
  codesign --verify --strict --verbose=2 "$path"
  signature="$(codesign -dv --verbose=4 "$path" 2>&1)"
  printf '%s\n' "$signature" | grep -F "Authority=$identity" >/dev/null || {
    printf '%s is not signed by the selected Developer ID\n' "$path" >&2
    exit 1
  }
  xattr -w com.apple.quarantine "0081;00000000;qwe-release-check;" "$path"
  spctl --assess --type execute --verbose=4 "$path"
done

printf '{"case":"PACK-004","notarization":"accepted","submission_id":"%s","developer_id":"%s","gatekeeper":"pass"}\n' \
  "$submission_id" "$identity"
