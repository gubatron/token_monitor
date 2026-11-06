---
name: rust-backend-engineer
description: "Implementation and review guidance for Rust code of any kind — libraries, services, CLIs and desktop apps. Covers writing new modules, designing types and enums, error handling, refactoring, testing conventions (integration tests in tests/), the fmt/clippy/test quality gate, and reviewing Rust for correctness, robustness and idiomatic patterns. Use proactively whenever Rust code is being written or modified."
tags: [rust, engineering, code-review, enums, error-handling, testing, types, refactoring, serde, tokio, thiserror, clippy]
triggers: [rust, new module, endpoint, refactor, review rust, typed enum, model a domain, error type, thiserror, integration test, clippy, cargo fmt, make it testable, rust cli, rust library, rust service]
---

# Rust Backend Engineer

## Overview

Senior Rust engineering: think in types first, use the compiler as the primary correctness tool, and treat every function as a contract. Immutable data and strong types are the default; reserve `mut` and `String` for where they are earned.

Despite the name, this applies to any Rust code: libraries, network services, CLIs, background workers and desktop apps.

Companion skills: `rust-performance-reviewer` (speed), `rust-concurrency-patterns` (shared state, locks, async), `rust-criterion-benchmarking` (measurement), `rust-quality-pipeline` (hermetic tests, property tests, fuzzing, coverage, CI reporting).

## Core Philosophy

**Immutability is the default. Mutability is the last resort.**

- Prefer immutable data structures and functional patterns (`map`, `filter`, `fold`, iterators) over mutable state.
- When mutation is unavoidable, contain it to the smallest possible scope and document why.
- Use `let`, not `let mut`, unless you can justify the mutation.
- Prefer returning new values over modifying existing ones.

**Strongly-typed domains eliminate entire classes of bugs.**

- Never use raw `String` constants as function parameters, control values, or discriminators.
- Model every domain concept with enums that encode valid states at compile time.
- Make enums ergonomically convertible to/from strings where interoperability requires it (APIs, serialization, CLI input) using `FromStr`, `Display`, `serde`, and `strum` where appropriate.
- This eliminates stringly-typed logic, rejects invalid inputs at compile time, enables exhaustiveness checks and makes interfaces self-documenting.

**Simplicity and composability over cleverness.**

- Write short, focused functions that do one thing well (typically under 30 lines).
- Compose small functions into larger behaviors instead of writing monoliths.
- Prefer explicit over implicit: avoid trait or macro magic unless it genuinely reduces code or improves clarity.
- If a function needs a comment to explain what it does, break it into smaller, well-named parts.

## Documentation Standards

Every public struct, enum, trait and function MUST have documentation:

```rust
/// Represents the lifecycle state of an order in the fulfillment pipeline.
///
/// Each variant encodes a valid state transition target. Invalid transitions
/// are prevented at compile time by the type system.
///
/// # Examples
///
/// ```rust
/// use my_crate::OrderStatus;
///
/// let status = OrderStatus::Pending;
/// assert_eq!(status.to_string(), "pending");
///
/// let parsed: OrderStatus = "shipped".parse().unwrap();
/// assert_eq!(parsed, OrderStatus::Shipped);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    /// Order has been created but not yet confirmed.
    Pending,
    /// Order has been confirmed and is being prepared.
    Confirmed,
    /// Order has been handed to the carrier.
    Shipped,
    /// Order has been delivered.
    Delivered,
    /// Order was cancelled before shipment.
    Cancelled,
}
```

- Document the **why**, not just the **what**.
- Include `# Examples` sections with runnable doc-tests for any non-trivial function.
- Document error conditions, panics (if any — prefer `Result`), and edge cases.
- Use `# Errors`, `# Panics`, `# Safety` sections per Rust convention.

## Enum Design

1. **Always derive** `Debug, Clone, PartialEq, Eq` at minimum, plus `Serialize, Deserialize` when the type crosses a boundary.
2. Add `Copy` (and usually `Hash`) when variants carry no heap data.
3. Implement `Display` for human-readable output.
4. Implement `FromStr` for parsing from strings with a proper error type.
5. Use `#[serde(rename_all = "snake_case")]` for consistent serialization.
6. Consider `strum::EnumString` / `strum::Display` to reduce boilerplate.
7. Create a dedicated error type for parse failures — never use `String` as an error type.

```rust
impl std::fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::Confirmed => "confirmed",
            Self::Shipped => "shipped",
            Self::Delivered => "delivered",
            Self::Cancelled => "cancelled",
        };
        f.write_str(s)
    }
}

/// Returned when a string does not name a known [`OrderStatus`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown order status: {0:?}")]
pub struct ParseOrderStatusError(pub String);

impl std::str::FromStr for OrderStatus {
    type Err = ParseOrderStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "confirmed" => Ok(Self::Confirmed),
            "shipped" => Ok(Self::Shipped),
            "delivered" => Ok(Self::Delivered),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(ParseOrderStatusError(other.to_owned())),
        }
    }
}
```

