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

    func testNoItemsMeansNoRows() {
        XCTAssertEqual(FlowLayout.rows(sizes: [], maxWidth: 300, spacing: 14), [])
    }
}
