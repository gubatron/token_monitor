//! Single-instance detection.
//!
//! On Windows this is a named mutex in the session namespace
//! (`Local\TokenMon.SingleInstance`); the fake keeps names in a shared set.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::shell::error::ShellError;

/// Name of the session-local mutex that marks the running instance.
pub const MUTEX_NAME: &str = r"Local\TokenMon.SingleInstance";

/// Opaque RAII token: while it lives, this process is the primary instance.
pub struct InstanceGuard(#[allow(dead_code)] Box<dyn Send>);

impl InstanceGuard {
    /// Wraps a platform resource whose drop releases the instance lock.
    #[must_use]
    pub fn new(resource: impl Send + 'static) -> Self {
        Self(Box::new(resource))
    }
}

impl std::fmt::Debug for InstanceGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InstanceGuard")
    }
}

/// Result of trying to become the primary instance.
#[derive(Debug)]
pub enum InstanceState {
    /// This process is the first; keep the guard for its whole lifetime.
    Primary(InstanceGuard),
    /// Another instance already holds the lock.
    Secondary,
}

/// Claims the single-instance lock.
pub trait InstanceLock: Send + Sync {
    /// Tries to become the primary instance.
    ///
    /// # Errors
    ///
    /// [`ShellError::Instance`] when the OS primitive fails for a reason other
    /// than "already held".
    fn try_acquire(&self) -> Result<InstanceState, ShellError>;
}

/// In-memory [`InstanceLock`]; clones share one lock, like processes sharing a session.
#[derive(Debug, Clone, Default)]
pub struct FakeInstanceLock {
    held: Arc<Mutex<HashSet<String>>>,
    name: String,
}

struct Release {
    held: Arc<Mutex<HashSet<String>>>,
    name: String,
}

impl Drop for Release {
    fn drop(&mut self) {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.name);
    }
}

impl FakeInstanceLock {
    /// Creates an unheld lock.
    #[must_use]
    pub fn new() -> Self {
        Self {
            held: Arc::default(),
            name: MUTEX_NAME.to_owned(),
        }
    }
}

impl InstanceLock for FakeInstanceLock {
    fn try_acquire(&self) -> Result<InstanceState, ShellError> {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.insert(self.name.clone()) {
            Ok(InstanceState::Primary(InstanceGuard::new(Release {
                held: Arc::clone(&self.held),
                name: self.name.clone(),
            })))
        } else {
            Ok(InstanceState::Secondary)
        }
    }
}
