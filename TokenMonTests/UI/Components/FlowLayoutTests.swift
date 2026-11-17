@testable import TokenMon
import XCTest

/// Row breaking for the wrapping legend layout.
final class FlowLayoutTests: XCTestCase {
    private func sizes(_ widths: [CGFloat]) -> [CGSize] {
        widths.map { CGSize(width: $0, height: 14) }
    }

    func testItemsShareARowWhileTheyFit() {
        XCTAssertEqual(FlowLayout.rows(sizes: sizes([80, 90, 100]), maxWidth: 300, spacing: 14), [[0, 1, 2]])
    }

    func testItemsWrapWhenTheNextWouldOverflow() {
        // 80 + 14 + 90 + 14 + 100 = 298 > 290, so the third wraps.
        XCTAssertEqual(FlowLayout.rows(sizes: sizes([80, 90, 100, 60]), maxWidth: 290, spacing: 14), [[0, 1], [2, 3]])
    }

    func testAnItemWiderThanTheRowGetsItsOwnRow() {
        XCTAssertEqual(FlowLayout.rows(sizes: sizes([400, 50]), maxWidth: 300, spacing: 14), [[0], [1]])
    }

    /// A spread row fills the width with equal gaps: 300 − (60 + 80 + 100) = 60
    /// free, so two gaps of 30.
    func testSpreadRowHasEqualGapsAndFillsTheWidth() {
        let xs = FlowLayout.xPositions(widths: [60, 80, 100], maxWidth: 300, spacing: 14, spreads: true)
        XCTAssertEqual(xs, [0, 90, 200])
        XCTAssertEqual(xs[2] + 100, 300)
    }

    /// A row that is not spread keeps the minimum gap from the leading edge.
    func testUnspreadRowKeepsTheMinimumGap() {
        XCTAssertEqual(FlowLayout.xPositions(widths: [60, 80], maxWidth: 300, spacing: 14, spreads: false), [0, 74])
    }

    /// One subview, or a row too full to spread, stays at the minimum gap.
    func testSpreadNeverGoesBelowTheMinimumGap() {
        XCTAssertEqual(FlowLayout.xPositions(widths: [60], maxWidth: 300, spacing: 14, spreads: true), [0])
        XCTAssertEqual(FlowLayout.xPositions(widths: [150, 150], maxWidth: 300, spacing: 14, spreads: true), [0, 164])
        XCTAssertEqual(FlowLayout.xPositions(widths: [60, 80], maxWidth: .infinity, spacing: 14, spreads: true), [0, 74])
    }

    func testNoItemsMeansNoRows() {
        XCTAssertEqual(FlowLayout.rows(sizes: [], maxWidth: 300, spacing: 14), [])
    }
}
