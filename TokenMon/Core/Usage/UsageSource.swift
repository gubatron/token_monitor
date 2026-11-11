import Foundation

/// Where a usage figure was counted: the provider's account, or this
/// computer's own logs.
///
/// Figures read from local files (Claude Code transcripts, OpenCode's
/// `opencode.db`) cover only what ran on this computer, so the panel labels
/// them "This Mac" to keep them from being read as account-wide totals.
/// Account figures (the 5-hour and weekly pools, the daily bars' percentages,
/// Cursor's and OpenRouter's server numbers) carry no label.
///
/// Every label string comes from here, so the wording changes in one place.
enum UsageSource: String, Codable, CaseIterable, Hashable, Sendable {
    /// Reported by the provider for the whole account, across every device.
    case account
    /// Counted from this computer's local logs only.
    case thisComputer

    /// What the labels call this computer ("Mac"; the Windows app says "PC").
    static let computerName = "Mac"

    /// The short tag on a section of local figures, "This Mac"; nil for
    /// account figures, which are not labelled.
    var tag: String? {
        switch self {
        case .account: return nil
        case .thisComputer: return "This \(Self.computerName)"
        }
    }

    /// The phrase after a local figure, "on this Mac"; nil for account figures.
    var locationPhrase: String? {
        switch self {
        case .account: return nil
        case .thisComputer: return "on this \(Self.computerName)"
        }
    }

    /// The words after a token count: "tokens", or "tokens on this Mac".
    var tokensUnit: String {
        qualifying("tokens")
    }

    /// `text` followed by ``locationPhrase`` when there is one, e.g.
    /// "Tokens this month on this Mac"; VoiceOver reads local sections this way.
    func qualifying(_ text: String) -> String {
        locationPhrase.map { "\(text) \($0)" } ?? text
    }
}
