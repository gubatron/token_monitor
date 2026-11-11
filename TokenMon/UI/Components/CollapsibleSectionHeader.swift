import SwiftUI

/// A section header that opens and closes its section, with a chevron that
/// points down while open, and a "This Mac" tag before the chevron when the
/// section's figures were counted on this computer.
struct CollapsibleSectionHeader: View {
    let title: String
    /// Where the section's figures come from; local ones are tagged.
    let source: UsageSource
    @Binding var isExpanded: Bool

    /// What VoiceOver reads for the header: "Models This Month on this Mac"
    /// for a local section, the plain title otherwise.
    var spokenTitle: String {
        source.qualifying(title)
    }

    var body: some View {
        Button {
            withAnimation(.easeInOut(duration: 0.2)) { isExpanded.toggle() }
        } label: {
            HStack {
                PanelSectionHeader(title: title)
                Spacer()
                UsageSourceTag(source: source)
                Image(systemName: "chevron.down")
                    .font(PanelTypography.caption)
                    .foregroundStyle(.secondary)
                    .rotationEffect(.degrees(isExpanded ? 0 : -90))
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(spokenTitle)
        .accessibilityValue(isExpanded ? "Expanded" : "Collapsed")
    }
}
