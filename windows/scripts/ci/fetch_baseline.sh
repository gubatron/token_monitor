#!/usr/bin/env bash
# Download the newest unexpired artifact NAME produced on BRANCH by another run
# and unpack it into DEST. Used for the coverage delta (and the nightly corpus).
#
#   fetch_baseline.sh <artifact-name> <dest-dir> [branch]
#
# BRANCH defaults to windows-port; "*" accepts an artifact from any branch.
# Needs GH_TOKEN with actions: read, GITHUB_REPOSITORY and GITHUB_RUN_ID;
# PYTHON names the interpreter that unzips (default python3).
# Writes found=true|false, run_id and run_url to $GITHUB_OUTPUT (or stdout).
# Exit status is always 0: a missing baseline only skips the comparison.
set -uo pipefail

name=${1:?usage: fetch_baseline.sh <artifact-name> <dest-dir> [branch]}
dest=${2:?usage: fetch_baseline.sh <artifact-name> <dest-dir> [branch]}
branch=${3:-windows-port}
out=${GITHUB_OUTPUT:-/dev/stdout}
repo=${GITHUB_REPOSITORY:?}
self=${GITHUB_RUN_ID:-0}

# Artifacts come back newest first.
row=$(gh api "repos/$repo/actions/artifacts?name=$name&per_page=100" --jq "
  [.artifacts[]
   | select(.expired | not)
   | select(\"$branch\" == \"*\" or .workflow_run.head_branch == \"$branch\")
   | select(.workflow_run.id != $self)][0]
  | select(. != null) | [.id, .workflow_run.id, .workflow_run.head_sha] | @tsv" 2>/dev/null) || row=""

if [ -z "$row" ]; then
  echo "No earlier '$name' artifact on $branch."
  echo "found=false" >> "$out"
  exit 0
fi

read -r artifact_id run_id head_sha <<<"$row"
mkdir -p "$dest"
zip="$dest/.artifact.zip"
if gh api "repos/$repo/actions/artifacts/$artifact_id/zip" > "$zip" 2>/dev/null \
    && "${PYTHON:-python3}" -I -m zipfile -e "$zip" "$dest"; then
  rm -f "$zip"
  echo "Baseline: artifact $artifact_id from run $run_id (${head_sha:0:7})."
  {
    echo "found=true"
    echo "run_id=$run_id"
    echo "run_url=${GITHUB_SERVER_URL:-https://github.com}/$repo/actions/runs/$run_id"
    echo "sha=${head_sha:0:7}"
  } >> "$out"
else
  rm -f "$zip"
  echo "Could not download artifact $artifact_id; continuing without a baseline."
  echo "found=false" >> "$out"
fi
exit 0
