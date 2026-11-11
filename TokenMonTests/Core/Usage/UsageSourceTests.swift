@testable import TokenMon
import XCTest

/// The "This Mac" wording, which every local-figure label takes from
/// ``UsageSource``.
final class UsageSourceTests: XCTestCase {
    func testRawValuesRoundTripAndRejectUnknownInput() {
        for source in UsageSource.allCases {
            XCTAssertEqual(UsageSource(rawValue: source.rawValue), source)
        }
        XCTAssertEqual(UsageSource.account.rawValue, "account")
        XCTAssertEqual(UsageSource.thisComputer.rawValue, "thisComputer")
        XCTAssertNil(UsageSource(rawValue: "thisPC"))
    }

    func testLocalFiguresSayThisMac() {
        let local = UsageSource.thisComputer
        XCTAssertEqual(local.tag, "This Mac")
        XCTAssertEqual(local.locationPhrase, "on this Mac")
        XCTAssertEqual(local.tokensUnit, "tokens on this Mac")
        XCTAssertEqual(local.qualifying("Tokens this month"), "Tokens this month on this Mac")
    }

    func testAccountFiguresAreNotLabelled() {
        let account = UsageSource.account
        XCTAssertNil(account.tag)
        XCTAssertNil(account.locationPhrase)
        XCTAssertEqual(account.tokensUnit, "tokens")
        XCTAssertEqual(account.qualifying("Monthly spend"), "Monthly spend")
    }
}
