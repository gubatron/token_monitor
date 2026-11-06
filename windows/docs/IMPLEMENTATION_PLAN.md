# TokenMon for Windows: implementation plan

The goal is a Rust system-tray app for Windows 10 and 11 (x64) with full feature parity with the macOS app and zero drift afterwards. The specs in `docs/spec/` are the source of truth for behavior. `docs/PARITY.md` maps every Swift test to the Rust test that covers it.

## Decisions (maintainer, 2026-11-06)

| Topic | Decision |
|---|---|
| Language | Rust, stable toolchain pinned in `windows/rust-toolchain.toml` |
| UI | Slint with the Skia renderer. Fluent style for Settings; the panel uses custom components that copy the Mac look |
| Sign-in | WebView2 (`webview2-com`), with an isolated user-data folder per provider and the cookie allowlist read through `ICoreWebView2CookieManager` |
| Secrets | DPAPI-encrypted (user scope) vault file `%LOCALAPPDATA%\TokenMon\vault.bin`. One mechanism, no Credential Manager (its 2,560-byte limit is too small) |
| Data dir | `%LOCALAPPDATA%\TokenMon\`, with the same file names and JSON formats as macOS where the spec gives them |
| Platforms | Windows 10 and 11, x64 only |
| Distribution | MSI (cargo-wix) plus a portable zip on GitHub Releases, tagged `windows-vX.Y.Z`. Asset names start with `tokenmon-windows-` so the Mac updater never picks them. Signing comes later (keys as repo secrets) |
| Right-click | A tray menu, plus an opt-in "Show TokenMon" entry on the desktop background (registry verb under HKCU) |
| Repo | `gubatron/token_monitor`, branch `windows-port`. Issues are on the fork, labeled `windows-port` |
| Drift | Parity ledger plus CI guard (see below). Any macOS change without Windows parity needs a `windows-parity` issue |
| Benchmarks | Not needed |

## Workspace layout and write ownership

Each crate or folder has exactly one owning workstream at a time. Touching another workstream's files needs a coordination note in MentisDB (`ws:` tag) and a comment on that workstream's issue first.

```
windows/
  Cargo.toml                 # workspace (owner: ws-foundation; others add their member line only)
  rust-toolchain.toml, deny.toml, .config/nextest.toml
  crates/
    tokenmon-core/           # platform-agnostic, no OS APIs, no UI
      src/net/               # HTTP client wrapper, UsageError/ProviderError, status mapping, Retry-After, bot challenge, Domain::matches   (ws-core-net)
      src/polling/           # BackoffTimer, PollingLoop, PollOutcome, ProviderRefresh, Clock                                             (ws-core-polling)
      src/storage/           # AppPaths, atomic 0600-equivalent writes, FileBackedStringStore, Hourly/Daily delta stores, QuotaWindow     (ws-core-storage)
      src/usage/             # Percent, DayKey, UsagePool, DailyTokenUsage, DailyBudget, pace, WeekOffset                                 (ws-core-usage)
      src/util/              # Format, JSON helpers, ISO-8601, Apple-epoch dates, logging                                                (ws-core-usage)
      src/settings/          # AppSettings model, clamps, needs_polling, JSON persistence                                                 (ws-settings)
      src/alerts/            # ThresholdNotifier logic (no OS toast)                                                                       (ws-settings)
      src/overview/          # Overview series and 30-day hourly history                                                                   (ws-overview)
      src/update/            # AppVersion, release feed, asset selection, URL trust rules (no install)                                      (ws-update)
      src/provider.rs        # Provider trait, ProviderId, snapshot types, registry order, accent colors                                   (ws-foundation)
      src/auth.rs            # CredentialStore and AuthSession traits, auth-failure threshold, session generation                          (ws-foundation)
    tokenmon-provider-grok/        (ws-grok)       # gRPC-web parser, REST probes, history store, daily builder, CSV/JSON export
    tokenmon-provider-cursor/      (ws-cursor)     # summary, events paging and cache, auth config
    tokenmon-provider-grokbot/     (ws-grokbot)    # depends on the cursor crate's auth config
    tokenmon-provider-claude/      (ws-claude)     # claude.ai usage, Claude Code JSONL scanner and ledger, API pricing
    tokenmon-provider-chatgpt/     (ws-chatgpt)    # session exchange, token cache, JWT account id
    tokenmon-provider-opencode/    (ws-opencode)   # console client, opencode.db (rusqlite bundled, read-only), price tables
    tokenmon-provider-openrouter/  (ws-openrouter) # API key, /key /credits /activity
    tokenmon-platform/       # OS services behind traits; cfg(windows) impls plus in-memory fakes for tests
      src/secrets/  (ws-secrets)    DPAPI vault
      src/notify/   (ws-notify)     WinRT toast (AppUserModelID)
      src/power/    (ws-power)      WM_POWERBROADCAST sleep/wake, Network List Manager; SystemWakeGate semantics
      src/shell/    (ws-shell)      autostart Run key, Explorer context-menu verb, single instance plus "show" IPC
      src/webauth/  (ws-webauth)    WebView2 sign-in window, popup stack, completion detection, cookie capture
      src/install/  (ws-update)     download, verify, staged replace, relaunch
    tokenmon-ui/             # Slint components and view models (view models are plain Rust and unit-tested)
      ui/components/ (ws-ui-kit)      PanelCard, MetricStat, SlimUsageTrack, SegmentedUsageBar, CategoryRow, DailyBudgetBars, HoverLabel, charts, typography, palette
      ui/panels/     (ws-ui-panels)   one panel per provider plus Overview, switcher
      ui/settings/   (ws-ui-settings) four-tab Settings window
      src/tray_icon/ (ws-tray)        tray bitmap renderer (tiny-skia), tooltip text, menu model
    tokenmon-app/            (ws-app)  # binary: wiring, registry, polling orchestration, tray (tray-icon crate), event loop bridge
  fuzz/                      # cargo-fuzz; each parser's target is owned by the workstream that owns the parser
  docs/  scripts/  installer/ (ws-release)
