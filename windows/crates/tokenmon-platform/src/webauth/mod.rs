//! WebView2 sign-in window, popup stack, completion detection and cookie capture.
//!
//! Owner: ws-webauth (#24). Specs: 01 section 1.6.4-1.6.5 and 04 section 4.
//!
//! # Layout
//!
//! * Portable, tested on every OS: [`machine`] (events in, actions and timers
//!   out), [`gate`] (auth-host then return-page detection, navigation policy),
//!   [`nav`] (Back policy, address label), [`cookies`] (allowlist selection,
//!   chunked tokens, e-mail), [`profile`] (non-persistent per-provider
//!   user-data folders), [`config`], and [`host`] (traits plus a headless
//!   driver and fake host).
//! * `webview2` (`cfg(windows)`): a thin adapter. It owns a native Win32 window
//!   with the header, navigation bar and footer as native controls, hosts
//!   WebView2 in the body, and executes the machine's actions. It contains no
//!   decisions.
//!
//! # Security rules enforced here
//!
//! * Provider pages never reach app internals: no host objects, no web
//!   messaging, no DevTools, no default context menu or accelerator keys.
//! * Navigation and popups are limited to `http(s)` (and `about:blank`).
//! * Cookies live in a per-provider, per-session user-data folder that is
//!   deleted after capture, on cancel, and purged at startup.
//! * Captured cookie values never appear in `Debug` output or logs.

pub mod config;
pub mod cookies;
pub mod gate;
pub mod host;
pub mod machine;
pub mod nav;
pub mod profile;

#[cfg(windows)]
pub mod webview2;

pub use config::SignInConfig;
pub use cookies::{CapturePolicy, CapturedCookies, WebCookie};
pub use host::{FakeWebAuthHost, HeadlessSignIn, SignInDelegate, WebAuthError, WebAuthHost};
pub use machine::{Action, Input, Phase, SignInMachine, TimerId, ViewKind, ViewModel};
#[cfg(windows)]
pub use webview2::{WindowsWebAuthHost, webview2_available};
