---
name: rust-quality-pipeline
description: "Use when setting up or reviewing the test and quality pipeline of a Rust project: hermetic tests (tempdirs, injected clocks, no network, fixture files), property tests with proptest, fuzzing with cargo-fuzz (targets, seed corpus, time-boxed CI runs, crash -> regression test), coverage with cargo-llvm-cov and thresholds, supply-chain checks (cargo-deny, cargo-audit), and GitHub Actions that report results in the job summary and PR comments without downloading artifacts."
tags: [rust, testing, ci, github-actions, fuzzing, cargo-fuzz, proptest, coverage, cargo-llvm-cov, cargo-deny, cargo-audit, quality-gate]
triggers: [fuzz, cargo fuzz, libfuzzer, proptest, property test, coverage, llvm-cov, code coverage, ci pipeline, github actions, step summary, quality gate, hermetic test, flaky test, cargo deny, cargo audit, supply chain]
---

# Rust Quality Pipeline

Layered defenses, cheapest first: **gate** (fmt, clippy, tests) -> **properties** -> **fuzzing** -> **coverage** -> **supply chain**. Every layer reports where a reviewer can see it on the run page, without downloading artifacts.

Rules that apply throughout:
- **Evidence before completion claims.** "Done" means the gate ran and passed, with the command output to show for it. No silent skips; a skipped layer is stated with the reason.
- **Gates stop the line.** Never proceed past a failed gate or red CI.
- **Parallel work uses disjoint write scopes** (separate test files, separate fuzz targets) so independent workstreams do not collide.
- Tests live in `tests/`, not in `#[cfg(test)]` modules in `src/` (see `rust-backend-engineer`).

## 1. The baseline gate

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Also worth a CI job: `cargo check --workspace --no-default-features` (feature combos rot silently) and `cargo doc --no-deps` with `RUSTDOCFLAGS="-D warnings"`.

## 2. Hermetic tests

A test must give the same answer on any machine, offline, in parallel, in any order.

- **Filesystem:** every test gets its own `tempfile::TempDir`; never write to the repo, `$HOME` or a shared `/tmp` path. Keep the `TempDir` alive for the whole test.
- **Time:** inject a clock instead of calling `SystemTime::now()` deep inside logic.

  ```rust
  pub trait Clock: Send + Sync {
      fn now(&self) -> std::time::SystemTime;
  }

  /// Production clock.
  pub struct SystemClock;
  impl Clock for SystemClock {
      fn now(&self) -> std::time::SystemTime { std::time::SystemTime::now() }
  }

  /// Test clock: fixed, advanced explicitly by the test.
  pub struct FixedClock(pub std::sync::Mutex<std::time::SystemTime>);
  impl Clock for FixedClock {
      fn now(&self) -> std::time::SystemTime { *self.0.lock().expect("clock lock") }
  }
  ```

  In async code, `#[tokio::test(start_paused = true)]` plus `tokio::time::advance` tests timeouts and retries without sleeping.
- **Network:** no real hosts. Bind local mocks to `127.0.0.1:0` (`wiremock`, `httpmock`, or a tiny `TcpListener`) and pass the URL in. Pin credential env vars to junk values in any test that could reach a provider, so a developer's real keys never cause live calls. Live-API tests are `#[ignore]`d and opt-in.
- **Fixtures:** check real-shaped samples into `tests/fixtures/` and resolve them from the manifest dir, never the CWD:

  ```rust
  fn fixture(name: &str) -> std::path::PathBuf {
      std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
  }
  ```

  Add `tests/fixtures/** -text` (or `binary`) to `.gitattributes` so Windows checkouts don't rewrite line endings.
- **Process-global state** (env vars, global hooks): serialize those tests behind one await-safe lock, or run them as sequential scenarios inside one test.
- **Randomness:** seed it, and print the seed on failure.

## 3. Property tests (proptest)

```toml
[dev-dependencies]
proptest = "1"
```

