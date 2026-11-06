---
name: rust-criterion-benchmarking
description: "Use when writing, running or interpreting Criterion benchmarks for Rust: benchmark layout, groups and throughput, fixtures and isolation, saved baselines, capturing output, reading confidence intervals, and common gotchas."
tags: [rust, benchmarks, criterion, performance, testing, measurement]
triggers: [benchmark, cargo bench, criterion, throughput, latency, performance measurement, regression, baseline, black_box]
---

# Rust Criterion Benchmarking

Use Criterion to measure Rust code with statistical rigor. Never rely on intuition — let the numbers decide.

## Project Setup

```toml
# Cargo.toml
[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
tempfile = "3" # isolated filesystem fixtures

[[bench]]
name = "my_bench"
harness = false # REQUIRED: disables the default libtest harness
```

Use the current Criterion release your workspace allows; the API below is stable across recent versions.

## File Structure

```text
benches/
  my_bench.rs   # harness = false entry point
  another.rs
tests/          # tests go here, NOT in benches/
```

## Benchmark File Skeleton

```rust
//! What this file benchmarks and how to run it.
//!
//! Run: cargo bench --bench my_bench
//! HTML report: target/criterion/report/index.html

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

fn bench_single_op(c: &mut Criterion) {
    c.bench_function("my_op", |b| {
        b.iter(|| my_expensive_fn(black_box(42)));
    });
}

fn bench_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_throughput");
    for size in [1usize, 10, 100, 1_000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("process", size), &size, |b, &n| {
            b.iter(|| {
                for _ in 0..n {
                    black_box(my_expensive_fn(black_box(n)));
                }
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_single_op, bench_batch);
criterion_main!(benches);
```

Returning a value from the `iter` closure already keeps it alive; `black_box` the inputs so the compiler cannot constant-fold them. Prefer `std::hint::black_box` (newer Criterion versions deprecate their own).

## Running

```bash
# All benches in the crate
cargo bench

# One bench file
cargo bench --bench my_bench

# One group or function (regex filter)
cargo bench --bench my_bench -- batch_throughput
cargo bench --bench my_bench -- "batch_throughput/process/100"

# Capture output: use tee (see Gotchas)
cargo bench 2>&1 | tee bench_results.txt
```

## Baselines

Name baselines instead of relying on "the last run":

```bash
git switch main && cargo bench -- --save-baseline main
git switch my-branch && cargo bench -- --baseline main
```

Keep the machine state comparable (AC power, no other heavy processes, same RUSTFLAGS and profile). For CI regression gates prefer instruction-count tools (`iai-callgrind`), which are immune to noisy shared runners; wall-clock Criterion on shared CI runners is informative, not a gate.

## HTML Report

Criterion writes reports under `target/criterion/` (per-benchmark `report/index.html` plus an index at `target/criterion/report/index.html`): violin plots, PDFs and comparisons with the previous run. They are produced even if stdout was swallowed.

## Latency vs. Throughput

For batch operations set `Throughput::Elements(n)` (or `Throughput::Bytes(n)`) so Criterion reports elements/s or bytes/s, not just time per iteration:

```rust
group.throughput(Throughput::Elements(batch_size as u64));
```

For single operations `bench_function` is sufficient.

## Fixtures and Isolation

Use `tempfile::TempDir` for disk-backed fixtures:

```rust
use tempfile::TempDir;

fn make_fixture() -> (MyStore, TempDir) {
    let dir = tempfile::Builder::new().prefix("bench-").tempdir().expect("tempdir");
    let store = MyStore::open(dir.path()).expect("open store");
    (store, dir) // return dir to keep it alive: drop = delete
}
```

Use `iter_batched` when the fixture must be fresh per iteration; setup time is excluded from the measurement:

```rust
b.iter_batched(
    make_fixture,
    |(mut store, _dir)| store.write(black_box(42)),
    criterion::BatchSize::SmallInput,
);
```

Use `BatchSize::LargeInput` (or `PerIteration`) when each input is big, so Criterion does not hold many in memory at once.

## Interpreting Results

```text
my_op   time:   [1.234 µs 1.250 µs 1.267 µs]
        change: [-3.2% -1.5% +0.1%] (p = 0.12 > 0.05)
        No change in performance detected.
```

- The three numbers are the lower bound, point estimate and upper bound of the confidence interval.
- `change` is the relative delta against the comparison baseline.
- `p > 0.05` means not statistically significant: treat as noise.
- "Performance has regressed" does not mean the code is broken — the stored baseline may have been taken on an idle machine.

**Golden rule:** compare absolute numbers across controlled, comparable runs, not just Criterion's delta label. Microbenchmarks can lie; validate important wins on a realistic workload.

## Gotchas

1. **Output capture:** `cargo bench > file.txt` can lose Criterion's progress output; use `cargo bench 2>&1 | tee file.txt`.
2. **Baseline drift:** two runs of the same code under different load will show a "regression". Save named baselines and record the numbers that matter alongside the change that produced them.
3. **`black_box`:** without it the compiler may optimize away the work you think you are measuring.
4. **`harness = false`:** without it the bench target uses libtest and Criterion's `main` will not run.
5. **TempDir lifetime:** dropping the `TempDir` before the benchmark loop deletes the directory mid-benchmark. Keep it in scope.
6. **HTTP / end-to-end load:** Criterion is for in-process microbenchmarks. For concurrent request load and p99 latency, write a `harness = false` bench binary (Tokio tasks + an HTTP client) or use a dedicated load tool.
7. **Release settings:** `cargo bench` uses the `bench` profile (inherits `release`); make sure LTO/codegen settings match what you ship.

## Git / Commit

Every commit message ends with the authoring model attribution trailer the harness specifies, e.g. `Co-Authored-By: <model name> <noreply@...>`.
