import Foundation

/// The Settings window's tabs, in display order.
///
/// - Note: Raw values are stable identifiers; they are not persisted today,
///   but keep them unchanged if they ever are.
enum SettingsTab: String, CaseIterable, Hashable, Sendable {
    case general
    case providers
    case accounts
    case data

    /// Human-readable, user-visible tab name.
    var displayName: String {
        switch self {
        case .general: return "General"
        case .providers: return "Providers"
        case .accounts: return "Accounts"
        case .data: return "Data"
        }
    }

    /// SF Symbol shown on the tab.
    var systemImage: String {
        switch self {
        case .general: return "gearshape"
        case .providers: return "square.stack.3d.up"
        case .accounts: return "person.crop.circle"
        case .data: return "externaldrive"
        }
    }

    /// The tab Settings opens on.
    ///
    /// With nothing signed in (first run, or after signing out of everything)
    /// there is nothing to show yet, so Settings opens on Accounts, where the
    /// user signs in. Otherwise it opens on General.
    ///
    /// - Parameter isAnyProviderSignedIn: Whether any provider holds a session
    ///   or, for OpenRouter, an API key.
    /// - Returns: `.accounts` when nothing is signed in, `.general` otherwise.
    static func initial(isAnyProviderSignedIn: Bool) -> SettingsTab {
        isAnyProviderSignedIn ? .general : .accounts
    }
}
