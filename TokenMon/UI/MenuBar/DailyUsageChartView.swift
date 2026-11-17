import SwiftUI

/// Settings → Usage style “Daily use” bar chart for the **billing period** week.
///
/// Each day’s track is an equal share of the weekly pool (`100/7`).
/// Grey stem is the daily cap; Grok blue is how much of that cap was used.
/// Footer pacing uses the pulled weekly used % when provided.
struct DailyUsageChartView: View {
    let week: DailyUsageWeek
    var onPreviousWeek: (() -> Void)?
    var onNextWeek: (() -> Void)?
    var canGoNext: Bool = true
    /// Live weekly used % for the current period (omit for past weeks).
    var periodUsedPercent: Double?
    /// Pool reset instant — paces the remainder against time left until reset.
    var resetsAt: Date?

    private let trackHeight: CGFloat = PanelChartStem.height

    /// The week's days as daily-budget days, for pacing and the hover label.
    private var budgetDays: [DailyBudgetDay] {
        week.displayDays.map {
            DailyBudgetDay(
                date: $0.dayStart,
                spentUSD: $0.totalPercent,
                budgetUSD: GrokDailyUsageBuilder.dailyCapPercent
            )
        }
    }

    private var pace: DailyBudget.PaceHeadroom? {
        guard let periodUsedPercent else { return nil }
        let days = budgetDays
        // Billing-period week already spans the full SuperGrok pool window.
        // Credit days elapsed through today so the on-track allowance includes today.
        let elapsed = DailyBudget.elapsedDaysThroughToday(from: week.weekStart)
        return DailyBudget.paceHeadroom(
            days: days,
            periodConsumed: periodUsedPercent,
            elapsedDaysInPeriod: elapsed,
            resetsAt: resetsAt
        )
    }

    private var footerCaption: String? {
        if let pace, let caption = DailyBudget.paceCaption(pace) {
            return caption
        }
        if week.isEstimated || !week.hasDailyData {
            return "Bars update between polls · week totals above"
        }
        return nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                PanelSectionHeader(title: "Daily Budget")
                Text(String(format: "%.1f%%", GrokDailyUsageBuilder.dailyCapPercent))
                    .font(PanelTypography.bodyDigit)
                    .foregroundStyle(.primary)
                Spacer(minLength: 8)
                Text(week.rangeLabel)
                    .font(PanelTypography.bodyDigit)
                    .foregroundStyle(.primary)
            }

            WeekChartNavigation(
                onPreviousWeek: onPreviousWeek,
                onNextWeek: onNextWeek,
                canGoNext: canGoNext
            )

            barsRow

            if let footerCaption {
                Text(footerCaption)
                    .font(PanelTypography.metricLabel)
                    .foregroundStyle(.secondary)
            }
        }
    }

    /// The stems, with a floating detail label beside the pointer.
    private var barsRow: some View {
        let days = week.displayDays
        return DailyBarsRow(days: budgetDays, allowanceNoun: "weekly") { index in
            stem(days[index])
        } caption: { index in
            caption(days[index])
        }
    }

    private func isFuture(_ day: DailyUsageDay) -> Bool {
        day.dayStart > Calendar.current.startOfDay(for: Date())
    }

    private func stem(_ day: DailyUsageDay) -> some View {
        let fraction = Self.fillFraction(forDayUsage: day.totalPercent)
        let fillHeight = max(6, trackHeight * CGFloat(fraction))

        return ZStack(alignment: .bottom) {
            Color.primary.opacity(0.12)

            if fraction > 0 && !isFuture(day) {
                MonitorProvider.grok.accentColor
                    .frame(height: min(trackHeight, fillHeight))
            }
        }
    }

    private func caption(_ day: DailyUsageDay) -> some View {
        VStack(spacing: 1) {
            Text("\(max(0, Int(day.totalPercent.rounded())))%")
                .font(PanelTypography.microDigit)
                .foregroundStyle(.primary)
                .lineLimit(1)
                .minimumScaleFactor(0.65)
            Text(String(day.weekdaySymbol.prefix(2)))
                .font(PanelTypography.microDigit)
                .foregroundStyle(.secondary)
        }
        .opacity(isFuture(day) ? 0.5 : 1)
    }

    /// Maps weekly-pool percent into track height using the equal daily cap (`100/7`).
    static func fillFraction(forDayUsage percent: Double) -> Double {
        GrokDailyUsageBuilder.fillFraction(forDayUsage: percent)
    }
}

#if DEBUG
#Preview {
    DailyUsageChartView(week: GrokDailyUsageBuilder.preview(), periodUsedPercent: 12)
        .padding()
        .frame(width: 340)
}
#endif
