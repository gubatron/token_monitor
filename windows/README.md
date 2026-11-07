# TokenMon for Windows

Rust system-tray app for Windows 10 and 11 (x64). Behavior is specified in `docs/spec/`; the plan, crate ownership and agent protocol are in `docs/IMPLEMENTATION_PLAN.md`.

## Quick start on Windows

1. Install the prerequisites once (from PowerShell or cmd), then open a new terminal so `PATH` picks them up:
   ```powershell
   winget install Git.Git Rustlang.Rustup Python.Python.3.12 ezwinports.make
   winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
   ```
   Git for Windows provides the bash the Makefile runs its recipes in; the Build Tools provide the MSVC linker. `choco install make` works too. rustup installs the pinned toolchain on first use.
2. Build and run, or install:
   ```sh
   git clone https://github.com/gubatron/token_monitor.git
   cd token_monitor/windows
   make run        # release build, then start TokenMon (look for the tray icon)
   make install    # build the MSI and install it for this user, no admin
   ```

`make` (or `make help`) lists every target. Any shell works: on Windows the Makefile finds Git for Windows' `bash.exe` next to `git` (set `GIT_BASH` to its `bin\bash.exe` if it lives elsewhere) and runs every recipe there, so the recipes are the same on every OS.

| Target | What it does | macOS/Linux |
|---|---|---|
| `help` | List the targets (default) | yes |
| `version` | Print the workspace version | yes |
| `build`, `release` | Debug or release build of `TokenMon.exe` (`x86_64-pc-windows-msvc` on Windows) | yes (native stub) |
| `run`, `run-debug` | Build, stop a running TokenMon, launch the new one detached | no |
| `smoke` | Release build, then `TokenMon.exe --smoke` (headless, one refresh cycle) | no |
| `test` | `cargo test --workspace` | yes |
| `lint` | `cargo fmt --check`, clippy `-D warnings`, `cargo deny` if installed | yes |
| `fmt` | `cargo fmt --all` | yes |
| `fuzz-check` | `cargo check` of `fuzz/` on stable | yes |
| `parity` | Check `docs/PARITY.md` against the Swift tests | yes |
| `xcheck` | clippy for `x86_64-pc-windows-msvc` (through `cargo xwin` on macOS/Linux) | yes |
| `tools` | WiX 3.14.1 and cargo-wix 0.3.9 into `.tools/`, pinned and SHA-256 checked like the release workflow; does nothing once they are there | no |
| `installer` | Per-user MSI: `target/wix/tokenmon-windows-<version>-x64.msi` | no |
| `zip` | Portable zip: `target/wix/tokenmon-windows-<version>-x64.zip` | no |
| `dist` | MSI, zip and `SHA256SUMS` in `target/wix/` (unsigned) | no |
| `install` | `installer`, then `msiexec /i` for this user (`QUIET=1` for no UI) and print where it went | no |
| `uninstall` | `msiexec /x` every product with TokenMon's UpgradeCode; keeps your data in `%LOCALAPPDATA%\TokenMon` | no |
| `clean` | `cargo clean`, including `target/wix` | yes |
| `distclean` | `clean`, plus `.tools/` | yes |

The Makefile never runs `installer/smoke-test.ps1` (see below).

## Layout

| Crate | Role | Builds on macOS/Linux |
|---|---|---|
| `tokenmon-core` | Domain, networking, polling, storage and the shared contracts (`ProviderId`, `UsageError`, `Clock`, `ProviderUsagePoller`, `CredentialStore`, `AuthConfig`) | yes |
| `tokenmon-provider-*` (7) | One crate per provider | yes |
| `tokenmon-platform` | OS services behind traits (DPAPI, toast, power, shell, WebView2, installer) with in-memory fakes. The only crate allowed to use `unsafe` | stubs and fakes yes, Windows code via cross-check |
| `tokenmon-ui` | Slint components and view models | yes |
| `tokenmon-app` | The tray binary | yes (stub), full app via cross-check |

## Build and test on macOS or Linux

The toolchain is pinned in `rust-toolchain.toml` (rustup installs it automatically). Everything except the `cfg(windows)` code builds and tests natively:

