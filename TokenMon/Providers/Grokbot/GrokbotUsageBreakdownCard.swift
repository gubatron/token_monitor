import SwiftUI

/// Grokbot "Usage Breakdown" card: who spent the allowance (You, Routines,
/// Bot to Bot, …), most used first.
struct GrokbotUsageBreakdownCard: View {
    let breakdown: GrokbotUsageBreakdown

    var body: some View {
        UsageBreakdownCard(items: items)
    }

    /// Percent units draw against the whole allowance, leaving the unused part
    /// as track. Dollars have no allowance to draw against, so each source
    /// takes its share of the total.
    private var items: [UsageBreakdownCard.Item] {
        let total = breakdown.total
        return breakdown.slices.map { slice in
            let percent: Double
            let value: String
            switch breakdown.unit {
            case .weeklyLimitPercent, .billingCyclePercent:
                percent = slice.value
                value = "\(Int(slice.value.rounded()))%"
            case .usd:
                percent = total > 0 ? slice.value / total * 100 : 0
                value = Format.usd(slice.value)
            }
            return UsageBreakdownCard.Item(
                id: slice.source.rawValue,
                label: slice.source.displayName,
                color: slice.source.sRGB.color,
                percent: percent,
                value: value
            )
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
