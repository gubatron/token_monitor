import Foundation

/// Who or what spent a share of the Grok Bot allowance, as the Grok Bot app
/// labels it.
///
/// - Note: Raw values are stable; the wire uses `GROK_BOT_USAGE_SOURCE_<CASE>`.
enum GrokbotUsageSource: String, Codable, CaseIterable, Hashable, Sendable {
    case user
    case automation
    case agent
    case group
    case other

    /// Reads a protobuf JSON enum value (`GROK_BOT_USAGE_SOURCE_USER`, the bare
    /// `USER`, or its number). Unspecified and unknown values count as `other`.
    init(wireValue: String) {
        let name = wireValue.uppercased().replacingOccurrences(of: "GROK_BOT_USAGE_SOURCE_", with: "")
        switch name {
        case "USER", "1": self = .user
        case "AUTOMATION", "2": self = .automation
        case "AGENT", "3": self = .agent
        case "GROUP", "4": self = .group
        default: self = .other
        }
    }

    /// The Grok Bot app's label for the source.
    var displayName: String {
        switch self {
        case .user: return "You"
        case .automation: return "Routines"
        case .agent: return "Bot to Bot"
        case .group: return "Group chats"
        case .other: return "Other"
        }
    }

    /// Muted cool tones around the Grokbot slate accent, so the breakdown
    /// reads as part of the Grokbot panel. `user` is the accent itself; the
    /// others stay mid-tone so each shows against the empty track in both
    /// light and dark appearances.
    var sRGB: SRGB {
        switch self {
        case .user: return MonitorProvider.grokbot.accent
        case .automation: return SRGB(red: 0.40, green: 0.56, blue: 0.82)
        case .agent: return SRGB(red: 0.42, green: 0.68, blue: 0.70)
        case .group: return SRGB(red: 0.60, green: 0.52, blue: 0.78)
        case .other: return SRGB(red: 0.62, green: 0.62, blue: 0.64)
        }
    }
}

/// How the Grok Bot allowance was spent this period, by source, from
/// `aiserver.v1.DashboardService/GetGrokBotUsageBreakdown`.
struct GrokbotUsageBreakdown: Hashable, Sendable {
    /// What a source's value measures.
    enum Unit: Hashable, Sendable {
        /// Percent of the weekly allowance (`weekly_limit_ppm`).
        case weeklyLimitPercent
        /// Percent of the billing cycle's allowance (`billing_cycle_ppm`).
        case billingCyclePercent
        /// Billed US dollars (`billed_cents`), for usage billed on demand.
        case usd
    }

    /// One source's share of the period.
    struct Slice: Hashable, Sendable, Identifiable {
        var source: GrokbotUsageSource
        /// Percent of the allowance, or US dollars for ``Unit/usd``.
        var value: Double

        var id: GrokbotUsageSource { source }
    }

    var unit: Unit
    var periodStart: Date?
    var periodEnd: Date?
    /// Sources that used anything, most used first; ties keep the source order.
    var slices: [Slice]

    /// Sum of every source's value.
    var total: Double { slices.reduce(0) { $0 + $1.value } }

    /// True while there is usage to show for a period that has not ended.
    func isShown(at now: Date) -> Bool {
        !slices.isEmpty && (periodEnd.map { now < $0 } ?? true)
    }

    /// The response's units: the `breakdown` oneof holds exactly one of these.
    private static let unitKeys: [(Unit, [String], [String])] = [
        (.weeklyLimitPercent, ["weeklyLimitPpm", "weekly_limit_ppm"], ["ppm"]),
        (.billingCyclePercent, ["billingCyclePpm", "billing_cycle_ppm"], ["ppm"]),
        (.usd, ["billedCents", "billed_cents"], ["cents"])
    ]

    /// Parses the response body; nil when it carries no breakdown.
    ///
    /// Rows arrive per UTC day and source; they are summed over the period.
    /// Parts per million become percent and cents become dollars.
    static func parse(_ root: [String: Any]) -> GrokbotUsageBreakdown? {
        for (unit, keys, valueKeys) in unitKeys {
            guard let splits = keys.lazy.compactMap({ root[$0] as? [String: Any] }).first else { continue }
            let rows = (splits["bySource"] ?? splits["by_source"]) as? [[String: Any]] ?? []
            var totals: [GrokbotUsageSource: Double] = [:]
            for row in rows {
                let wire = JSON.string(row["source"]) ?? JSON.number(row["source"]).map { String(Int($0)) }
                let source = GrokbotUsageSource(wireValue: wire ?? "")
                let raw = JSON.firstDouble(row, keys: valueKeys) ?? 0
                totals[source, default: 0] += unit == .usd ? raw / 100 : raw / 10_000
            }
            return GrokbotUsageBreakdown(
                unit: unit,
                periodStart: date(splits, keys: ["periodStartMs", "period_start_ms"]),
                periodEnd: date(splits, keys: ["periodEndMs", "period_end_ms"]),
                slices: sortedSlices(totals)
            )
        }
        return nil
    }

    private static func sortedSlices(_ totals: [GrokbotUsageSource: Double]) -> [Slice] {
        GrokbotUsageSource.allCases
            .compactMap { source in totals[source].flatMap { $0 > 0 ? Slice(source: source, value: $0) : nil } }
            .enumerated()
            .sorted { lhs, rhs in
                lhs.element.value != rhs.element.value ? lhs.element.value > rhs.element.value : lhs.offset < rhs.offset
            }
            .map(\.element)
    }

    /// Epoch milliseconds (int64 arrives as a string in protobuf JSON).
    private static func date(_ dict: [String: Any], keys: [String]) -> Date? {
        JSON.firstDouble(dict, keys: keys).flatMap { $0 > 0 ? Date(timeIntervalSince1970: $0 / 1000) : nil }
    }
}
