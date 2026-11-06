---
name: rust-performance-reviewer
description: "Use when reviewing Rust code for performance and micro-optimizations — hot-path audits, allocation and cache-footprint review, API design for speed, database query plans, batch/scheduled pipeline cost, Criterion-backed regressions, or flat profiles with no obvious hotspot. Inspired by Abseil Performance Hints (Jeff Dean & Sanjay Ghemawat). Pair with rust-backend-engineer (standards), rust-criterion-benchmarking (measurement) and rust-concurrency-patterns (locks)."
tags: [rust, performance, micro-optimization, code-review, cache, allocations, criterion, hot-path, abseil, sqlite, database, batch-jobs]
triggers: [performance review, micro-optimize, hot path, allocation profile, cache miss, flat profile, speed up rust, reduce allocations, criterion, pprof, perf, SmallVec, zero-copy, bulk API, rust performance, sqlite slow, EXPLAIN QUERY PLAN, slow query, N+1, scheduled job slow]
---

# Rust Performance Reviewer

Disciplined performance review of Rust code through the lens of Jeff Dean & Sanjay Ghemawat's **Performance Hints** (Abseil, 2025 — https://abseil.io/fast/hints.html), translated to idiomatic Rust.

**Scope:** single-binary / library performance — CPU, memory, cache, allocations, locks, API shape, plus the local I/O (database, files, HTTP) that usually dominates wall clock. Not distributed-systems or ML-hardware tuning.

**Core principle (Knuth, full quote):** forget about small efficiencies ~97% of the time — *but do not pass up the critical 3%*. An easily obtained 12% gain is never marginal in quality software. Prefer the faster alternative when it does not significantly hurt readability.

**Evidence before claims.** Every finding cites `file:line`, states whether the code is on a **hot path**, and proposes one concrete fix. Unmeasured "this looks slow" on a cold path is Minor at most.

Companion skills: `rust-backend-engineer` (idioms, types, workflow), `rust-criterion-benchmarking` (how to measure), `rust-concurrency-patterns` (locks, sharded maps, write batching).

---

## When to engage

- Asked for a performance review, micro-optimizations, or "make it faster"
- A profile or benchmark shows a hotspot, or a flat profile after the low-hanging fruit
- Designing library APIs that many call sites will use
- Pre-merge audit of hot request / parse / encode / batch paths
- High RSS, allocator pressure, or cache-miss-bound code

**Do not** micro-optimize test-only code beyond asymptotics, or one-shot scripts. Never sacrifice correctness or introduce a second mechanism for speed.

**Correctness bugs often *are* the performance story.** A feature that "never fires" (an always-false flag, a comparison made after the value it compares against was overwritten) looks like lag or missing work. When a user reports "X is missing / late", inspect live state and the predicates that gate the work before rewriting loops, and pin the bug with a regression test.

---

## Philosophy (Abseil -> Rust)

1. **Think about performance while writing**, not only after profiles go flat. Flat profiles mean the cost is smeared everywhere.
2. **Library code is high leverage.** Callers cannot fix your internals; choose good defaults (`&str`, bulk APIs, `SmallVec`) with low local complexity.
3. **Estimate before implementing.** Back-of-envelope with known costs; discard alternatives that cannot win.
4. **Measure before claiming.** Criterion for micro; `perf`/`samply` for CPU profiles; allocation profiles when the allocator is suspect.
5. **Many 1% wins compound** — given stable benchmarks.
6. **Deep modules.** Narrow public interfaces keep layout/allocation changes inside encapsulation boundaries.
7. **Don't pay for what you don't use.** Thread safety, generality, stats and logging on hot paths all cost.

### Rough cost table (order of magnitude, modern x86)

| Operation | ~Cost |
|---|---|
| L1 reference | 0.5 ns |
| L2 reference | 3 ns |
| Branch mispredict | 5 ns |
| Uncontended mutex lock/unlock | 15 ns |
| Main memory reference | 50 ns |
| Compress 1 KB (Snappy-class) | 1 µs |
| SSD 4 KB read | 20 µs |
| Same-DC network RTT | 50 µs |
| 1 MB sequential DRAM read | 64 µs |
| 1 MB over 100 Gbps network | 100 µs |
| Disk seek | 5 ms |
| Transcontinental RTT | 150 ms |

Track *your* higher-level costs too: a SQL point read, an HTTP hop, a template render, serde of a typical payload, a process spawn.