```rust
// tests/test_codec_properties.rs
use my_crate::{decode, encode, Message};
use proptest::prelude::*;

proptest! {
    /// Any message survives an encode -> decode roundtrip.
    #[test]
    fn roundtrip(msg in any::<Message>()) {
        let bytes = encode(&msg);
        prop_assert_eq!(decode(&bytes).expect("decode"), msg);
    }

    /// The decoder never panics on arbitrary bytes; it returns Ok or Err.
    #[test]
    fn decode_total(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let _ = decode(&bytes);
    }
}
```

- Good properties: roundtrips, idempotence (`normalize(normalize(x)) == normalize(x)`), invariants after every operation, equivalence with a simple reference implementation.
- Derive strategies with `proptest-derive` (`#[derive(Arbitrary)]`) or write `prop_compose!` strategies that generate *valid* domain values.
- Commit the `proptest-regressions/` files: they replay past failures first on every run.
- In CI raise the case count for a scheduled job: `PROPTEST_CASES=10000 cargo test --test test_codec_properties`.

## 4. Fuzzing (cargo-fuzz / libFuzzer)

Fuzz every parser or decoder of **untrusted input**: file formats, network messages, config, URLs, anything from a user or a remote peer. One target per entry point.

### Layout

```bash
cargo install --locked cargo-fuzz   # pin a version in CI
cargo fuzz init                      # creates fuzz/ (its own crate, outside the workspace)
cargo fuzz add parse_message
```

```text
fuzz/
  Cargo.toml                  # cargo-fuzz = true; depends on the crate by path
  fuzz_targets/
    parse_message.rs
    parse_config.rs
  corpus/parse_message/       # seed corpus: COMMITTED (small, valid, diverse inputs)
  artifacts/                  # crashes: gitignored, uploaded by CI
```

Keep `fuzz/` out of the main workspace (`exclude = ["fuzz"]` in the root `Cargo.toml` if needed) so stable builds never compile it.

```rust
// fuzz/fuzz_targets/parse_message.rs
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Must never panic, hang or allocate without bound, whatever the input.
    if let Ok(msg) = my_crate::parse_message(data) {
        // Stronger oracle: re-encoding a parsed message must parse back equal.
        let again = my_crate::parse_message(&my_crate::encode(&msg)).expect("re-parse");
        assert_eq!(msg, again);
    }
});
```

For structured input, take `|input: MyInput|` where `MyInput: arbitrary::Arbitrary`.

### Running

```bash
cargo +nightly fuzz run parse_message -- -max_total_time=300 -print_final_stats=1
cargo +nightly fuzz run parse_message -- -max_len=65536 -timeout=10 -rss_limit_mb=2048
cargo +nightly fuzz cmin parse_message          # shrink corpus, keep coverage
cargo +nightly fuzz coverage parse_message      # which code the corpus reaches
```

Seed the corpus with real samples (copies of `tests/fixtures/`). A target with an empty corpus wastes most of its budget rediscovering the format.

### From crash to regression test

1. CI (or a local run) writes `fuzz/artifacts/parse_message/crash-<sha1>`.
2. Reproduce: `cargo +nightly fuzz run parse_message fuzz/artifacts/parse_message/crash-<sha1>`.
3. Minimize: `cargo +nightly fuzz tmin parse_message fuzz/artifacts/parse_message/crash-<sha1>` (writes `minimized-from-...`).
4. Copy the minimized input to `tests/fixtures/fuzz/parse_message/<short-description>.bin` and add it to the corpus too.
5. A deterministic stable-toolchain test replays every saved input:

   ```rust
   // tests/test_fuzz_regressions.rs
   /// Every input that once crashed `parse_message` must now return Ok or Err.
   #[test]
   fn test_parse_message_fuzz_regressions() {
       let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
           .join("tests/fixtures/fuzz/parse_message");
       for entry in std::fs::read_dir(&dir).expect("fixture dir") {
           let path = entry.expect("dir entry").path();
           let bytes = std::fs::read(&path).expect("read fixture");
           let _ = my_crate::parse_message(&bytes); // panics fail the test
       }
   }
   ```

6. Fix the root cause (and every place the same pattern occurs), then re-run the target from the updated corpus.

## 5. Coverage (cargo-llvm-cov)

