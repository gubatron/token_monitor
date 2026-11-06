#!/usr/bin/env bash
# Authenticode-sign files with a PFX from the environment (Windows runner).
#
#   PFX_BASE64=... PFX_PASSWORD=... sign.sh FILE...
#
# Used by .github/workflows/windows-release.yml only when the signing secrets
# exist. SHA-256 file digest, RFC 3161 timestamp (also SHA-256), then
# `signtool verify /pa` so a broken signature fails the build. The decoded
# PFX lives in RUNNER_TEMP and is deleted on exit.
set -euo pipefail

: "${PFX_BASE64:?PFX_BASE64 is not set}"
: "${PFX_PASSWORD:?PFX_PASSWORD is not set}"
TIMESTAMP_URL=${TIMESTAMP_URL:-http://timestamp.digicert.com}
[ "$#" -gt 0 ] || { echo "usage: sign.sh FILE..." >&2; exit 2; }

# Newest x64 signtool from the Windows SDK on the runner.
signtool=$(find "/c/Program Files (x86)/Windows Kits/10/bin" -path '*/x64/signtool.exe' 2>/dev/null | sort -V | tail -n 1)
[ -n "$signtool" ] || { echo "signtool.exe not found" >&2; exit 1; }

# Git Bash would rewrite signtool's /switches as paths; paths below are
# converted with cygpath instead.
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'

pfx="${RUNNER_TEMP:?}/signing.pfx"
trap 'rm -f "$pfx"' EXIT
printf '%s' "$PFX_BASE64" | base64 -d > "$pfx"

for file in "$@"; do
  win=$(cygpath -w "$file")
  "$signtool" sign /fd SHA256 /tr "$TIMESTAMP_URL" /td SHA256 \
    /f "$(cygpath -w "$pfx")" /p "$PFX_PASSWORD" /d "TokenMon" "$win"
  "$signtool" verify /pa "$win"
done
