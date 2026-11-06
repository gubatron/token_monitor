//! Launch-at-login behavior against a fake OS service.

use std::sync::{Arc, Mutex};

use tokenmon_core::settings::{LaunchAtLogin, LoginItemError, LoginItemService, LoginItemStatus};

struct Fake {
    status: Mutex<LoginItemStatus>,
    fail: bool,
    /// Status after a successful register (models StartupApproved disabled).
    after_register: LoginItemStatus,
}

impl LoginItemService for Fake {
    fn status(&self) -> LoginItemStatus {
        *self.status.lock().expect("lock")
    }
    fn register(&self) -> Result<(), LoginItemError> {
        if self.fail {
            return Err(LoginItemError("denied".into()));
        }
        *self.status.lock().expect("lock") = self.after_register;
        Ok(())
    }
    fn unregister(&self) -> Result<(), LoginItemError> {
        *self.status.lock().expect("lock") = LoginItemStatus::NotRegistered;
        Ok(())
    }
}

fn fake(status: LoginItemStatus, fail: bool, after: LoginItemStatus) -> Arc<Fake> {
    Arc::new(Fake {
        status: Mutex::new(status),
        fail,
        after_register: after,
    })
}

/// A login item awaiting approval still reads as on, and flags approval.
#[test]
fn test_launch_at_login_shows_on_while_awaiting_approval() {
    let login = LaunchAtLogin::new(fake(
        LoginItemStatus::RequiresApproval,
        false,
        LoginItemStatus::Enabled,
    ));
    assert!(login.is_on());
    assert!(login.needs_approval());
}

/// Register succeeds, unregister turns it off, and a failure reverts the toggle.
#[test]
fn test_launch_at_login_registers_and_reverts_on_failure() {
    let ok = LaunchAtLogin::new(fake(
        LoginItemStatus::NotRegistered,
        false,
        LoginItemStatus::Enabled,
    ));
    assert!(ok.set(true));
    assert!(!ok.set(false));
    let bad = LaunchAtLogin::new(fake(
        LoginItemStatus::NotRegistered,
        true,
        LoginItemStatus::Enabled,
    ));
    assert!(!bad.set(true));
    assert!(!bad.refresh());
}
