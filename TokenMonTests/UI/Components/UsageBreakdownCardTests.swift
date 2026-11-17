@testable import TokenMon
import XCTest

/// Legend shares for the Usage Breakdown card's always-full bar.
final class UsageBreakdownCardTests: XCTestCase {
    /// SuperGrok products at 25 / 12 / 6 / 4 % of the pool (47% used) read as
    /// shares of the usage.
    func testSharesAreOfTheUsageNotThePool() {
        XCTAssertEqual(UsageBreakdownCard.sharePercents([25, 12, 6, 4]), [53, 26, 13, 8])
    }

    /// Rounding each share alone gives 33 + 33 + 33 = 99; the largest
    /// remainders take the missing point so the legend sums to 100.
    func testSharesAlwaysSumToOneHundred() {
        let shares = UsageBreakdownCard.sharePercents([1, 1, 1])
        XCTAssertEqual(shares.reduce(0, +), 100)
        XCTAssertEqual(shares, [34, 33, 33])
        XCTAssertEqual(UsageBreakdownCard.sharePercents([50.31, 28.09, 21.6]), [50, 28, 22])
    }

    func testNothingUsedGivesZeroShares() {
        XCTAssertEqual(UsageBreakdownCard.sharePercents([0, 0]), [0, 0])
        XCTAssertEqual(UsageBreakdownCard.sharePercents([]), [])
        XCTAssertEqual(UsageBreakdownCard.sharePercents([-5, 10]), [0, 100])
    }
}