```sh
cd windows
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

These three commands are the gate that must be green before every push.

## Cross-check the Windows code from macOS or Linux

The `cfg(windows)` code in `tokenmon-platform` and `tokenmon-app` only type-checks for the Windows target. `cargo-xwin` downloads the MSVC CRT and Windows SDK on first use (a few hundred MB, cached under `~/.cache/cargo-xwin`), so no Windows machine is needed to check it:

```sh
rustup target add x86_64-pc-windows-msvc   # also listed in rust-toolchain.toml
cargo install cargo-xwin --locked
cd windows
XWIN_ACCEPT_LICENSE=1 cargo xwin check --target x86_64-pc-windows-msvc -p tokenmon-platform -p tokenmon-app
```

`XWIN_ACCEPT_LICENSE=1` accepts the Microsoft SDK license non-interactively. `check` does not link or run anything; running tests needs the Windows CI runner.

## Coverage

CI measures line coverage on Ubuntu and Windows and fails when total line coverage drops more than 0.5 points below the latest `windows-port` run, or when a floor in `.config/coverage-floors.toml` is not met. `make coverage` gives the same numbers locally (needs `cargo-llvm-cov`; HTML in `target/llvm-cov/html`).

Test, bench, fuzz and example code is not measured, and neither is the Windows-only glue listed in **`.config/coverage-excludes.txt`**: thin `cfg(windows)` adapters that tests cannot drive (tray, panel windows, WebView2, toasts, message boxes, session end, power broadcasts, the registry). Real-Windows behaviour is covered by `TokenMon.exe --smoke` in CI and by manual testing. That file is the only list; CI and `make coverage` both build `--ignore-filename-regex` from it with `scripts/ci/coverage_excludes.py`. The rules:

* Excluded files stay thin adapters: OS calls in, OS calls out. New logic (parsing, decisions, state machines, layout, formatting) goes in a portable module with tests, and the adapter calls it. For example, the registry adapter uses `shell::registry::{encode_reg_sz, decode_reg_sz}` and the WebView2 window uses `webauth::layout`.
* Only files that are entirely `cfg(windows)` can be listed. A file that mixes portable and Windows code is split first, never excluded whole.
* Each file has a line budget: 400, or a frozen `max-lines=N` for the two that were larger when the policy started. They may shrink, not grow.
* Every entry needs a one-line reason.

The lint job runs `python3 scripts/ci/coverage_excludes.py check`, which fails when an entry has no reason or matches no file, or when a listed file is not entirely `cfg(windows)` or is over its budget. Keeping adapters thin is up to review. When the list changes, the coverage job recomputes the baseline from the previous run's per-file data with the new list, so the delta compares like with like.

## Other tools

* `cargo deny check` (config in `deny.toml`): licenses compatible with an MIT app, advisories, duplicate versions (warn). Slint is used under its royalty-free desktop license; see the note in `deny.toml`.
* Shared dependency versions and lints live in the workspace `Cargo.toml`. Crates use `dep = { workspace = true }`.
* Tests live in each crate's `tests/` directory, never in `src/`.

## Logs and diagnosing a crash

`TokenMon.exe` has no console, so it logs to `%LOCALAPPDATA%\TokenMon\logs\tokenmon.log`. The file rolls at 1 MiB and keeps five files (`tokenmon.log`, `tokenmon.log.1` ... `.4`), so the folder stays under about 5 MiB. The default level is `info`; set `RUST_LOG` (for example `RUST_LOG=tokenmon_app=debug`) to change it. Nothing secret is logged: cookies, API keys and tokens never go to `tracing`. Release builds use `panic = "abort"`; a panic hook writes the message, thread and source location to the log before the process ends, so "the icon vanished" reports should include the last lines of this file. Startup failures (no tray icon, no IPC pipe) also show a message box. `TokenMon.exe --smoke` logs to stdout instead.

## Updates: repository and signature policy

* **Release repository.** The update feed repository is chosen in one place, `tokenmon_core::update::feed::configured_repository`. It reads the build-time environment variable `TOKENMON_UPDATE_REPO` (`owner/repo`, validated as a plain GitHub slug; invalid values fall back) and defaults to `gubatron/token_monitor`. Example: `TOKENMON_UPDATE_REPO=owner/fork cargo build --release`.
* **Signature policy.** `tokenmon_platform::install::SignaturePolicy` has `AcceptUnsignedWithDigest` (default: SHA-256 from the same GitHub release plus PE/MSI structure and Mark-of-the-Web checks) and `RequireAuthenticode` (additionally `WinVerifyTrust`, fails closed off Windows). The app picks one with `verifier_for_policy(policy)`; switching is a one-line change.
* **Startup helpers for the app.** Call `tokenmon_platform::install::handle_startup_args(std::env::args(), &current_exe)` first thing in `main`: it honours `--wait-for-pid <pid>` (bounded 30 s wait for the old process) and then runs `swap::cleanup_leftovers`.

## Releases

Windows releases are tagged `windows-vX.Y.Z` and built by `.github/workflows/windows-release.yml`. Release files live in `installer/`.

| File (on the release) | What it is |
|---|---|
| `tokenmon-windows-X.Y.Z-x64.msi` | Per-user MSI, no admin rights. Installs `TokenMon.exe` to `%LOCALAPPDATA%\Programs\TokenMon` and adds a Start-menu shortcut that carries the AppUserModelID `TokenMon` (toasts) |
| `tokenmon-windows-X.Y.Z-x64.zip` | Portable: `TokenMon.exe`, `README.txt`, `LICENSE.txt` at the top level |
| `tokenmon-windows-X.Y.Z-SHA256SUMS.txt` | SHA-256 of both, in `sha256sum -c` format |

The in-app updater only looks at `windows-v*` tags and only picks assets named `tokenmon-windows-*.zip` or `tokenmon-windows-*.msi`; the macOS updater only picks `TokenMon-*` assets from `v*` tags. Keep these names, or the updaters will miss the release (or pick the other platform's). The updater skips drafts and prereleases, so nothing reaches users until the draft is published.

### Cut a release

1. Bump `version` under `[workspace.package]` in `windows/Cargo.toml` (every crate inherits it; the MSI and the tag check read it). Run `cargo check --workspace` so `Cargo.lock` follows, then commit to `windows-port`.
2. Wait for Windows CI on that commit to be green.
3. Tag the commit and push the tag:
   ```sh
   git tag -a windows-v1.2.3 -m "TokenMon for Windows 1.2.3"
   git push origin windows-v1.2.3
   ```
4. The workflow checks that the tag is `windows-v` plus the crate version, runs every Windows CI gate (`windows-ci.yml`) on the tagged commit, builds `x86_64-pc-windows-msvc` on `windows-latest`, builds the MSI and the zip, then installs, upgrades, downgrades and uninstalls the MSI on a clean runner. When everything passes, it creates a **draft** release with the three files. The run's summary lists each file's size and SHA-256.
5. Review the draft (notes, files) and publish it.

If the tag does not match, the run fails with the fix to apply. Re-running a failed run replaces files already attached to the draft; a first run refuses to.

### Dry runs

The same build and test, without a release, run:

* on every push to `windows-port` that changes `windows/installer/` or the workflow;
* on demand: `gh workflow run windows-release.yml --ref windows-port` (add `-f gates=false` to skip the CI gates). The files are kept as the run's artifacts for 14 days.

### Build the MSI by hand (on Windows)

`make installer` (or `make dist` for the MSI, the zip and `SHA256SUMS`) does all of this. By hand:

```sh
cd windows
cargo build --locked --release -p tokenmon-app --target x86_64-pc-windows-msvc
cargo install --locked cargo-wix --version 0.3.9      # needs WiX Toolset 3.14.1 (candle/light)
cargo wix -p tokenmon-app --no-build --nocapture --target x86_64-pc-windows-msvc \
  --include "$PWD/installer/main.wxs" --bin-path "<WiX 3.14 bin folder>" \
  --output target/wix/tokenmon-windows-$(python installer/package.py version)-x64.msi
