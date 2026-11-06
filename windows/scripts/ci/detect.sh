#!/usr/bin/env bash
# Report what exists under windows/ so windows-ci.yml can skip jobs that have
# nothing to work on yet (the workspace is built up over several waves).
#
#   detect.sh [windows-dir]
#
# Writes key=value lines to $GITHUB_OUTPUT (or stdout when unset):
#   workspace  true when windows/Cargo.toml exists
#   locked     "--locked" when windows/Cargo.lock exists, else ""
#   deny       true when windows/deny.toml exists
#   fuzz       true when windows/fuzz/Cargo.toml exists
#   toolchain  the channel pinned in windows/rust-toolchain.toml, or "stable"
# and a short Markdown note to $GITHUB_STEP_SUMMARY when something is missing.
set -euo pipefail

dir=${1:-windows}
out=${GITHUB_OUTPUT:-/dev/stdout}
summary=${GITHUB_STEP_SUMMARY:-/dev/null}

flag() { if [ -e "$1" ]; then echo true; else echo false; fi; }

workspace=$(flag "$dir/Cargo.toml")
deny=$(flag "$dir/deny.toml")
fuzz=$(flag "$dir/fuzz/Cargo.toml")
locked=""
[ -f "$dir/Cargo.lock" ] && locked="--locked"

toolchain=stable
for f in "$dir/rust-toolchain.toml" "$dir/rust-toolchain"; do
  if [ -f "$f" ]; then
    # channel = "1.91.0"   (TOML form)   or a bare channel name (legacy form)
    found=$(sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$f" | head -n 1)
    if [ -z "$found" ] && [ "${f##*.}" != "toml" ]; then
      found=$(head -n 1 "$f" | tr -d '[:space:]')
    fi
    [ -n "$found" ] && toolchain=$found
    break
  fi
done

{
  echo "workspace=$workspace"
  echo "locked=$locked"
  echo "deny=$deny"
  echo "fuzz=$fuzz"
  echo "toolchain=$toolchain"
} >> "$out"

{
  echo "### What exists under \`$dir/\`"
  echo
  echo "| item | present |"
  echo "|---|---|"
  echo "| \`Cargo.toml\` workspace | $workspace |"
  echo "| \`Cargo.lock\` (builds use \`--locked\`) | $([ -n "$locked" ] && echo true || echo false) |"
  echo "| \`deny.toml\` | $deny |"
  echo "| \`fuzz/\` crate | $fuzz |"
  echo "| toolchain | \`$toolchain\` |"
  if [ "$workspace" != true ]; then
    echo
    echo "No cargo workspace yet: the Rust jobs are skipped until \`$dir/Cargo.toml\` lands."
  fi
} >> "$summary"
