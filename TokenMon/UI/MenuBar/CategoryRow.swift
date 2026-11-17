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
