@testable import TokenMon
import XCTest

/// Pointer and placement math for the daily bars' floating hover label.
final class DailyBudgetHoverTests: XCTestCase {
    func testColumnIndexMapsThePointerToABar() {
        XCTAssertEqual(DailyBudgetHover.columnIndex(x: 0, columnWidth: 40, columns: 7), 0)
        XCTAssertEqual(DailyBudgetHover.columnIndex(x: 119.9, columnWidth: 40, columns: 7), 2)
        XCTAssertEqual(DailyBudgetHover.columnIndex(x: 279, columnWidth: 40, columns: 7), 6)
        XCTAssertNil(DailyBudgetHover.columnIndex(x: 280, columnWidth: 40, columns: 7))
        XCTAssertNil(DailyBudgetHover.columnIndex(x: -1, columnWidth: 40, columns: 7))
        XCTAssertNil(DailyBudgetHover.columnIndex(x: 10, columnWidth: 0, columns: 0))
    }

    /// Only the stem (plus a little slack) shows the label; the gaps between
    /// stems hide it.
    func testStemIndexCoversOnlyTheStem() {
        let slop = DailyBudgetHover.stemHitSlop
        // Column 1 spans 40..<80 with its 12-wide stem centered on 60.
        XCTAssertEqual(DailyBudgetHover.stemIndex(x: 60, columnWidth: 40, columns: 7, stemWidth: 12), 1)
        XCTAssertEqual(DailyBudgetHover.stemIndex(x: 54 - slop, columnWidth: 40, columns: 7, stemWidth: 12), 1)
        XCTAssertEqual(DailyBudgetHover.stemIndex(x: 66 + slop, columnWidth: 40, columns: 7, stemWidth: 12), 1)
        XCTAssertNil(DailyBudgetHover.stemIndex(x: 53 - slop, columnWidth: 40, columns: 7, stemWidth: 12))
        XCTAssertNil(DailyBudgetHover.stemIndex(x: 67 + slop, columnWidth: 40, columns: 7, stemWidth: 12))
        XCTAssertNil(DailyBudgetHover.stemIndex(x: 41, columnWidth: 40, columns: 7, stemWidth: 12))
        XCTAssertNil(DailyBudgetHover.stemIndex(x: 300, columnWidth: 40, columns: 7, stemWidth: 12))
    }

    /// The label floats left of the pointer, flips right when there is no
    /// room on the left, and stays inside the chart when neither side fits.
    func testLabelFloatsBesideThePointer() {
        let size = CGSize(width: 100, height: 60)
        let gap = HoverLabelPlacement.pointerGap
        XCTAssertEqual(
            HoverLabelPlacement.labelOrigin(pointer: CGPoint(x: 200, y: 50), labelSize: size, containerWidth: 300),
            CGPoint(x: 200 - gap - 100, y: 20)
        )
        XCTAssertEqual(
            HoverLabelPlacement.labelOrigin(pointer: CGPoint(x: 40, y: 50), labelSize: size, containerWidth: 300),
            CGPoint(x: 40 + gap, y: 20)
        )
        XCTAssertEqual(
            HoverLabelPlacement.labelOrigin(pointer: CGPoint(x: 100, y: 50), labelSize: CGSize(width: 250, height: 60), containerWidth: 300),
            CGPoint(x: 0, y: 20)
        )
    }

    func testUsageLineDescribesTheDay() {
        let date = Date(timeIntervalSinceReferenceDate: 0)
        XCTAssertEqual(
            DailyBudgetHover.usageLine(DailyBudgetDay(date: date, spentUSD: 31, budgetUSD: 100.0 / 7), allowanceNoun: "weekly"),
            "31.0% of weekly · 217% of daily allowance"
        )
        XCTAssertEqual(
            DailyBudgetHover.usageLine(DailyBudgetDay(date: date, spentUSD: 0, budgetUSD: 100.0 / 7), allowanceNoun: "weekly"),
            "0% of the 14.3% daily allowance"
        )
    }
}
