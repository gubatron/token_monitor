import SwiftUI

/// Compact Grok product chip: color mark, short name, and percent of the weekly pool.
struct CategoryRow: View {
    let product: ProductUsage

    var body: some View {
        UsageLegendChip(
            color: .product(product.colorToken),
            label: ProductCatalog.shortName(for: product.id),
            value: "\(Int(product.percentOfPool.rounded()))%"
        )
    }
}

/// Legend chip for a segmented usage bar: color mark, name, and value on one
/// line. The name is quieter than the value, so the numbers stand out.
struct UsageLegendChip: View {
    let color: Color
    let label: String
    let value: String

    var body: some View {
        HStack(spacing: 5) {
            Circle()
                .fill(color)
                .frame(width: 7, height: 7)
            // Same treatment as the weekday labels under the daily budget bars.
            Text(label)
                .font(PanelTypography.microDigit)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            Text(value)
                .font(PanelTypography.bodyDigit)
                .foregroundStyle(.primary)
                .fixedSize()
        }
    }
}