**Worked example:** scanning 1M strings for a prefix. ~50 MB of data => memory-bound floor ≈ 50 MB / 16 GB/s ≈ 3 ms. If the compare branch is data-dependent, ~50% of 1M branches mispredict => 0.5M × 5 ns ≈ 2.5 ms extra. Bandwidth and mispredicts are the same order, so (a) keep data compact (arena / `Box<str>`, not `Vec<String>` with capacity slack) and (b) a branch-free `memchr`/SIMD scan can roughly halve the time. The estimate picks the fix before any code is written.

---

## Review method

1. **Scope.** Crate, module, function or diff? Hot path vs init/setup?
2. **Classify.** Test-only -> asymptotics only. App-specific -> is it per request / per item / per scheduled run? Library -> apply cheap techniques aggressively.
3. **Structural first.** Wrong query plan, N+1 I/O, unbounded queues, full re-scans before a cheap gate — fix these before allocation polish. A query-plan fix routinely beats every Rust micro-optimization in the same path by orders of magnitude.
4. **Estimate** the dominant operations (allocations, copies, syscalls, locks, branches, bytes, round trips).
5. **Measure** when tradeoffs are non-obvious.
6. **Apply the lenses** in order: algorithmic -> memory representation -> allocations -> avoided work -> compiler help -> code size -> concurrency -> serialization -> containers -> database -> batch pipelines.
7. **Report** with severity, evidence, impact class, one fix.
8. **Gate Important+ claims** with a benchmark or profile delta.

### Severity

- **Critical** — pathological complexity on a production path (O(n²) per request), unbounded memory, a lock held across network I/O, allocation/parallelism that can exhaust the host (e.g. unbounded worker fan-out or uncapped parallel builds on a shared machine).
- **Important** — measurable hot-path waste: needless alloc/copy per request, wrong container, missing `reserve`, regex where a prefix match suffices, missing bulk API forcing N boundary crossings, contended global lock, false sharing, wrong query plan.
- **Minor** — cold-path micro-opts, style-level `clone` cleanup, small layout polish, speculative SIMD. Batch only with approval.

### Impact classes

Structural/algorithmic (often 2–10×+) · Representation/cache (10–50% when data is large) · Allocation reduction (10–30% on alloc-heavy paths) · Avoided work/fast path (variable, can be huge) · Lock/parallel (multi-core throughput) · Death by 1000 cuts (many 1% changes on a flat profile).

---

## Lens 1 — Algorithmic improvements

- [ ] Wrong asymptotics? Nested loops, repeated full scans, sorting when hashing works
- [ ] Sorted intersection -> hash-set lookup; ordered map used only for equality -> `HashMap`
- [ ] Incremental build with per-edge checks -> one-shot construction
- [ ] Hash quality: expected O(1) not accidentally O(n)

Rust notes:
- Internal (non-adversarial) keys on hot maps: `rustc_hash::FxHashMap` or `foldhash`. Keep SipHash for attacker-controlled keys.
- `entry()` instead of `contains_key` + `insert` (double hash and lookup).
- `swap_remove` (O(1)) when order doesn't matter; `VecDeque` instead of `Vec::remove(0)`.
- `sort_unstable` when stability isn't needed; `sort_by_cached_key` for expensive keys.
- Moderate-N ordered lookups: sorted `Vec` + `binary_search` often beats `BTreeMap`.
- Build in bulk (`extend`, `collect`) instead of repeated inserts.

## Lens 2 — Memory representation

Touch fewer cache lines.

- Smaller integer types when the domain fits; `#[repr(u8)]` enums when it matters
- Bitsets (`bitvec`, `fixedbitset`, masks) instead of `HashSet<SmallId>`; dense arrays indexed by small ids instead of maps
- `Box<str>` / `Box<[T]>` for stored immutable data (drops the capacity word); `Arc<str>` / `Arc<[T]>` for shared immutable data (no double indirection of `Arc<String>`)
- Enum size = largest variant + discriminant: `Box` large cold variants
- Field order by access; cold fields last or behind `Box`; separate hot read-mostly from hot mutable data
- Check sizes with `static_assertions::assert_eq_size!` and `cargo +nightly rustc -- -Zprint-type-sizes`
- Indices instead of pointers: `u32` indices into a `Vec`/arena; `slab` / `slotmap` for generational keys
- Avoid per-element heap nodes (`LinkedList`, naive trees) on hot data
- Small-N collections: `SmallVec`, `ArrayVec`, `tinyvec`; short strings: `compact_str`, `smartstring` (caveat: large inline N bloats every instance)
- `HashMap<A, HashMap<B, V>>` -> often `HashMap<(A, B), V>`; measure both
- Arenas (`bumpalo`, `typed-arena`) for many short-lived related objects; beware long-lived arenas holding junk