.github/workflows/windows-*.yml  (ws-ci; ws-parity owns windows-parity.yml)
```

## Engineering rules (from `skills/`)

- Follow `skills/rust-backend-engineer` and `skills/rust-quality-pipeline`.
- Tests go in each crate's `tests/`, never in `#[cfg(test)]` blocks in `src/`. They are hermetic: injected `Clock`, temp dirs, local mock HTTP (wiremock), no real network, no real home directory.
- Every Swift test in the spec checklists gets a Rust test, and its row in `docs/PARITY.md` is updated in the same commit.
- Every parser of untrusted input gets a fuzz target in `fuzz/`, with a seed corpus taken from the spec fixtures.
- Where the specs flag a Swift bug as "fix rather than copy", fix it and record it in the Divergences section of `PARITY.md`.
- Gate before every push, run from `windows/`:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`

  Platform crates must also pass `cargo check --target x86_64-pc-windows-msvc` (cargo-xwin).
- Commits are conventional (`feat(windows/core): …`), granular, with a body that explains why, end with `Refs #<issue>`, and carry the Co-Authored-By trailer.

## Coordination protocol (every agent)

1. **Start:** bootstrap MentisDB chain `token_monitor` as agent `claude-code-token_monitor`. Read standing context and search `ws:<your-ws>`. Write a `Subgoal` claiming your workstream and issue.
2. **Work:** in your own worktree off `origin/windows-port`. Commit small. Before each push, `git pull --rebase origin windows-port`, rerun the gate, then push.
3. **Progress:** comment on your issue after each meaningful push (commit SHA and what changed). Record decisions and lessons as MentisDB `Decision` and `LessonLearned`.
4. **Blocked on the maintainer:** comment on the issue under a heading `## Questions for maintainer`, with each question self-contained (context, options, your recommendation). Add the label `needs-maintainer`, then continue with your recommended default where that is safe.
5. **Done:** gate green, PARITY rows updated, issue closed with a summary comment, and a MentisDB `TaskComplete` written.

## Waves

- **Wave 1 (foundation, 3 agents in parallel):**
  - ws-foundation: workspace, core contracts and stub modules.
  - ws-ci: CI with summaries for tests, coverage and fuzzing.
  - ws-parity: parity ledger, guard and CONTRIBUTING rule.
- **Wave 2 (wide, one agent per workstream):** every remaining workstream above, all in parallel.
- **Wave 3:** ws-app integration, ws-release (MSI, zip, release workflow), end-to-end smoke on the Windows CI runner, then independent security and correctness reviews.
- The maintainer does a manual smoke test on real Windows at the end of waves 2 and 3.
