import SwiftUI

/// "Usage Breakdown" card shared by the provider panels: one bar split by what
/// spent the pool, then a legend of the same items, most used first.
struct UsageBreakdownCard: View {
    /// One slice of the breakdown.
    struct Item: Identifiable {
        var id: String
        var label: String
        var color: Color
        /// Width in the bar, as percent of the bar (0…100); the rest is track.
        var percent: Double
        /// The legend's figure, e.g. "28%" or "$1.25".
        var value: String
    }

    /// Items in display order (most used first).
    let items: [Item]

    var body: some View {
        PanelCard {
            PanelSectionHeader(title: "Usage Breakdown")
            SegmentedUsageBar(
                segments: items.map { SegmentedUsageBar.Segment(id: $0.id, percent: $0.percent, color: $0.color) },
                height: 8
            )
            // Chips keep their natural width and wrap, so no name is cut off.
            FlowLayout {
                ForEach(items) { item in
                    UsageLegendChip(color: item.color, label: item.label, value: item.value)
                }
            }
        }
    }
}

/// Legend chip for a segmented usage bar: color mark, name, and value on one
/// line at the same size (12pt semibold), the name grey and the value primary
/// so the numbers stand out, as under the daily budget bars.
struct UsageLegendChip: View {
    let color: Color
    let label: String
    let value: String

    var body: some View {
        HStack(spacing: 5) {
            Circle()
                .fill(color)
                .frame(width: 7, height: 7)
            Text(label)
                .font(PanelTypography.bodySemibold)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            Text(value)
                .font(PanelTypography.bodyDigit)
                .foregroundStyle(.primary)
                .fixedSize()
        }
    }
}