## Lens 3 — Reduce allocations

Allocation cost = allocator time + init/drop + cache footprint.

- [ ] Static/empty sentinel instead of allocating (`&[]`, `Cow::Borrowed("")`)
- [ ] `with_capacity` / `reserve` when the size is known; never grow one-by-one in a loop
- [ ] Move (`mem::take`, ownership transfer) instead of `clone`
- [ ] Hoist buffers out of loops; reuse `Vec`/`String`/`BytesMut` with `clear` (reused buffers keep high-water capacity: `shrink_to` periodically if sizes vary wildly)
- [ ] Iterator pipelines without intermediate `collect`
- [ ] `Cow<'_, str>` when sometimes borrowed, sometimes owned
- [ ] `Rc` instead of `Arc` in provably single-threaded code

`iter.map(f).collect::<Vec<_>>()` reserves from `size_hint`; `iter.filter(f).collect()` does not.

Hot-path smells:
- `.clone()` on `String`/`Vec`/`HashMap` per item
- `format!` / `to_string()` just to pass to an API taking `impl AsRef<str>`
- `format!` chains in a loop -> `write!` into one reused `String`
- `to_lowercase() == other` -> `eq_ignore_ascii_case` (ASCII caveat)
- `Regex::new` inside a function -> compile once in a `LazyLock`; better, `starts_with`/`split_once`/`memchr`
- Serialize-then-parse round trips when a typed value already exists
- `format!` for numbers on ultra-hot paths -> `itoa` / `ryu`
- `s.as_bytes().to_vec()` where `s.into_bytes()` moves
- `Arc<Mutex<u64>>` counters -> `AtomicU64`

**Allocator swap** (`mimalloc`, `tikv-jemallocator` via `#[global_allocator]`) is often a bigger win than any single code change for allocation-heavy multi-threaded services. Benchmark on the real workload; it also changes RSS behavior.

## Lens 4 — Avoid unnecessary work

- **Fast paths:** common case branch-predictable and allocation-free; slow path in a `#[cold]` function
- **Precompute once:** lookup tables, compiled regexes (`LazyLock`/`OnceLock`); validate at module boundaries, not deep inside
- **Hoist from loops:** bounds, formatting, config flags, log-enabled checks
- **Defer:** don't compute what no consumer reads yet
- **Specialize:** `memchr`/`memmem` for search, `aho-corasick` for many patterns, prefix checks instead of regex
- **Reuse loaded inputs** within one process instead of re-reading and re-parsing the same file per step
- **Logging/stats:** even disabled logging costs a branch; use `max_level_*`/`release_max_level_*` features, sample stats, relaxed atomics for counters, drop useless counters. Per-item `info!` across thousands of items per run is a disk and I-cache tax: log one summary, per-item at `debug`/`trace`.

## Lens 5 — Help the compiler

Only when profiles show pain; inspect assembly (`cargo-show-asm`, `perf annotate`, Godbolt).

- Avoid `dyn Trait` in the hottest loops when monomorphization is acceptable
- `#[cold]` / `#[inline(never)]` on rare error paths
- Iterate slices (`for x in s`, `chunks_exact`) instead of indexing — elides bounds checks, enables autovectorization
- Copy small hot data into locals to end aliasing doubts
- Explicit SIMD: `std::simd` (nightly), `wide`, `core::arch` (not the deprecated `packed_simd`)
- Careful `#[inline]`: helps tiny getters, hurts when code size explodes
- Limit monomorphization blowup: non-generic inner function for bulky logic

Build flags: `lto = "thin"` (or `"fat"` for final binaries), `codegen-units = 1`, `panic = "abort"` where acceptable; `-C target-cpu=native` only on controlled deploy targets; PGO via `cargo-pgo` (typically 5–15%); `debug = 1` in the profiling profile.

## Lens 6 — Code size

- Measure with `cargo llvm-lines` (monomorphization) and `cargo bloat` (binary)
- Avoid heavy `format!`/Display machinery in tiny inlined helpers
- `fn api<S: AsRef<str>>(s: S) { inner(s.as_ref()) }` keeps bulky logic non-generic
- Watch derives on huge types pulled into hot crates

## Lens 7 — Parallelism and synchronization

See `rust-concurrency-patterns`.

