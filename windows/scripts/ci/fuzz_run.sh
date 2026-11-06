#!/usr/bin/env bash
# Run every cargo-fuzz target (or the ones named) for a fixed time and record
# one JSON row per target for fuzz_report.py.
#
#   fuzz_run.sh --seconds 60 --out <dir> [--cmin] [target...]
#
# Run from windows/ (the fuzz crate is windows/fuzz). Uses `cargo fuzz`, so
# set RUSTUP_TOOLCHAIN to the pinned nightly. For each target it:
#   1. fuzzes from the committed corpus fuzz/corpus/<target> for --seconds,
#   2. on a crash, minimizes the input with `cargo fuzz tmin` (time-boxed),
#   3. with --cmin, shrinks the corpus to the inputs that add coverage,
#   4. writes <out>/rows/<target>.json and <out>/logs/<target>.log, and copies
#      crash inputs to <out>/crashes/<target>/.
# The script exits 0 when the targets ran, whatever they found:
# `fuzz_report.py table` decides pass or fail. It exits 1 only when the fuzz
# crate does not build, leaving the build log in <out>/build-failed.log.
set -uo pipefail

seconds=60
out=""
cmin=0
targets=()
while [ $# -gt 0 ]; do
  case "$1" in
    --seconds) seconds=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    --cmin) cmin=1; shift ;;
    -*) echo "unknown option $1" >&2; exit 2 ;;
    *) targets+=("$1"); shift ;;
  esac
done
[ -n "$out" ] || { echo "usage: fuzz_run.sh --seconds N --out DIR [--cmin] [target...]" >&2; exit 2; }

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
fuzz_dir=fuzz
# cargo-fuzz defaults --target to the triple it was itself built for; the
# release binary is musl, and ASan cannot link a static musl libc. Always
# fuzz for the toolchain's host triple instead.
host=$(rustc -vV | sed -n 's/^host: //p')
target_args=(--target "$host")
mkdir -p "$out/rows" "$out/logs" "$out/crashes"

if [ ${#targets[@]} -eq 0 ]; then
  # A failing `cargo fuzz list` (e.g. a broken manifest) must not read as
  # "no targets": record it as a build failure.
  if ! listing=$(cargo fuzz list --fuzz-dir "$fuzz_dir" 2>"$out/logs/list.log"); then
    cat "$out/logs/list.log"
    cp "$out/logs/list.log" "$out/build-failed.log"
    exit 1
  fi
  mapfile -t targets <<<"$listing"
  [ -n "${targets[0]:-}" ] || targets=()
fi
if [ ${#targets[@]} -eq 0 ]; then
  echo "No fuzz targets."
  exit 0
fi

# Build first so the time budget is spent fuzzing, not compiling: the one
# target when a single one is named (nightly matrix), otherwise all of them.
build_args=()
[ ${#targets[@]} -eq 1 ] && build_args=("${targets[0]}")
echo "::group::cargo fuzz build"
if ! cargo fuzz build --fuzz-dir "$fuzz_dir" "${target_args[@]}" "${build_args[@]}" 2>&1 | tee "$out/logs/build.log"; then
  echo "::endgroup::"
  # fuzz_report.py table --build-failed-log shows this instead of an empty table.
  cp "$out/logs/build.log" "$out/build-failed.log"
  exit 1
fi
echo "::endgroup::"

for target in "${targets[@]}"; do
  corpus="$fuzz_dir/corpus/$target"
  artifacts="$fuzz_dir/artifacts/$target"
  log="$out/logs/$target.log"
  mkdir -p "$corpus"
  rm -rf "$artifacts"
  before=$(find "$corpus" -maxdepth 1 -type f | wc -l | tr -d ' ')

  echo "::group::fuzz $target (${seconds}s, corpus $before)"
  started=$SECONDS
  cargo fuzz run --fuzz-dir "$fuzz_dir" "${target_args[@]}" "$target" "$corpus" -- \
    -max_total_time="$seconds" -print_final_stats=1 -timeout=10 -rss_limit_mb=2048 \
    >"$log" 2>&1
  status=$?
  elapsed=$((SECONDS - started))
  tail -n 40 "$log"
  echo "::endgroup::"

  crash_args=()
  original=$(find "$artifacts" -maxdepth 1 -type f \
    \( -name 'crash-*' -o -name 'leak-*' -o -name 'timeout-*' -o -name 'oom-*' \) 2>/dev/null | head -n 1)
  if [ -n "$original" ]; then
    mkdir -p "$out/crashes/$target"
    cp "$original" "$out/crashes/$target/"
    reported=$original
    # Minimize; tmin keeps the original when it cannot shrink it.
    if timeout 120 cargo fuzz tmin --fuzz-dir "$fuzz_dir" "${target_args[@]}" -r 2000 "$target" "$original" \
        >"$out/logs/$target.tmin.log" 2>&1; then
      minimized=$(find "$artifacts" -maxdepth 1 -type f -name 'minimized-from-*' | head -n 1)
      if [ -n "$minimized" ]; then
        cp "$minimized" "$out/crashes/$target/"
        reported=$minimized
      fi
    fi
    crash_args=(--crash "$reported" --original-crash "$original")
  elif [ "$cmin" = 1 ] && [ "$status" = 0 ]; then
    echo "::group::cmin $target"
    cargo fuzz cmin --fuzz-dir "$fuzz_dir" "${target_args[@]}" "$target" "$corpus" >"$out/logs/$target.cmin.log" 2>&1 \
      || echo "cmin failed for $target (corpus kept as is)"
    tail -n 5 "$out/logs/$target.cmin.log"
    echo "::endgroup::"
  fi

  python3 -I "$here/fuzz_report.py" row --target "$target" --log "$log" --corpus-dir "$corpus" \
    --corpus-before "$before" --seconds "$seconds" --exit-code "$status" --elapsed "$elapsed" \
    "${crash_args[@]}" --out "$out/rows/$target.json"
done
exit 0
