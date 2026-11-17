@testable import TokenMon
import XCTest

final class ProductCatalogTests: XCTestCase {
    private func product(_ id: String, _ pct: Double) -> ProductUsage {
        ProductUsage(id: id, displayName: ProductCatalog.displayName(for: id), percentOfPool: pct)
    }

    func testFilteredKeepsVisibleAboveThreshold() {
        let products = [
            product("chat", 50),
            product("build", 30),
            product("api", 2)
        ]
        let result = ProductCatalog.filtered(
            products,
            visible: ["chat", "build", "api"],
            threshold: 5
        )
        XCTAssertEqual(result.map(\.id), ["chat", "build"])
    }

    func testFilteredExcludesHiddenProducts() {
        let products = [product("chat", 50), product("api", 10)]
        let result = ProductCatalog.filtered(products, visible: ["chat"], threshold: 0)
        XCTAssertEqual(result.map(\.id), ["chat"])
    }

    func testFilteredOrdersByDisplayOrder() {
        let products = [product("other", 30), product("chat", 40), product("build", 20)]
        let result = ProductCatalog.filtered(products, visible: ["other", "chat", "build"], threshold: 0)
        XCTAssertEqual(result.map(\.id), ["chat", "build", "other"])
    }

    func testFilteredEmptyWhenNoneAboveThreshold() {
        let products = [product("chat", 3), product("api", 1)]
        let result = ProductCatalog.filtered(products, visible: ["chat", "api"], threshold: 5)
        XCTAssertTrue(result.isEmpty)
    }

    /// A breakdown-less snapshot arrives as a synthesized "other" slice carrying
    /// the full used%; the panel shows it so the bar carries that usage.
    func testPanelProductsKeepsSynthesizedOtherSlice() {
        let synthesized = GrokUsageClient.synthesizeProducts(usedPercent: 64)
        let result = ProductCatalog.panelProducts(synthesized, visible: Set(ProductCatalog.knownIDs))
        XCTAssertEqual(result.map(\.id), ["other"])
        XCTAssertEqual(result.first?.percentOfPool, 64)
    }

    /// The panel keeps any non-zero product but, like the menu bar, drops a
    /// zero-value one so a breakdown-less snapshot does not paint four 0% rows.
    func testPanelProductsDropsZeroValueProducts() {
        let products = [product("chat", 0), product("build", 40)]
        let result = ProductCatalog.panelProducts(products, visible: ["chat", "build"])
        XCTAssertEqual(result.map(\.id), ["build"])
    }

    /// Hidden products stay hidden in the panel.
    func testPanelProductsRespectsVisibility() {
        let products = [product("chat", 40), product("api", 20)]
        let result = ProductCatalog.panelProducts(products, visible: ["chat"])
        XCTAssertEqual(result.map(\.id), ["chat"])
    }

    /// The Categories card always shows Chat, Build, Imagine, and Voice — absent
    /// or unused products fall back to a 0% placeholder; API and Other never show.
    func testCategoryProductsAlwaysIncludesTheFourCategories() {
        let products = [
            product("chat", 40),
            product("api", 8),
            product("other", 7)
        ]
        let result = ProductCatalog.categoryProducts(products, visible: Set(ProductCatalog.knownIDs))
        XCTAssertEqual(result.map(\.id), ["chat", "build", "imagine", "voice"])
        XCTAssertEqual(result.map(\.percentOfPool), [40, 0, 0, 0])
    }

    /// The usage bar runs from the most used product on the left to the least
    /// used on the right.
    func testPanelProductsOrdersMostUsedFirst() {
        let products = [
            product("chat", 5),
            product("build", 40),
            product("api", 12),
            product("imagine", 20)
        ]
        let result = ProductCatalog.panelProducts(products, visible: Set(ProductCatalog.knownIDs))
        XCTAssertEqual(result.map(\.id), ["build", "imagine", "api", "chat"])
    }

    /// The category chips run from most used to least used, with unused
    /// categories last in display order.
    func testCategoryProductsOrdersMostUsedFirst() {
        let products = [product("chat", 3), product("voice", 9), product("build", 30)]
        let result = ProductCatalog.categoryProducts(products, visible: Set(ProductCatalog.knownIDs))
        XCTAssertEqual(result.map(\.id), ["build", "voice", "chat", "imagine"])
    }

    /// Equal usage keeps the canonical display order.
    func testSortedByUsageBreaksTiesByDisplayOrder() {
        let products = [product("voice", 10), product("build", 10), product("chat", 10)]
        XCTAssertEqual(ProductCatalog.sortedByUsage(products).map(\.id), ["chat", "build", "voice"])
    }

    /// Hidden categories stay hidden, even the 0% placeholders.
    func testCategoryProductsRespectsVisibility() {
        let result = ProductCatalog.categoryProducts(
            [product("chat", 40)],
            visible: ["chat", "voice"]
        )
        XCTAssertEqual(result.map(\.id), ["chat", "voice"])
    }
}