- `rayon` for CPU-bound, bounded `tokio` task batches for I/O-bound; measure — saturated bandwidth makes parallel slower
- **Cap worker counts.** `num_cpus::get()` unbounded on a shared host starves everything else; make the cap configurable
- One lock per batch, not per item; never hold a lock across `.await`, RPC or disk I/O
- Shard contended maps (`DashMap`); `crossbeam_utils::CachePadded` for false sharing; atomics for counters; `parking_lot` in sync code
- Bounded channels and batching instead of per-item task spawn
- Lock-free only via proven crates (`crossbeam`, `arc-swap`)
- Library types thread-compatible (external sync) by default

## Lens 8 — Serialization

Heavy schema frameworks are convenient and expensive (Abseil: 1000 points ~20× faster as `Vec<Struct>` than protobuf).

- Plain structs on internal hot paths; serialize at boundaries
- `bytes::Bytes` to avoid copying large fields; reuse parse buffers
- `simd-json` for proven hot JSON; `rkyv`/flatbuffers only with measured need
- Never re-encode/decode the same blob repeatedly

## Lens 9 — Container cheat sheet

| C++ / Abseil idea | Rust default | Faster / denser |
|---|---|---|
| `std::vector` | `Vec<T>` | `reserve`; `SmallVec`/`ArrayVec` if small |
| `absl::InlinedVector` | — | `smallvec`, `arrayvec`, `tinyvec` |
| `flat_hash_map` | `HashMap` (hashbrown) | `FxHashMap`/`foldhash` for internal keys; `entry` API |
| `std::map` | `BTreeMap` | sorted `Vec` + `binary_search`; `blart` for string keys |
| bit sets | `HashSet` of ids | `bitvec`, `fixedbitset`, masks |
| intrusive list | `LinkedList` | `Vec` + indices; `intrusive-collections` |
| arenas | — | `bumpalo`, `typed-arena`, `slab`, `slotmap` |
| `string_view` / `Span` | `&str`, `&[T]` | always prefer views in APIs |
| `FunctionRef` | `impl Fn` | avoid `Box<dyn Fn>` on hot calls |
| `alignas(64)` | — | `CachePadded` |
| stats counter | `Mutex<u64>` | `AtomicU64` (relaxed) |
| allocator | system malloc | `mimalloc`, `tikv-jemallocator` |

Strings: shared immutable -> `Arc<str>`; stored immutable -> `Box<str>`; short -> `compact_str`; conditional ownership -> `Cow`; search -> `memchr`/`aho-corasick`; I/O buffers -> `Bytes`; numbers -> `itoa`/`ryu`.

API design: **bulk APIs** (`lookup_many`) to amortize locking and boundary costs; **view types** in parameters; let callers pass **pre-allocated** buffers, clocks and scratch space; thread-compatible by default.

## Lens 10 — Embedded / local databases (SQLite and similar)

When a process repeatedly queries a large local database, the query plan is the hot path.

