import SwiftUI

/// SuperGrok "Usage Breakdown" card: the weekly pool split by product (Chat,
/// Build, Imagine, …), most used first.
struct GrokUsageBreakdownCard: View {
    /// Products from ``ProductCatalog/breakdownProducts(_:visible:)``.
    let products: [ProductUsage]

    var body: some View {
        UsageBreakdownCard(items: products.map { product in
            UsageBreakdownCard.Item(
                id: product.id,
                label: ProductCatalog.shortName(for: product.id),
                color: .product(product.colorToken),
                percent: product.percentOfPool,
                value: "\(Int(product.percentOfPool.rounded()))%"
            )
        })
    }
}