## Error Handling

- Use `thiserror` for library/domain errors; `anyhow` only in binaries, top-level glue or test code.
- Define specific error enums per module — never `Box<dyn Error>` in library code.
- Prefer `Result<T, E>` over panics. Reserve `unwrap()`/`expect()` for provable infallibility, and always give `expect()` a message stating the invariant.
- Use `?` for propagation; avoid verbose match chains for simple propagation.
- Error values that get persisted, logged or displayed must never embed raw upstream response bodies: the parse-failure path is exactly when a body may still carry a live secret (tokens, session ids). Summarize untrusted bodies with char-boundary-safe truncation.

## Wire Formats and Protocol Evolution

- Evolve serialized types with new **optional** fields (`Option<T>` plus `#[serde(default)]`) and design the degradation explicitly: old readers ignore the field, new readers see `None` on old payloads.
- Write the mixed-version matrix down (old writer/new reader, new writer/old reader) and pin it with serde tests over old-shaped and new-shaped payloads.
- Platform and API size caps are part of the contract. Any dynamic content placed into a size-capped or parser-validated field (a button payload, a header, a filename) gets a test that pins the limit with production-shaped data, not toy fixtures.

## Testing Standards

**Convention: tests live in the crate's `tests/` directory, never in `src/`.**

- All tests are integration tests — one `tests/test_<topic>.rs` (or `tests/<topic>.rs`) file per behavior, with shared helpers in `tests/common/mod.rs`.
- **Never** add `#[cfg(test)] mod tests { ... }` blocks to source files. Source modules stay free of inline test modules and test-only imports.
- If a private helper needs testing from `tests/`, make it `pub` (not `pub(crate)`, which integration tests cannot reach) — preferably behind a clearly documented module — rather than adding a `#[cfg(test)]` escape hatch.
- Document tests with `///` comments explaining the behavior they verify.
- Use descriptive names: `test_parse_response_rejects_success_false`, not `test1`.
- Test happy paths, error cases, edge cases and boundary conditions.
- For enums: always test `Display`/`FromStr` round-tripping and invalid-input rejection.
- Tests that hit live third-party APIs are `#[ignore]` and run only with `--ignored` plus explicitly provided credentials. Everything else runs in the default `cargo test`.
- Keep the default suite hermetic: tempdirs, injected clocks, no network. In tests that could reach a network stage, pin provider credential env vars to junk values so a developer shell's real keys can never trigger live calls. See `rust-quality-pipeline`.
- Hermetic fixtures encode your assumptions; run one live smoke test against the real thing before trusting thresholds or heuristics derived from fixtures.

Each test file imports the crate like an external user would. Example `tests/test_order_status.rs`:

```rust
use my_crate::OrderStatus;

/// Verifies that every `OrderStatus` variant survives a
/// `Display` -> `FromStr` roundtrip without data loss.
#[test]
fn test_order_status_display_fromstr_roundtrip() {
    let variants = [
        OrderStatus::Pending,
        OrderStatus::Confirmed,
        OrderStatus::Shipped,
        OrderStatus::Delivered,
        OrderStatus::Cancelled,
    ];
    for status in variants {
        let s = status.to_string();
        let parsed: OrderStatus = s.parse().expect("roundtrip should succeed");
        assert_eq!(status, parsed);
    }
}

/// Ensures that invalid strings produce a meaningful parse error
/// rather than silently succeeding.
#[test]
fn test_order_status_rejects_invalid_input() {
    let result = "nonexistent".parse::<OrderStatus>();
    assert!(result.is_err());
}
```

### Tests and process-global state

- Env vars, global hooks, event listeners and other process-global slots make parallel tests race. Serialize the tests that touch them with a file-local lock — a `tokio::sync::Mutex` in async tests (a `std::sync::Mutex` guard held across `.await` trips `clippy::await_holding_lock`) — or run them as sequential scenarios inside one test under one set of RAII guards.
- Assert ordering with a recording listener (deterministic), never with wall-clock timing (flaky).

## Performance Optimization

Once code is correct and tested (details in `rust-performance-reviewer`):

1. Identify hot paths — what runs per request/item vs. once at startup.
2. Prefer zero-copy: `&str` over `String`, `Cow<'_, str>` when ownership is conditional.
3. Avoid unnecessary allocations: iterators instead of intermediate `Vec`s.
4. Share large immutable data with `Arc` instead of deep clones.
5. `SmallVec`/`ArrayVec` for small bounded collections on hot paths.
6. Batch I/O and database queries — avoid N+1 patterns.
7. Cache expensive computations with `std::sync::OnceLock` / `LazyLock`.
8. Cache heavyweight external resources (spawned processes, browser instances, connection pools) process-wide; spawning one per request is a performance bug. If a library binds objects to the async runtime that created them, key the cache per runtime.
9. Measure before optimizing: `criterion` for microbenchmarks, `tracing` spans for latency.

