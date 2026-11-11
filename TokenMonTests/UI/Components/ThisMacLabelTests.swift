import SwiftUI
@testable import TokenMon
import XCTest

/// Which panel sections carry the "This Mac" label. Figures read from this
/// computer's logs must say so; account-wide figures must not.
///
/// The section tests read the grid and header each panel view actually draws
/// (its `stats`, `grid`, or `header`), so they fail when a local section stops
/// passing its source along, not only when a constant changes.
@MainActor
final class ThisMacLabelTests: XCTestCase {
    private let utc = ClaudeCodeUsageLedger.dayCalendar
    private let opus = "claude-opus-5-5"

    private func summary() throws -> ClaudeCodeUsageSummary {
        let now = try XCTUnwrap(ISO8601DateFormatter.parseFlexible("2026-11-09T12:00:00Z"))
        let counts = ClaudeModelTokens(inputTokens: 1_200_000, outputTokens: 2_000_000, responseCount: 3)
        return ClaudeCodeUsageSummary.make(days: ["2026-11-02": [opus: counts]], now: now, calendar: utc)
    }

    /// Each cell's spoken title, in order.
    private func spokenTitles(_ grid: MetricStatGrid) -> [String] {
        grid.stats.map(grid.spokenTitle(of:))
    }

    // MARK: - Data sources

    func testLocalSourcesAreThisComputer() {
        XCTAssertEqual(ClaudeCodeUsageSummary.source, .thisComputer)
        XCTAssertEqual(OpenCodeStatsRow.source, .thisComputer)
        XCTAssertEqual(OpenCodeModelsSection.source, .thisComputer)
        XCTAssertEqual(DailyTokenUsage(claudeModels: [opus: ClaudeModelTokens(outputTokens: 1)]).source, .thisComputer)
    }

    // MARK: - Claude panel

    func testClaudeCodeStatsAreLabelled() throws {
        let stats = ClaudeCodeUsageSection(usage: try summary(), modelsExpanded: .constant(true)).stats
        XCTAssertEqual(stats.source.tag, "This Mac")
        XCTAssertEqual(spokenTitles(stats), [
            "Tokens this month on this Mac",
            "Spend this month on this Mac",
            "Total tokens on this Mac",
            "Total spend on this Mac"
        ])
    }

    func testClaudeModelsHeaderIsLabelled() throws {
        let header = ClaudeModelsSection(usage: try summary(), isExpanded: .constant(true)).header
        XCTAssertEqual(header.source.tag, "This Mac")
        XCTAssertEqual(header.spokenTitle, "Models This Month on this Mac")
    }

    func testModelHoverSaysTokensOnThisMac() throws {
        let usage = try summary()
        let model = try XCTUnwrap(usage.monthModels.first)
        let detail = ClaudeModelHoverDetail(model: model, summary: usage)
        XCTAssertTrue(
            detail.accessibilityLabel.hasPrefix("Opus 5.5, November: 3.2M tokens on this Mac, about $"),
            detail.accessibilityLabel
        )
        XCTAssertEqual(ClaudeCodeUsageSummary.source.tokensUnit, "tokens on this Mac")
    }

    /// The bars' hover labels its Claude Code tokens; the bars' percentages
    /// (the account's weekly pool) are left alone.
    func testDailyBarTokensAreLabelledButNotTheBudgetLine() throws {
        let date = try XCTUnwrap(ISO8601DateFormatter.parseFlexible("2026-11-02T12:00:00Z"))
        let day = DailyBudgetDay(date: date, spentUSD: 7, budgetUSD: 14)
        let bars = ClaudeUsagePoller.attachTokens(
            ["2026-11-02": [opus: ClaudeModelTokens(outputTokens: 44_000_000)]],
            to: [day],
            calendar: utc
        )
        XCTAssertEqual(bars.first?.tokens?.source.tokensUnit, "tokens on this Mac")
        XCTAssertFalse(DailyBudgetHover.usageLine(day, allowanceNoun: "weekly").contains("Mac"))
    }

    // MARK: - OpenCode panel

    func testOpenCodeLocalStatsAreLabelled() {
        let grid = OpenCodeStatsRow(monthlySpendUSD: 12, totalTokens: 1_000, inputTokens: 400, outputTokens: 600).grid
        XCTAssertEqual(grid.source.tag, "This Mac")
        XCTAssertEqual(spokenTitles(grid), [
            "Monthly spend on this Mac",
            "Total tokens on this Mac",
            "Input tokens on this Mac",
            "Output tokens on this Mac"
        ])
    }

    func testOpenCodeModelsHeaderIsLabelled() {
        let header = OpenCodeModelsSection(models: [], isExpanded: .constant(true)).header
        XCTAssertEqual(header.spokenTitle, "Top Models by Spend on this Mac")
    }

    // MARK: - Account figures

    func testAccountStatsAreNotLabelled() {
        let grid = MetricStatGrid(source: .account, [MetricStat(title: "Monthly spend", value: "$1.00")])
        XCTAssertNil(grid.source.tag)
        XCTAssertEqual(spokenTitles(grid), ["Monthly spend"])
        let header = CollapsibleSectionHeader(title: "Models", source: .account, isExpanded: .constant(true))
        XCTAssertEqual(header.spokenTitle, "Models")
    }

    // MARK: - Overview

    func testOverviewTooltipLabelsOnlyLocalTokens() {
        func segment(_ title: String, _ source: UsageSource) -> OverviewHourlyUsageChart.HourBarSegment {
            .init(
                id: title,
                title: title,
                activity: 1.2,
                tokens: 3_400_000,
                tokenSource: source,
                color: .clear,
                quotaLabel: "of weekly share"
            )
        }
        XCTAssertEqual(
            OverviewHourlyUsageChart.segmentHelp(segment("OpenCode Go", .thisComputer)),
            "OpenCode Go 1.20% of weekly share · 3.4M tokens on this Mac"
        )
        XCTAssertEqual(
            OverviewHourlyUsageChart.segmentHelp(segment("Cursor", .account)),
            "Cursor 1.20% of weekly share · 3.4M"
        )
    }
}
