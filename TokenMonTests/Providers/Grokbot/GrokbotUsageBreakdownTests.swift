@testable import TokenMon
import XCTest

/// Parsing of `GetGrokBotUsageBreakdown` and the session token it sends.
final class GrokbotUsageBreakdownTests: XCTestCase {
    private func root(_ json: String) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
    }

    /// Shape captured from a live response (values made up): int64 fields as
    /// strings, the source as a protobuf enum name, one row per UTC day.
    private let weeklyFixture = """
    {"weeklyLimitPpm":{"periodStartMs":"1794425100000","periodEndMs":"1795029900000",
     "bySource":[
      {"dayMs":"1794355200000","source":"GROK_BOT_USAGE_SOURCE_USER","ppm":100000},
      {"dayMs":"1794441600000","source":"GROK_BOT_USAGE_SOURCE_USER","ppm":150000},
      {"dayMs":"1794441600000","source":"GROK_BOT_USAGE_SOURCE_AUTOMATION","ppm":300000},
      {"dayMs":"1794441600000","source":"GROK_BOT_USAGE_SOURCE_AGENT","ppm":120000}],
     "byBot":[{"dayMs":"1794441600000","botId":"bot_1","ppm":670000}]}}
    """

    func testWeeklyBreakdownSumsDaysPerSourceAsPercent() throws {
        let breakdown = try XCTUnwrap(GrokbotUsageBreakdown.parse(root(weeklyFixture)))
        XCTAssertEqual(breakdown.unit, .weeklyLimitPercent)
        XCTAssertEqual(breakdown.slices.map(\.source), [.automation, .user, .agent])
        XCTAssertEqual(breakdown.slices.map(\.value), [30, 25, 12])
        XCTAssertEqual(breakdown.total, 67, accuracy: 1e-9)
        XCTAssertEqual(breakdown.periodStart, Date(timeIntervalSince1970: 1_794_425_100))
        XCTAssertEqual(breakdown.periodEnd, Date(timeIntervalSince1970: 1_795_029_900))
    }

    func testBilledCentsBecomeDollars() throws {
        let breakdown = try XCTUnwrap(GrokbotUsageBreakdown.parse(root("""
        {"billed_cents":{"by_source":[{"source":"USER","cents":"250"},{"source":2,"cents":"125"}]}}
        """)))
        XCTAssertEqual(breakdown.unit, .usd)
        XCTAssertEqual(breakdown.slices.map(\.source), [.user, .automation])
        XCTAssertEqual(breakdown.slices.map(\.value), [2.5, 1.25])
    }

    func testBillingCycleShareIsPercent() throws {
        let breakdown = try XCTUnwrap(GrokbotUsageBreakdown.parse(root("""
        {"billingCyclePpm":{"bySource":[{"source":"GROK_BOT_USAGE_SOURCE_GROUP","ppm":50000}]}}
        """)))
        XCTAssertEqual(breakdown.unit, .billingCyclePercent)
        XCTAssertEqual(breakdown.slices.first?.value, 5)
    }

    /// No unit means no breakdown; an empty one has no slices and is hidden.
    func testMissingOrEmptyBreakdown() throws {
        XCTAssertNil(try GrokbotUsageBreakdown.parse(root("{}")))
        let empty = try XCTUnwrap(GrokbotUsageBreakdown.parse(root(#"{"weeklyLimitPpm":{}}"#)))
        XCTAssertTrue(empty.slices.isEmpty)
        XCTAssertFalse(empty.isShown(at: Date()))
    }

    func testUnknownAndUnspecifiedSourcesCountAsOther() {
        XCTAssertEqual(GrokbotUsageSource(wireValue: "GROK_BOT_USAGE_SOURCE_UNSPECIFIED"), .other)
        XCTAssertEqual(GrokbotUsageSource(wireValue: "GROK_BOT_USAGE_SOURCE_SOMETHING_NEW"), .other)
        XCTAssertEqual(GrokbotUsageSource(wireValue: "grok_bot_usage_source_agent"), .agent)
        XCTAssertEqual(GrokbotUsageSource(wireValue: "4"), .group)
        XCTAssertEqual(GrokbotUsageSource.agent.displayName, "Bot to Bot")
        XCTAssertEqual(GrokbotUsageSource(rawValue: "automation"), .automation)
        XCTAssertNil(GrokbotUsageSource(rawValue: "routines"))
    }

    /// Equal values keep the source order.
    func testTiesKeepSourceOrder() throws {
        let breakdown = try XCTUnwrap(GrokbotUsageBreakdown.parse(root("""
        {"weeklyLimitPpm":{"bySource":[{"source":"AGENT","ppm":10000},{"source":"USER","ppm":10000}]}}
        """)))
        XCTAssertEqual(breakdown.slices.map(\.source), [.user, .agent])
    }

    /// A breakdown is hidden once its period has ended.
    func testHiddenAfterThePeriodEnds() {
        let end = Date(timeIntervalSince1970: 1_000)
        let breakdown = GrokbotUsageBreakdown(
            unit: .weeklyLimitPercent,
            periodEnd: end,
            slices: [.init(source: .user, value: 10)]
        )
        XCTAssertTrue(breakdown.isShown(at: end.addingTimeInterval(-1)))
        XCTAssertFalse(breakdown.isShown(at: end))
    }

    // MARK: - Access token

    func testAccessTokenComesFromTheSessionCookie() {
        // Short fixtures: the repo's gitleaks rule flags realistic cookie values.
        let header = "other=1; WorkosCursorSessionToken=u1%3A%3Aa.b.c; theme=dark"
        XCTAssertEqual(GrokbotUsageClient.accessToken(fromCookieHeader: header), "a.b.c")
        XCTAssertEqual(GrokbotUsageClient.accessToken(fromCookieHeader: "WorkosCursorSessionToken=a.b.c"), "a.b.c")
    }

    func testNoAccessTokenWithoutAJWT() {
        XCTAssertNil(GrokbotUsageClient.accessToken(fromCookieHeader: "WorkosCursorSessionToken=live"))
        XCTAssertNil(GrokbotUsageClient.accessToken(fromCookieHeader: "other=aaa.bbb.ccc"))
        XCTAssertNil(GrokbotUsageClient.accessToken(fromCookieHeader: ""))
    }
}
