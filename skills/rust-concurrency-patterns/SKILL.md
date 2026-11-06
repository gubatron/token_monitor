---
name: rust-concurrency-patterns
description: "Use when designing or reviewing concurrent Rust code involving shared mutable state, lock contention, async tasks and throughput: RwLock vs DashMap, per-entity locking, keeping locks out of await points, write batching, write-ahead logs, and the durability/throughput tradeoffs between them."
tags: [rust, concurrency, async, tokio, dashmap, rwlock, mutex, performance, throughput, wal]
triggers: [concurrent access, lock contention, RwLock, DashMap, write throughput, async shared state, mutex, high concurrency, await_holding_lock, deadlock, write-ahead log, batching writes]
---

# Rust Concurrency Patterns for High-Throughput Code

## The Core Principle

Lock granularity sets the throughput ceiling. Every shared lock is a serialization point: identify which locks are on the hot path and eliminate or shrink them.

## Pattern 1: DashMap instead of RwLock<HashMap>

**Problem:** `Arc<RwLock<HashMap<K, V>>>` serializes all callers on the outer map; readers wait behind any writer.

**Solution:** `DashMap<K, V>` shards the map; operations on different keys proceed in parallel with no outer lock.

```toml
[dependencies]
dashmap = "6"
```

```rust
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// Before: all callers serialize on the outer lock.
// Arc<RwLock<HashMap<String, Arc<RwLock<Entity>>>>>

// After: callers for different keys run in parallel.
struct Service {
    entities: Arc<DashMap<String, Arc<RwLock<Entity>>>>,
}

impl Service {
    fn get_or_open(&self, key: &str) -> Result<Arc<RwLock<Entity>>, EntityError> {
        // Fast path: key exists; the shard read lock is held only briefly.
        if let Some(existing) = self.entities.get(key) {
            return Ok(existing.clone());
        }
        // Slow path: atomic at shard level; no outer lock needed.
        let entry = self
            .entities
            .entry(key.to_owned())
            .or_try_insert_with(|| Entity::open(key).map(|e| Arc::new(RwLock::new(e))))?;
        Ok(entry.clone())
    }
}
```

Note that `get_or_open` is synchronous: never hold a DashMap reference across `.await` (see Common Mistakes). Keep the open closure cheap — it runs while the shard write lock is held.

**When to use:** maps whose values are created lazily and looked up concurrently by many callers with different keys.

## Pattern 2: Per-entity locking, not global

**Problem:** one global `RwLock<State>` serializes every operation on every entity.

**Solution:** one lock per entity, found through a concurrent map. Reads on A never block writes on B.

```rust
// DON'T: single global lock
struct Service { state: Arc<RwLock<AllEntities>> }

// DO: per-entity lock, looked up via DashMap
struct Service { entities: Arc<DashMap<EntityId, Arc<RwLock<Entity>>>> }
```

Write throughput then scales with the number of distinct entities written concurrently. If an operation must lock two entities, always acquire them in a global order (e.g. by id) to avoid deadlock.

## Pattern 3: Keep locks out of await points

Most services read far more than they write:

- In async code, use `tokio::sync::RwLock`/`Mutex` only when a guard truly must live across `.await`. Otherwise a `std::sync::Mutex` or `parking_lot` lock held for a short, non-awaiting critical section is fine and faster.
- Never hold a `std::sync` guard across `.await` — it can block the runtime worker and deadlock, and `clippy::await_holding_lock` rejects it under `-D warnings`.
- Hold read locks minimally: copy out what you need, drop the guard, then work.

```rust
// Bad: write lock held during I/O
let mut guard = entity.write().await;
let result = some_async_call().await;
guard.update(result);

// Good: minimal lock scope
let data = {
    let guard = entity.read().await;
    guard.snapshot() // cheap clone
};
let result = process(data).await; // no lock held
entity.write().await.apply(result);
```

When splitting read and write like this, the entity may have changed in between: either make `apply` validate a version/generation, or accept last-writer-wins explicitly.

## Pattern 4: Write buffering

