//! The OpenCode `AuthConfig` must persist the workspace id
//! (`ProviderAuthSession::workspace_id` reads `CredentialKey::Workspace`,
//! listed in `extra_store_keys` so sign-out and invalidation wipe it with the
//! session).

use tokenmon_core::CredentialKey;
use tokenmon_provider_opencode::auth::auth_config;

/// `CredentialKey::Workspace` is in the extra store keys, so the shared
/// session stores and clears the remembered workspace.
#[test]
fn test_auth_config_lists_workspace_in_extra_store_keys() {
    assert!(
        auth_config()
            .extra_store_keys
            .contains(&CredentialKey::Workspace)
    );
}