```bash
cargo install --locked cargo-llvm-cov   # or taiki-e/install-action in CI
cargo llvm-cov --workspace --all-features \
  --ignore-filename-regex '(^|/)(tests|benches|fuzz)/|src/ui/|src/ffi/' \
  --lcov --output-path lcov.info
cargo llvm-cov report --json --summary-only --output-path cov.json   # reuse the same run
cargo llvm-cov report --summary-only                                  # text table
cargo llvm-cov report --fail-under-lines 80                           # global threshold
```

- **Per-crate thresholds:** run `cargo llvm-cov -p core_crate --fail-under-lines 85`, or filter `cov.json` files by path prefix (script below). Core logic gets a high bar; glue crates a lower one.
- **Exclude thoughtfully:** UI view code, FFI shims and generated code can be excluded *only* when the logic they wrap lives in tested modules. Write the exclusion reason next to the regex. Never exclude a file just to pass the threshold.
- Coverage measures what ran, not what was checked: pair it with assertions, properties and fuzzing.

## 6. GitHub Actions

Principles: pin every third-party action to a **full commit SHA** (with the tag in a comment); pin tool versions; `permissions: contents: read` by default, widening per job only where needed; `concurrency` to cancel superseded runs; `fail-fast: false` in matrices.

Resolve a tag to its SHA once, then paste it:

```bash
gh api repos/actions/checkout/commits/v4.2.2 --jq .sha
```

```yaml
# .github/workflows/quality.yml
name: quality
on:
  pull_request:
  push:
    branches: [main]
  schedule:
    - cron: "17 3 * * *" # nightly long fuzz + proptest
permissions:
  contents: read
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true
env:
  CARGO_TERM_COLOR: always
  RUST_TOOLCHAIN: "1.89.0"       # pin; bump deliberately
  CARGO_LLVM_COV_VERSION: "0.6.x" # replace with an exact version you verified
  CARGO_FUZZ_VERSION: "0.12.x"    # replace with an exact version you verified

jobs:
  test:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@<sha> # v4.x
      - uses: dtolnay/rust-toolchain@<sha> # master
        with:
          toolchain: ${{ env.RUST_TOOLCHAIN }}
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@<sha> # v2.x
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets --all-features -- -D warnings
      - name: Tests
        shell: bash
        run: |
          set -o pipefail
          cargo test --workspace --all-features 2>&1 | tee test.log
      - name: Test summary
        if: always()
        shell: bash
        run: |
          passed=$(grep -Eo '[0-9]+ passed' test.log | awk '{s+=$1} END {print s+0}')
          failed=$(grep -Eo '[0-9]+ failed' test.log | awk '{s+=$1} END {print s+0}')
          ignored=$(grep -Eo '[0-9]+ ignored' test.log | awk '{s+=$1} END {print s+0}')
          {
            echo "### Tests (${{ matrix.os }})"
            echo "| passed | failed | ignored |"
            echo "|---:|---:|---:|"
            echo "| $passed | $failed | $ignored |"
            if [ "$failed" -gt 0 ]; then
              echo; echo "**Failing tests**"; echo '```'
              grep -E '^test .* FAILED$' test.log | head -50
              echo '```'
            fi
          } >> "$GITHUB_STEP_SUMMARY"
