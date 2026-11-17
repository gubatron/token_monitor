import SwiftUI

/// SuperGrok "Usage Breakdown" card: the week's usage split by product (Chat,
/// Build, Imagine, …), most used first, each as its share of the usage.
struct GrokUsageBreakdownCard: View {
    /// Products from ``ProductCatalog/breakdownProducts(_:visible:)``.
    let products: [ProductUsage]

    var body: some View {
        UsageBreakdownCard(items: products.map { product in
            UsageBreakdownCard.Item(
                id: product.id,
                label: ProductCatalog.shortName(for: product.id),
                color: .product(product.colorToken),
                amount: product.percentOfPool
            )
        })
    }
}