## Robustness Rules for I/O-heavy Code

1. **Timeouts must cover every wait** — including library internals and cleanup paths (close/shutdown RPCs, load-state waits with their own hardcoded defaults). Wrap the whole external operation in `tokio::time::timeout` (or a watchdog that kills the child process), not just the steps that accept a timeout parameter. A blocking client with no timeout story needs one imposed from outside.
2. **"Success" on empty or truncated input is data loss.** If a downstream trim, queue or buffer can silently violate an invariant (e.g. dropping the whole payload because it exceeded a budget), check it before the call and fail loudly.
3. **HTTP 200 is not "usable".** Check content type and body shape, not just status. A feed URL returning an HTML shell with 200 is a failure.
4. **Reachability is not 2xx.** Many clients surface non-2xx as `Err`, so `probe().is_ok()` answers "did it succeed", not "is it there". A probe that treats a 401/403 from an endpoint (e.g. a link-local metadata service that requires a token) as unreachable is wrong — any response at all means reachable. Re-audit every detection helper written this way.
5. **Detection helpers fail silently.** A broken environment probe can make a whole feature never engage in production while its logic and hermetic tests are correct. Verify detection once on the real target machine class; env overrides that force a path are not a substitute.
6. **Fallbacks must trigger on "succeeded but unusable"** (thin content, challenge pages, empty extraction), not only on hard errors, and every fallback layer re-validates its own output with the same checks as the primary path. A fallback that can never return something worse than the original is safe to fire and safe to reorder.
7. **Verify at the deepest level the pipeline depends on.** "The endpoint answers" is not "the data we need is extractable from it". Verify claims about external systems live, not from general knowledge.
8. **Simulate the exact external-API contract before shipping UX.** Transport success (a 200 from curl) does not verify how the platform renders or interprets the payload. Read the platform's interaction docs and test the exact payload shape.

## Design Rules

1. **One mechanism, no speculative fallbacks.** Ship exactly one way to do something. A second mechanism (dual config sources, fallback chains nobody asked for) is a second thing to test, document and debug.
2. **Follow the existing naming convention before inventing one.** Grep for existing config keys, env vars and module names first.
3. **Match the tool to the job.** Don't reuse an existing pattern just because it exists — evaluate fit. If the main output is unusable, fix it before presenting, not "in v2".
4. **One implementation of shared logic.** Expose a public API from the crate that owns the logic and depend on it, rather than copying it into another crate; two copies always drift.
5. **Public crates get capabilities, private crates get policies.** A reusable crate exposes generic knobs (an ordering preference, off by default); the application decides when to turn them on. Environment- or deployment-specific semantics never leak into the reusable crate.
6. **Never hardcode secrets** in source, scripts or start-up files. Read them from one source of truth at runtime; after a restart, run a cheap authenticated probe (distinguishing 400 from 401) before declaring health.
7. **A library fix ships nowhere until every binary that links it is rebuilt and redeployed.** Know which binaries depend on a shared crate, rebuild all of them, and verify the deployed artifact contains the change.

## Workflow

1. Understand the requirement — ask clarifying questions if the domain is ambiguous.
2. Design types first — structs, enums and traits before logic.
3. Implement with immutability and composability.
4. Document every public item, with examples.
5. Write tests in `tests/` covering happy paths, errors and edge cases. Never `#[cfg(test)]` in `src/`.
6. Review hot paths once correct.
7. Self-review: all public items documented? Tests in `tests/`? Mutation minimized? Strings replaced by enums? Errors typed? Timeouts on every wait?
8. **Quality gate (mandatory before claiming done or asking to commit):** all three must exit 0 with zero warnings:

   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace --all-features
   ```

   Do not hand off code that fails the gate. If a public item is only used by integration tests, a binary target may still fail `dead_code` under `-D warnings`; use it from a real code path or reconsider whether it should exist.

9. **Evidence before completion claims.** Report the commands you ran and their results. No silent skips; if a gate cannot run, say so explicitly.

## Review Focus

- Check type design before line-by-line logic.
- Check failure modes, timeouts, boundary cases and test coverage before micro-optimizations.
- Prefer a small refactor that improves type safety over a large stylistic rewrite.
- Confirm the quality gate is clean before approving.
- Verify the topology/environment before theorizing about a failure: what process is actually listening, in which container, with which config. A wrong mental model is the most expensive bug.
- After an edit that "shouldn't have" applied cleanly, re-read the file. Surprise is a signal.
- Narrate before touching production: state what will run, why it is safe, and get approval.

## Git / Commit

Every commit message ends with the authoring model attribution trailer the harness specifies, e.g. `Co-Authored-By: <model name> <noreply@...>`. Prefer granular commits, one logical change each.