```

For richer per-test reporting, use `cargo nextest run --profile ci` with JUnit output enabled in `.config/nextest.toml` and render the XML into the summary.

### Coverage job with summary table and delta gate

```yaml
  coverage:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<sha> # v4.x
      - uses: dtolnay/rust-toolchain@<sha>
        with: { toolchain: "${{ env.RUST_TOOLCHAIN }}", components: llvm-tools-preview }
      - uses: Swatinem/rust-cache@<sha>
      - uses: taiki-e/install-action@<sha>
        with: { tool: "cargo-llvm-cov@${{ env.CARGO_LLVM_COV_VERSION }}" }
      - run: |
          cargo llvm-cov --workspace --all-features \
            --ignore-filename-regex '(^|/)(tests|benches|fuzz)/' \
            --lcov --output-path lcov.info
          cargo llvm-cov report --json --summary-only --output-path cov.json
      - name: Coverage summary
        run: |
          {
            echo "### Coverage"
            jq -r '.data[0].totals | "**Total:** lines \(.lines.percent*100|floor/100)% · functions \(.functions.percent*100|floor/100)% · regions \(.regions.percent*100|floor/100)%"' cov.json
            echo
            echo "| file | lines | % |"
            echo "|---|---:|---:|"
            jq -r --arg root "$PWD/" '.data[0].files | sort_by(.summary.lines.percent)[]
              | "| \(.filename | ltrimstr($root)) | \(.summary.lines.covered)/\(.summary.lines.count) | \(.summary.lines.percent*100|floor/100) |"' cov.json
          } >> "$GITHUB_STEP_SUMMARY"
          jq '.data[0].totals.lines.percent' cov.json > coverage-total.txt
      - uses: actions/upload-artifact@<sha> # v4.x  (kept for the delta gate and for local tools)
        with: { name: coverage, path: "lcov.info\ncoverage-total.txt" }
      - name: Coverage delta vs base
        if: github.event_name == 'pull_request'
        env:
          GH_TOKEN: ${{ github.token }}
          BASE: ${{ github.base_ref }}
          MAX_DROP: "0.5" # percentage points
        run: |
          run_id=$(gh run list --workflow quality.yml --branch "$BASE" --status success \
                   --limit 1 --json databaseId --jq '.[0].databaseId')
          if [ -z "$run_id" ] || ! gh run download "$run_id" -n coverage -D base; then
            echo "No base coverage found; delta gate skipped." >> "$GITHUB_STEP_SUMMARY"; exit 0
          fi
          head=$(cat coverage-total.txt); base=$(cat base/coverage-total.txt)
          delta=$(echo "$head - $base" | bc -l)
          printf '**Line coverage:** %.2f%% (base %.2f%%, delta %+.2f pp)\n' "$head" "$base" "$delta" >> "$GITHUB_STEP_SUMMARY"
          if (( $(echo "$delta < -$MAX_DROP" | bc -l) )); then
            echo "Coverage dropped more than $MAX_DROP pp" >> "$GITHUB_STEP_SUMMARY"; exit 1
          fi
```

Per-crate threshold from the same JSON (path prefix -> minimum):

```bash
check() { # $1 = path prefix, $2 = minimum line %
  jq -r --arg p "$PWD/$1" '[.data[0].files[] | select(.filename|startswith($p)) | .summary.lines]
    | (map(.covered)|add) as $c | (map(.count)|add) as $n | if $n>0 then $c*100/$n else 100 end' cov.json \
  | awk -v min="$2" -v p="$1" '{printf "| %s | %.2f%% | %s%% |\n", p, $1, min; if ($1 < min) exit 1}' \
  >> "$GITHUB_STEP_SUMMARY"
}
echo "| crate | lines | min |" >> "$GITHUB_STEP_SUMMARY"; echo "|---|---:|---:|" >> "$GITHUB_STEP_SUMMARY"
check crates/core/ 85 && check crates/cli/ 60
```

### Fuzz job (Linux, nightly, time-boxed)

```yaml
  fuzz:
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        target: [parse_message, parse_config]
    steps:
      - uses: actions/checkout@<sha>
      - uses: dtolnay/rust-toolchain@<sha>
        with: { toolchain: nightly-2025-06-01 } # pin a dated nightly
      - uses: Swatinem/rust-cache@<sha>
        with: { workspaces: fuzz }
      - run: cargo install --locked cargo-fuzz --version "$CARGO_FUZZ_VERSION"
      - name: Fuzz ${{ matrix.target }}
        env:
          SECS: ${{ github.event_name == 'schedule' && '1800' || '120' }}
        run: |
          set +e
          cargo fuzz run ${{ matrix.target }} -- -max_total_time=$SECS -print_final_stats=1 2>&1 | tee fuzz.log
          status=${PIPESTATUS[0]}
          execs=$(grep -Eo 'stat::number_of_executed_units: [0-9]+' fuzz.log | awk '{print $2}')
          rate=$(grep -Eo 'stat::average_exec_per_sec: [0-9]+' fuzz.log | awk '{print $2}')
          corpus=$(ls fuzz/corpus/${{ matrix.target }} 2>/dev/null | wc -l)
          crashes=$(ls fuzz/artifacts/${{ matrix.target }} 2>/dev/null | wc -l)
          {
            echo "### Fuzz \`${{ matrix.target }}\` (${SECS}s)"
            echo "| execs | execs/sec | corpus files | crashes |"
            echo "|---:|---:|---:|---:|"
            echo "| ${execs:-0} | ${rate:-0} | $corpus | $crashes |"
            if [ "$crashes" -gt 0 ]; then
              echo; echo "**Crash input (base64, first artifact):**"; echo '```'
              base64 -w0 "$(ls -d fuzz/artifacts/${{ matrix.target }}/* | head -1)" | head -c 4000; echo
              echo '```'
              grep -m1 -A15 -E 'panicked at|ERROR: libFuzzer' fuzz.log | sed 's/^/    /'
            fi
          } >> "$GITHUB_STEP_SUMMARY"
          exit $status
      - if: failure()
        uses: actions/upload-artifact@<sha>
        with: { name: "fuzz-artifacts-${{ matrix.target }}", path: fuzz/artifacts }