python installer/package.py zip --exe target/x86_64-pc-windows-msvc/release/TokenMon.exe \
  --license ../LICENSE --readme installer/portable-README.txt --out target/wix/portable.zip
```

`installer/package.py` (version, tag check, asset names, zip, SHA256SUMS, summary) is tested on any OS with `python3 -I -m unittest discover -s windows/installer/tests`. Never run `installer/smoke-test.ps1` on your own machine: it installs and uninstalls TokenMon and deletes the app's registry entries.

### What the MSI does

* WiX Toolset 3.14.1 driven by cargo-wix 0.3.9 (cargo-wix only drives WiX 3; both are downloaded by version and checked against a SHA-256 in the workflow).
* Per user (`InstallScope="perUser"`): `%LOCALAPPDATA%\Programs\TokenMon`, an ARP entry under HKCU, a shortcut in the user's Start menu. No elevation.
* UpgradeCode `97EEF0DA-B00F-4527-93D7-EF335558A346`. Never change it: `MajorUpgrade` finds the installed copy by it. A newer MSI replaces an older one in place; an older one refuses to install over a newer one.
* A running TokenMon is closed before its files are replaced or removed.
* Uninstall removes what the app registers at run time: the `TokenMon` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` and `...\Explorer\StartupApproved\Run`, `HKCU\Software\Classes\DesktopBackground\Shell\TokenMon` and `HKCU\Software\Classes\AppUserModelId\TokenMon`. An upgrade keeps them.
* It never deletes the user's data in `%LOCALAPPDATA%\TokenMon`.

### Signing

Builds are unsigned until the maintainer adds signing secrets, and SmartScreen warns on first run. With the repository secrets `WINDOWS_SIGNING_PFX_BASE64` and `WINDOWS_SIGNING_PFX_PASSWORD` set, tag builds and manual runs sign `TokenMon.exe` (before it is packaged) and the MSI with `signtool` (SHA-256, RFC 3161 timestamp) through `installer/sign.sh`. Without them those steps are skipped and the build never fails. The workflow's TODO(signing) note describes the switch to Azure Trusted Signing.
