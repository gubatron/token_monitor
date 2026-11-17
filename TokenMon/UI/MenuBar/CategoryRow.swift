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

/// Legend chip for a segmented usage bar: color mark, name, and value.
struct UsageLegendChip: View {
    /// Where the value sits relative to the name.
    enum Layout {
        /// On one line after the name.
        case inline
        /// On its own line under the name, for longer names.
        case stacked
    }

    let color: Color
    let label: String
    let value: String
    var layout: Layout = .inline

    var body: some View {
        switch layout {
        case .inline:
            HStack(spacing: 5) {
                mark
                Text(label)
                    .font(PanelTypography.bodySemibold)
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                valueText
            }
        case .stacked:
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 5) {
                    mark
                    Text(label)
                        .font(PanelTypography.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                valueText
            }
        }
    }

    private var mark: some View {
        Circle()
            .fill(color)
            .frame(width: 7, height: 7)
    }

    private var valueText: some View {
        Text(value)
            .font(PanelTypography.bodyDigit)
            .foregroundStyle(.primary)
    }
}
