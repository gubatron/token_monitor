import Foundation

/// The privacy note on the Accounts tab, kept in one place so the claim it
/// makes is checked against the storage code in one place.
///
/// Every sentence must stay true of `SecretRoutingCredentialStore` and
/// `KeychainVault`: secrets go only to the login Keychain (a write the
/// Keychain refuses fails instead of falling back to a file); non-secret
/// labels (email, account identity, workspace id) are `0600` files under
/// Application Support; requests carry a credential only to its own provider
/// (`ProviderURLSession`); and sign-out removes the session.
enum AccountsPrivacyNote {
    static let text = "TokenMon has no servers and never reads or stores your passwords. "
        + "When you sign in, the provider's own page handles your login. TokenMon keeps only "
        + "the session it needs (or the OpenRouter key you paste) on this Mac, in your login "
        + "Keychain. "
        + "Non-secret labels such as your email stay in ~/Library/Application Support/TokenMon. "
        + "A session is sent only to the provider it belongs to, and signing out deletes it."
}
