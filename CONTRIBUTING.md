# Contributing to TokenMon

Thanks for helping improve TokenMon. This project is a native macOS menu bar app; small, focused changes are easiest to review.

## Development setup

1. Install [Xcode 16+](https://developer.apple.com/xcode/) on macOS 14+.
2. Clone the repo and open `TokenMon.xcodeproj`.
3. Select the **TokenMon** scheme → **My Mac** → Run.

```bash
git clone https://github.com/faulknerpearce/token_monitor.git
cd token_monitor
open TokenMon.xcodeproj
```

After adding or removing source files, regenerate the Xcode project:

```bash
xcodegen generate
```

To regenerate the TokenMon app icon:

```bash
swift Scripts/generate_icon.swift TokenMon/Resources/Assets.xcassets/AppIcon.appiconset
```

## Before you open a PR

- Keep changes focused on one concern.
- Prefer clear commit messages that explain *why*.
- Run the checks CI runs: `make lint`, `make format`, `make secrets`, and the
  full test suite. The test suite builds the app host and covers Cursor usage,
  auth sessions, pollers, and UI behavior:

```bash
make test
```

- `make test-core` is an optional, fast CLT-only smoke. It compiles the Grok
  parsers, CSV/JSON export, and the Daily Budget preview/Thursday-week assert, and does **not**
  cover Cursor daily quota, auth sessions, pollers, or UI. A green core run is
  not sufficient for billing or auth changes:

```bash
make test-core
```

## Windows parity (zero drift)

TokenMon also has a Windows port, a Rust app under `windows/`. The two apps must
not drift apart: every behavior change on macOS needs the same change on
Windows, or an issue that tracks it. CI enforces this on every push and pull
request, `master` included (`.github/workflows/windows-parity.yml`):

- **Drift guard.** A change that touches the macOS app (`TokenMon/`,
  `TokenMonTests/`, `Tests/`, `Scripts/`, `TokenMon.xcodeproj/`, `project.yml`
  or `Makefile`) must carry its Windows parity:
  - When you change the body of a Swift test whose `PARITY.md` row is
    `ported` (a new expected value, say), update that row or the Rust test it
    names in the same change, after checking the Rust test still matches.
    Comments and formatting don't count as a change.
  - Any other macOS change (app code, resources, build files, test helpers)
    needs a change under `windows/crates/` or to `windows/docs/PARITY.md`. A
    `windows/` README or docs edit is not enough. When Windows needs nothing
    (a macOS-only tweak), add a `Windows-Parity: <reason>` line to the pull
    request description or commit message instead.
  - Either rule is also met by referencing an **open** issue labeled
    `windows-parity` (for example `Refs #123` in the pull request description,
    or in the commit message for a direct push).

  Otherwise the check fails, and its job summary lists the exact tests and
  paths.
- **Parity ledger.** `windows/docs/PARITY.md` has one row per Swift test (every
  XCTest method and every `make test-core` check) giving its status (`todo`,
  `ported` or `n/a` with a reason) and the Rust test that covers it. Adding,
  renaming or deleting a Swift test without updating the ledger fails CI.

To satisfy it, do one of these:

1. **Port it.** Make the same change under `windows/`, with Rust tests, and
   update the matching `PARITY.md` rows in the same commit. See
   `windows/docs/IMPLEMENTATION_PLAN.md`.
2. **Track it.** If you are not porting it yourself, open an issue labeled
   `windows-parity` that says what the Windows port must do, and reference it
   from the pull request. For a new or renamed Swift test, also add its `todo`
   row: `python3 windows/scripts/parity/check_parity.py --print-missing-rows`
   prints the rows to paste.

Changes merged from upstream (faulknerpearce/token_monitor) into `master`
cannot carry a reference, so the guard fails on that push and says so. Open a
`windows-parity` issue that names the merge commit SHA and the upstream pull
request, then re-run the failed job; an open `windows-parity` issue that
mentions the commit satisfies the guard.

Check locally before pushing:

```bash
python3 windows/scripts/parity/check_parity.py
python3 -m unittest discover -s windows/scripts/parity/tests
```

## Pull requests

1. Branch from `master`.
2. Describe the problem and how you verified the fix.
3. Note any user-facing behavior changes (menu bar, sign-in, privacy).
4. Do not commit secrets, signing certificates, provisioning profiles, or personal session cookies.

## Scope notes

- TokenMon is an **unofficial** client. It uses authenticated provider web surfaces (grok.com, cursor.com, opencode.ai, claude.ai, chatgpt.com) that can change without notice.
- Avoid scraping that violates a provider's terms; prefer the existing auth + endpoint approach documented in `Docs/AUTH_AND_ENDPOINTS.md`.
- Do not add telemetry or third-party analytics without discussion.

## Code of conduct

By participating, you agree to uphold our [Code of Conduct](CODE_OF_CONDUCT.md).