```

The base64 crash input in the summary lets a developer reproduce with `echo <b64> | base64 -d > crash.bin` without downloading anything.

### Supply chain

```yaml
  supply-chain:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<sha>
      - uses: EmbarkStudios/cargo-deny-action@<sha> # v2.x
        with: { command: check advisories bans licenses sources }
      - uses: taiki-e/install-action@<sha>
        with: { tool: cargo-audit }
      - run: cargo audit --deny warnings
```

`deny.toml` holds allowed licenses, banned or duplicate crates and allowed registries/git sources; every `ignore` of an advisory carries a reason and a review date. Run this job on a schedule too: new advisories appear without code changes.

### PR comment (optional)

```yaml
  report:
    if: github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository
    needs: [coverage]
    runs-on: ubuntu-latest
    permissions: { pull-requests: write }
    steps:
      - env: { GH_TOKEN: "${{ github.token }}", PR: "${{ github.event.pull_request.number }}" }
        run: |
          echo "Quality report: ${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}" > body.md
          gh pr comment "$PR" -R "${{ github.repository }}" --body-file body.md --edit-last \
            || gh pr comment "$PR" -R "${{ github.repository }}" --body-file body.md
```

Pass the coverage markdown between jobs via job outputs or a small artifact if you want it in the comment. Fork PRs get a read-only token, hence the `if:` guard; the step summary still works for them.

### Matrix notes

- **Linux:** runs everything; the only place fuzzing and coverage gates run.
- **macOS:** tests and clippy; libFuzzer works locally on macOS for developers.
- **Windows:** tests and clippy. Use `shell: bash` for the summary scripts; watch path separators, CRLF in fixtures, file locking (files cannot be deleted while open) and long paths. Skip fuzzing on Windows.
- Platform-specific code paths need tests on that platform; `#[cfg(windows)]` code compiled only on Linux is untested code.

## 7. Review checklist

- [ ] Gate commands run locally and in CI; results quoted when claiming done
- [ ] No test touches the network, real clock, `$HOME` or CWD-relative paths
- [ ] Every untrusted-input parser has a fuzz target with a committed seed corpus
- [ ] Every fuzz crash became a minimized fixture plus a stable-toolchain regression test
- [ ] `proptest-regressions/` committed
- [ ] Coverage thresholds per crate; exclusions justified in place
- [ ] Summaries visible on the run page: tests, coverage table, fuzz stats, delta
- [ ] Actions pinned by SHA, tools and toolchains pinned, least-privilege `permissions`
- [ ] cargo-deny and cargo-audit on PRs and on a schedule

## Git / Commit

Every commit message ends with the authoring model attribution trailer the harness specifies, e.g. `Co-Authored-By: <model name> <noreply@...>`.
