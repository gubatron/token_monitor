//! DPAPI-encrypted (user scope) vault file `%LOCALAPPDATA%\TokenMon\vault.bin`
//! and the credential stores built on it (spec 01 sections 1.6.1 to 1.6.3).
//!
//! Layers, all portable except the DPAPI encryptor:
//!
//! * [`Encryptor`]: the OS encryption seam (DPAPI on Windows, [`FakeEncryptor`]
//!   in tests).
//! * [`decode_payload`] / [`encode_payload`]: the decrypted vault JSON format.
//!   The decoder takes untrusted bytes and is fuzzed.
//! * [`Vault`]: one encrypted file holding every provider's secrets, with
//!   write-then-read-back verification, pending removals and the
//!   "unavailable means locked out for the process" rule.
//! * [`FileBackedCredentialStore`]: plain owner-only `<prefix><key>.dat` files.
//! * [`SecretRoutingCredentialStore`]: the production
//!   [`CredentialStore`](tokenmon_core::CredentialStore) that routes by
//!   [`CredentialKind`](tokenmon_core::CredentialKind).
//!
//! Secrets are held in [`zeroize::Zeroizing`] buffers and are never logged.

mod encryptor;
mod files;
mod payload;
mod routing;
mod secure_file;
mod vault;

#[cfg(windows)]
mod dpapi;

#[cfg(windows)]
pub use dpapi::DpapiEncryptor;
pub use encryptor::{CryptError, Encryptor, FakeEncryptor};
pub use files::{FileBackedCredentialStore, atomic_write};
pub use payload::{
    MAX_PAYLOAD_BYTES, PayloadError, VaultPayload, decode_payload, decode_pending, encode_payload,
};
pub use routing::{SecretRoutingCredentialStore, SecretStoreError, VAULT_UNAVAILABLE_REASON};
#[cfg(windows)]
pub(crate) use secure_file::current_user_sid;
pub use secure_file::{
    WindowsOwnerOnly, create_new_owner_only, owner_only_directory_sddl, owner_only_sddl,
    register_owner_only_permissions,
};
pub use vault::{PENDING_FILE_NAME, VAULT_FILE_NAME, Vault};
