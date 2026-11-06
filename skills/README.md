# Skills

Agent skills used in this repository. Each lives in `skills/<skill-id>/SKILL.md` with YAML frontmatter (`name`, `description`, `tags`, `triggers`) followed by the instructions.

| Skill | Scope | Source |
|---|---|---|
| [`swift-engineer`](swift-engineer/SKILL.md) | Native macOS Swift / SwiftUI / AppKit implementation, review and performance for this app | Project-local. Not synced to any registry. |
| [`rust-backend-engineer`](rust-backend-engineer/SKILL.md) | Rust implementation and review for libraries, services, CLIs and desktop apps: types, enums, errors, tests in `tests/`, fmt/clippy/test gate, robustness rules | Mirrored from the MentisDB skill registry (generic copy) |
| [`rust-performance-reviewer`](rust-performance-reviewer/SKILL.md) | Rust performance review (Abseil Performance Hints): allocations, layout, containers, query plans, batch jobs | Mirrored from the MentisDB skill registry (generic copy) |
| [`rust-concurrency-patterns`](rust-concurrency-patterns/SKILL.md) | Shared state, DashMap vs RwLock, per-entity locks, locks and `.await`, write batching, WAL | Mirrored from the MentisDB skill registry (generic copy) |
| [`rust-criterion-benchmarking`](rust-criterion-benchmarking/SKILL.md) | Writing, running and interpreting Criterion benchmarks | Mirrored from the MentisDB skill registry (generic copy) |
| [`rust-quality-pipeline`](rust-quality-pipeline/SKILL.md) | Hermetic tests, proptest, cargo-fuzz, cargo-llvm-cov, cargo-deny/audit, GitHub Actions summaries | New in this repo; intended for upload to the MentisDB registry under this skill id |

The `rust-*` skills are shared by many unrelated Rust projects, so they must stay project-agnostic: no project names, hosts, deployment specifics or codebase-specific numbers. Project-specific lessons are written as the generic principle they teach.

## Registry versions copied

| skill_id | MentisDB registry version_id copied from |
|---|---|
| `rust-backend-engineer` | `d771029e-f11a-41a5-8a84-7c1fb83f5441` |
| `rust-performance-reviewer` | `2d3a6ef3-8cb3-4419-8a6c-0580d0c234d8` |
| `rust-concurrency-patterns` | `c5a9bd44-a307-4051-ad65-e96299c2e378` |
| `rust-criterion-benchmarking` | `1405905a-5dcb-40b4-a935-1d36eff2b841` |
| `rust-quality-pipeline` | none yet (authored here) |

The copies here were generalized and their formatting repaired (duplicated title blocks removed, mangled `#[derive]`/`#[serde]`/`#[test]` attributes and code comments restored), so they differ from the registry versions listed above. Update this table whenever a copy is pushed to or pulled from the registry.

## Keeping skills in sync

The MentisDB skill registry is canonical for the `rust-*` skills. The copies in this folder are mirrors.

**When starting work** that uses a `rust-*` skill:

1. Call `mentisdb_skill_versions` with the skill id and compare the latest `version_id` with the table above.
2. If the registry has a newer version, read it (`mentisdb_read_skill`), merge it into the copy here (keeping it project-agnostic), and update the table.

**When changing a `rust-*` skill here:**

1. Keep the change generic: no project names, hosts or one-codebase numbers.
2. In the same work session, upload the new content with `mentisdb_upload_skill` using the **same `skill_id`** (never a renamed one), so the registry and every other project see it.
3. Record the new `version_id` returned by the upload in the table above, in the same commit as the skill change.

A change that exists only here, or only in the registry, is a sync bug: fix it before ending the session.

`swift-engineer` is project-local and is edited only here.