- [ ] **`=` not `LIKE`** for exact keys — `LIKE` without wildcards still changes the plan and can turn an index search into a scan
- [ ] Run **`EXPLAIN QUERY PLAN`** (or the engine's equivalent) on every new or changed query against **production-sized** data: want `SEARCH ... USING INDEX`, not `SCAN`
- [ ] **Composite indexes match the real predicates** (`(key, timestamp)`), not single-column indexes on low-selectivity columns that never get used
- [ ] **One transaction per batch** (`BEGIN IMMEDIATE ... COMMIT`) instead of thousands of autocommits/fsyncs
- [ ] **Prepare statements once**, reset and re-bind in the loop
- [ ] **WAL** (`PRAGMA journal_mode=WAL`) when readers run during a writer; set **`busy_timeout` on every connection** — the default 0 turns any contention into immediate "database is locked" errors; mark read-only connections `query_only`
- [ ] **Avoid `DISTINCT`/`GROUP BY`** when the filter already yields one row per key; dedup in Rust if needed
- [ ] **No N+1 point queries** — bulk `JOIN`/`IN (...)`, or query only the keys actually needed
- [ ] `ANALYZE` after schema/index changes; `PRAGMA integrity_check` after index surgery; drop unused indexes
- [ ] Back up before live schema changes; idempotent `CREATE INDEX IF NOT EXISTS`

## Lens 11 — Scheduled jobs and multi-stage pipelines

Run frequency multiplies waste: a job that runs every minute and does hundreds of network calls before discovering it has nothing to do is an incident, not a micro-optimization.

- [ ] **Order work cheapest -> dearest:** kill switch -> throttle/quota check -> load local state -> only then network, LLM or heavy I/O
- [ ] **Don't pay O(queue) external I/O** when this run only advances one item
- [ ] **Cap retries and abandon stuck items** so queues cannot grow forever against a hard external quota
- [ ] **Atomic publish** of files other processes read mid-write: write a temp sibling, then `rename`
- [ ] **Shared HTTP client with timeouts** — one hung upstream must not stall the whole chain
- [ ] **No CWD-relative paths** in jobs launched by schedulers; use absolute or config-derived paths
- [ ] In shell orchestration, know whether background jobs are `wait`ed or race the next phase
- [ ] Measure **wall clock on the full real workload** and peak RSS (`/usr/bin/time -v` or `-f '%e %M'`) per binary before and after

---

## Flat profile playbook

1. Take the many 1% wins (layout, reserve, hoist, cold paths) with benchmarks
2. Flame graph: find loops near the top of stacks; restructure the whole loop
3. Step up a level — structural/API changes beat instruction tweaks
4. Replace overly general code (regex -> prefix, serde -> manual, generic -> special)
5. Allocation profile — cut top allocation sites
6. Hardware counters — `perf stat -e cache-misses,branch-misses`
7. I-cache — binary growth and over-inlining

## Measurement toolkit

| Need | Tool |
|---|---|
| Microbench | `criterion` (see `rust-criterion-benchmarking`), `divan` |
| Noise-immune CI regression gate | `iai-callgrind` (instruction counts) |
| CPU profile | `samply`, `perf record` + `cargo flamegraph` |
| Assembly | `cargo-show-asm`, Godbolt |
| Allocations | `dhat`, `heaptrack` |
| Cache/branch | `perf stat` |
| Async contention | `tokio-console` |
| Binary size / bloat | `cargo bloat`, `cargo llvm-lines` |
| Type layouts | `-Zprint-type-sizes` |
| PGO | `cargo-pgo` |
| Query plans | `EXPLAIN QUERY PLAN` |
| Wall clock + peak RSS | `/usr/bin/time` |

Rules: benchmark with `--release` and production RUSTFLAGS; `black_box` inputs and outputs; prefer throughput for batch work; distrust single-run deltas; validate microbenchmark wins on realistic workloads. Run heavy builds/benchmarks on a development machine, never uncapped on a host that also serves production traffic (use `nice` and `--jobs N` if you must).

Know what **not** to optimize: paths already at millisecond scale, intentional sleeps/rate limits, jobs dominated by one or two network calls, string building when a query is 50× the wall clock.

---

## Review output format

```markdown
## Performance review: <scope>

### Summary
- Hot paths identified: ...
- Profile/bench evidence: ... (or "static review only")
- Top opportunities: ...

### Findings

#### [Critical|Important|Minor] <title>
- **Where:** `path/file.rs:LINE`
- **Evidence:** quote + why it is hot
- **Mechanism:** alloc / cache / algorithm / lock / avoided work / query plan / ...
- **Impact class:** structural | representation | allocation | ...
- **Fix:** minimal concrete change (code sketch OK)
- **Validate:** criterion case / perf command / query plan / reason the estimate suffices

### Non-findings / accepted costs
- ...

### Suggested bench plan
- ...
```

Push back on premature micro-opts that hurt clarity without hot-path evidence, and on "optimize everything" — prioritize the critical 3%.

## Quick hot-path checklist

1. Per item / per run / per request? If not, deprioritize.
2. Structural first: query plan, N+1, unbounded queue, full re-scan before a cheap gate?
3. Database: `=` not `LIKE`, matching composite index, one transaction per batch, prepared statements, `busy_timeout`, plan checked on production-sized data?
4. Any alloc/copy that could be a borrow, move or reuse?
5. Any map/regex/`format!` that could be simpler or denser?
6. Locks and pools: minimal scope, capped workers, nothing held over I/O?
7. Cheap gates before expensive I/O?
8. Logging per item? Atomic file publish?
9. Is the predicate that gates the work actually correct (inspect live state)?
10. Measured or estimated before recommending Important+ changes?

## Source attribution

Principles and structure adapted from Jeffrey Dean & Sanjay Ghemawat, *Performance Hints*, 2025, https://abseil.io/fast/hints.html. When a measured, generalizable win or footgun is confirmed, add the principle (not the project anecdote) to the relevant lens.

## Git / Commit

Every commit message ends with the authoring model attribution trailer the harness specifies, e.g. `Co-Authored-By: <model name> <noreply@...>`.
