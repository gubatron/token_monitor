import SwiftUI

/// The quiet "This Mac" tag on a section whose figures were counted from
/// this computer's logs; draws nothing for account figures.
///
/// VoiceOver skips the tag: the section it marks speaks its own title with
/// ``UsageSource/qualifying(_:)`` ("Tokens this month on this Mac"), so the
/// location is read once, with the figure it belongs to.
struct UsageSourceTag: View {
    let source: UsageSource

    var body: some View {
        if let tag = source.tag {
            HStack(spacing: 3) {
                Image(systemName: "laptopcomputer")
                Text(tag)
            }
            .font(PanelTypography.micro)
            .foregroundStyle(.tertiary)
            .lineLimit(1)
            .fixedSize()
            .accessibilityHidden(true)
        }
    }
}