**Problem:** syncing to disk on every operation caps throughput near 1 / sync latency (2 ms per write => ~500 ops/s).

**Solution:** accumulate writes in memory and flush every N operations, on a timer, and on drop. This trades per-operation durability for throughput.

```rust
use std::fs::File;
use std::io::{self, BufWriter, Write};

const FLUSH_THRESHOLD: usize = 16;

struct BufferedWriter {
    file: BufWriter<File>, // keeps the file open: no reopen per write
    pending: Vec<u8>,
    pending_ops: usize,
    auto_flush: bool, // true = flush every write (durable); false = batch
}

impl BufferedWriter {
    fn append(&mut self, data: &[u8]) -> io::Result<()> {
        if self.auto_flush {
            self.file.write_all(data)?;
            return self.file.flush();
        }
        self.pending.extend_from_slice(data);
        self.pending_ops += 1;
        if self.pending_ops >= FLUSH_THRESHOLD {
            self.flush_pending()?;
        }
        Ok(())
    }

    fn flush_pending(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let buf = std::mem::take(&mut self.pending);
        self.file.write_all(&buf)?;
        self.file.flush()?;
        self.pending_ops = 0;
        Ok(())
    }
}

impl Drop for BufferedWriter {
    fn drop(&mut self) {
        // Best effort: Drop cannot return errors. Offer an explicit
        // `close(self) -> io::Result<()>` for callers that must know.
        let _ = self.flush_pending();
    }
}
```

`flush()` hands data to the OS; it survives a process crash but not power loss. Call `File::sync_data` when the guarantee must cover the machine going down.

**Tradeoff:** with batching, up to `FLUSH_THRESHOLD - 1` operations can be lost on a hard crash. Make the mode a runtime option so operators choose, and document the default.

## Pattern 5: Write-ahead log (WAL)

When per-write durability is required and throughput must exceed what per-write syncs allow:

1. Writes append to an in-memory buffer plus an append-only log file (µs latency, group-committed).
2. A background task folds the log into the main snapshot every N ms or N entries.
3. Readers see snapshot + unflushed log entries.
4. On open after a crash, replay the log; tolerate (and truncate) a torn final record.

```text
write() --> buffer + log append   (~µs)
                   |
         background compaction task
                   |
              main snapshot        (durable, possibly a few ms stale)
```

Use when brief staleness of the snapshot is acceptable but losing acknowledged writes is not (event logs, append-only chains, journals).

## Throughput reference (order of magnitude, per entity, commodity SSD)

| Pattern | Writes/s | Durability |
|---|---|---|
| Sync per write, reopen file each time | hundreds | Full |
| Sync per write, file kept open | hundreds to ~1k | Full |
| Batch flush every N ops | thousands | Last N-1 ops at risk |
| WAL + background compaction | tens of thousands | Per log sync policy |
| Pure in memory | 1M+ | None |

Measure on your hardware before quoting numbers; fsync latency varies by orders of magnitude between devices and filesystems.

## Common Mistakes

- Holding a `std::sync::Mutex` guard across `.await`: blocks a runtime worker and can deadlock. Restructure, or use `tokio::sync::Mutex` if the guard must cross the await.
- Holding a `DashMap` `Ref`/`RefMut`/entry across `.await`, or calling back into the same map while holding one: refs hold a shard lock; the same-shard re-entry deadlocks. Clone the value out first.
- Acquiring two locks in inconsistent order across code paths: classic deadlock. Define and document a lock order.
- Spawning one task per item for huge inputs: use bounded concurrency (`buffer_unordered(n)`, a `Semaphore`, or a bounded channel).
- Unbounded worker pools sized from `num_cpus` on shared hosts: cap them and make the cap configurable.
- Forgetting a flush on drop/shutdown for buffered structs: silent data loss on early return or panic.
- Tests that mutate process-global state (env vars, global hooks) running in parallel: serialize them with an await-safe lock and assert order with a recorder, not timing.

## Git / Commit

Every commit message ends with the authoring model attribution trailer the harness specifies, e.g. `Co-Authored-By: <model name> <noreply@...>`.
