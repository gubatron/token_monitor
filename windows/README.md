# TokenMon for Windows

Rust system-tray app for Windows 10 and 11 (x64). Behavior is specified in `docs/spec/`; the plan, crate ownership and agent protocol are in `docs/IMPLEMENTATION_PLAN.md`.

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

## Other tools

* `cargo deny check` (config in `deny.toml`): licenses compatible with an MIT app, advisories, duplicate versions (warn). Slint is used under its royalty-free desktop license; see the note in `deny.toml`.
* Shared dependency versions and lints live in the workspace `Cargo.toml`. Crates use `dep = { workspace = true }`.
* Tests live in each crate's `tests/` directory, never in `src/`.

## Updates: repository and signature policy

* **Release repository.** The update feed repository is chosen in one place, `tokenmon_core::update::feed::configured_repository`. It reads the build-time environment variable `TOKENMON_UPDATE_REPO` (`owner/repo`, validated as a plain GitHub slug; invalid values fall back) and defaults to `gubatron/token_monitor`. Example: `TOKENMON_UPDATE_REPO=owner/fork cargo build --release`.
* **Signature policy.** `tokenmon_platform::install::SignaturePolicy` has `AcceptUnsignedWithDigest` (default: SHA-256 from the same GitHub release plus PE/MSI structure and Mark-of-the-Web checks) and `RequireAuthenticode` (additionally `WinVerifyTrust`, fails closed off Windows). The app picks one with `verifier_for_policy(policy)`; switching is a one-line change.
* **Startup helpers for the app.** Call `tokenmon_platform::install::handle_startup_args(std::env::args(), &current_exe)` first thing in `main`: it honours `--wait-for-pid <pid>` (bounded 30 s wait for the old process) and then runs `swap::cleanup_leftovers`.
