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
