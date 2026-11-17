import SwiftUI

/// "Usage Breakdown" card shared by the provider panels: one bar split by what
/// spent the pool, then a legend of the same items, most used first.
///
/// The bar always fills its width: each item takes its share of the total
/// used, and the legend shows that share (summing to 100%) unless an item
/// supplies its own figure. The weekly card above carries the overall % used.
struct UsageBreakdownCard: View {
    /// One slice of the breakdown.
    struct Item: Identifiable {
        var id: String
        var label: String
        var color: Color
        /// The item's usage, in any unit shared by all items.
        var amount: Double
        /// The legend's figure (e.g. "$1.25"); nil shows the share of usage.
        var valueText: String?
    }

    /// Items in display order (most used first).
    let items: [Item]

    var body: some View {
        let total = items.reduce(0) { $0 + max(0, $1.amount) }
        let shares = Self.sharePercents(items.map(\.amount))
        PanelCard {
            PanelSectionHeader(title: "Usage Breakdown")
            SegmentedUsageBar(
                segments: items.map { item in
                    SegmentedUsageBar.Segment(
                        id: item.id,
                        percent: total > 0 ? max(0, item.amount) / total * 100 : 0,
                        color: item.color
                    )
                },
                height: 8
            )
            // Chips keep their natural width and wrap, so no name is cut off.
            FlowLayout {
                ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                    UsageLegendChip(
                        color: item.color,
                        label: item.label,
                        value: item.valueText ?? "\(shares[index])%"
                    )
                }
            }
        }
    }

    /// Whole-number shares of `amounts` that add up to exactly 100 (largest
    /// remainder), so the legend never reads 99% or 101%; all zeros when
    /// nothing was used. Negative amounts count as zero.
    static func sharePercents(_ amounts: [Double]) -> [Int] {
        let clamped = amounts.map { max(0, $0) }
        let total = clamped.reduce(0, +)
        guard total > 0 else { return clamped.map { _ in 0 } }
        let exact = clamped.map { $0 / total * 100 }
        var shares = exact.map { Int($0.rounded(.down)) }
        let missing = 100 - shares.reduce(0, +)
        let byRemainder = exact.indices.sorted { lhs, rhs in
            let left = exact[lhs] - Double(shares[lhs])
            let right = exact[rhs] - Double(shares[rhs])
            return left != right ? left > right : lhs < rhs
        }
        for index in byRemainder.prefix(missing) {
            shares[index] += 1
        }
        return shares
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
