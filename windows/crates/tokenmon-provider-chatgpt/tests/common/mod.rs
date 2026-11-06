//! Shared fixtures: JWT builder, scripted transport, fake rolling-cookie session.
#![allow(
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::option_option
)]
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokenmon_core::{
    AuthEventListener, AuthSession, AuthSessionState, CapturedSession, CredentialStoreError,
    ProviderAuthSession, ProviderError, SessionGeneration,
};
use tokenmon_provider_chatgpt::{ChatGptTransport, RawReply, chatgpt_auth_config};

/// Unpadded base64url encoder for building JWT payloads.
pub fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |a, b| (a << 8) | u32::from(*b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// `h.<b64url payload>.s`
pub fn jwt(payload: &str) -> String {
    format!("h.{}.s", b64url(payload.as_bytes()))
}

pub fn jwt_exp(exp: i64) -> String {
    jwt(&format!(r#"{{"exp":{exp}}}"#))
}

pub const USAGE_BODY: &str = r#"{"plan_type":"plus","rate_limit":{"allowed":true,"limit_reached":false,
 "primary_window":{"used_percent":12,"limit_window_seconds":18000,"reset_after_seconds":9000,"reset_at":1787300000},
 "secondary_window":{"used_percent":57.5,"limit_window_seconds":604800,"reset_after_seconds":343143,"reset_at":1787800000}}}"#;

type UsageScript = dyn Fn(usize) -> Result<RawReply, ProviderError> + Send + Sync;

/// Transport that counts calls and answers from a script.
pub struct FakeTransport {
    pub token: String,
    pub session_calls: AtomicUsize,
    pub usage_calls: AtomicUsize,
    pub last_account: Mutex<Option<Option<String>>>,
    pub usage: Box<UsageScript>,
}

impl FakeTransport {
    pub fn new(token: String) -> Self {
        Self::scripted(token, |_| {
            Ok(RawReply {
                body: USAGE_BODY.as_bytes().to_vec(),
                set_cookies: vec![],
            })
        })
    }

    pub fn scripted(
        token: String,
        usage: impl Fn(usize) -> Result<RawReply, ProviderError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            token,
            session_calls: AtomicUsize::new(0),
            usage_calls: AtomicUsize::new(0),
            last_account: Mutex::new(None),
            usage: Box::new(usage),
        }
    }

    pub fn exchanges(&self) -> usize {
        self.session_calls.load(Ordering::SeqCst)
    }

    pub fn usages(&self) -> usize {
        self.usage_calls.load(Ordering::SeqCst)
    }
}

impl ChatGptTransport for FakeTransport {
    async fn session(&self, _cookie: &str) -> Result<RawReply, ProviderError> {
        self.session_calls.fetch_add(1, Ordering::SeqCst);
        Ok(RawReply {
            body: format!(r#"{{"accessToken":"{}"}}"#, self.token).into_bytes(),
            set_cookies: vec!["session=renewed".to_owned()],
        })
    }

    async fn usage(
        &self,
        _cookie: &str,
        _token: &str,
        account_id: Option<&str>,
    ) -> Result<RawReply, ProviderError> {
        let n = self.usage_calls.fetch_add(1, Ordering::SeqCst) + 1;
        *self.last_account.lock().unwrap() = Some(account_id.map(str::to_owned));
        (self.usage)(n)
    }
}

#[derive(Default)]
struct Inner {
    state: AuthSessionState,
    cookie: Option<String>,
}

/// In-memory session honoring the real ChatGPT allowlist.
#[derive(Default)]
pub struct FakeSession {
    inner: Mutex<Inner>,
}

impl FakeSession {
    pub fn signed_in(cookie: &str) -> Arc<Self> {
        let s = Arc::new(Self::default());
        s.save(cookie);
        s
    }

    pub fn save(&self, cookie: &str) {
        let mut g = self.inner.lock().unwrap();
        g.cookie = Some(cookie.to_owned());
        g.state.mark_signed_in(None);
    }

    pub fn sign_out(&self) {
        let mut g = self.inner.lock().unwrap();
        g.cookie = None;
        g.state.mark_session_invalid(None);
    }

    pub fn generation(&self) -> SessionGeneration {
        self.inner.lock().unwrap().state.session_generation
    }
}

impl AuthSession for FakeSession {
    fn cookie_header(&self) -> Option<String> {
        self.inner.lock().unwrap().cookie.clone()
    }
    fn state(&self) -> AuthSessionState {
        self.inner.lock().unwrap().state.clone()
    }
    fn record_auth_failure(&self, reason: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        let invalid = g.state.record_auth_failure(reason);
        if invalid {
            g.cookie = None;
        }
        invalid
    }
    fn record_auth_success(&self) {
        self.inner.lock().unwrap().state.record_auth_success();
    }
    fn is_current(&self, generation: SessionGeneration) -> bool {
        self.inner.lock().unwrap().state.is_current(generation)
    }
    fn add_listener(&self, _listener: Arc<dyn AuthEventListener>) {}
}

impl ProviderAuthSession for FakeSession {
    fn apply_refreshed_cookies(&self, set_cookie_headers: &[String]) {
        let cfg = chatgpt_auth_config();
        let mut g = self.inner.lock().unwrap();
        let Some(stored) = g.cookie.clone() else {
            return;
        };
        let mut pairs: Vec<(String, String)> = stored
            .split(';')
            .filter_map(|p| p.trim().split_once('='))
            .map(|(n, v)| (n.to_owned(), v.to_owned()))
            .collect();
        for header in set_cookie_headers {
            let Some((name, value)) = header.split(';').next().and_then(|p| p.split_once('='))
            else {
                continue;
            };
            if !cfg.is_essential(name) || value.is_empty() {
                continue;
            }
            pairs.retain(|(n, _)| n.to_lowercase() != name.to_lowercase());
            pairs.push((name.to_owned(), value.to_owned()));
        }
        g.cookie = Some(
            pairs
                .iter()
                .map(|(n, v)| format!("{n}={v}"))
                .collect::<Vec<_>>()
                .join("; "),
        );
    }

    fn set_needs_sign_in(&self, needs: bool) {
        self.inner.lock().unwrap().state.needs_sign_in = needs;
    }
    fn save_account_email(&self, _email: &str) {}
    fn note_account_identity(&self, _identity: &str) {}
    fn mark_session_invalid(&self, reason: Option<&str>) {
        let mut g = self.inner.lock().unwrap();
        g.cookie = None;
        g.state.mark_session_invalid(reason);
    }
    fn sign_out(&self) {
        FakeSession::sign_out(self);
    }
    fn save(&self, cookie_header: &str) -> Result<(), CredentialStoreError> {
        FakeSession::save(self, cookie_header);
        Ok(())
    }
    fn adopt(&self, _captured: &CapturedSession) -> Result<(), CredentialStoreError> {
        Ok(())
    }
    fn refresh_from_disk(&self) {}
    fn workspace_id(&self) -> Option<String> {
        None
    }
    fn set_workspace_id(&self, _id: &str) {}
}
