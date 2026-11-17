import SwiftUI

/// Grokbot "Usage Breakdown" card: one bar split by who spent the allowance
/// (You, Routines, Bot to Bot, …), most used first, with a legend underneath.
struct GrokbotUsageBreakdownCard: View {
    let breakdown: GrokbotUsageBreakdown

    var body: some View {
        PanelCard {
            PanelSectionHeader(title: "Usage Breakdown")
            SegmentedUsageBar(segments: segments, height: 8)
            // Chips keep their natural width and wrap, so no name is cut off.
            FlowLayout {
                ForEach(breakdown.slices) { slice in
                    UsageLegendChip(
                        color: slice.source.sRGB.color,
                        label: slice.source.displayName,
                        value: valueText(slice.value)
                    )
                }
            }
        }
    }

    /// Percent units draw against the whole allowance, leaving the unused part
    /// as track. Dollars have no allowance to draw against, so each source
    /// takes its share of the total.
    private var segments: [SegmentedUsageBar.Segment] {
        let total = breakdown.total
        return breakdown.slices.map { slice in
            let percent: Double
            switch breakdown.unit {
            case .weeklyLimitPercent, .billingCyclePercent: percent = slice.value
            case .usd: percent = total > 0 ? slice.value / total * 100 : 0
            }
            return SegmentedUsageBar.Segment(id: slice.source.rawValue, percent: percent, color: slice.source.sRGB.color)
        }
    }

    private func valueText(_ value: Double) -> String {
        switch breakdown.unit {
        case .weeklyLimitPercent, .billingCyclePercent: return "\(Int(value.rounded()))%"
        case .usd: return Format.usd(value)
        }
    }
}

#if DEBUG
#Preview {
    GrokbotUsageBreakdownCard(
        breakdown: GrokbotUsageBreakdown(
            unit: .weeklyLimitPercent,
            slices: [
                .init(source: .user, value: 50.31),
                .init(source: .automation, value: 28.09),
                .init(source: .agent, value: 21.6)
            ]
        )
    )
    .padding()
    .frame(width: 340)
}
#endif
