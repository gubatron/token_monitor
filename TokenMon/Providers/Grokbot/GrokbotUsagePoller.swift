import Combine
import Foundation
import Observation
import os

/// Periodically refreshes the Grok Bot allowance from the shared Cursor session.
@MainActor
@Observable
final class GrokbotUsagePoller: ProviderUsagePoller {
    private(set) var snapshot: GrokbotSnapshot?
    private(set) var isRefreshing = false
    private(set) var lastError: String?
    private(set) var lastRefreshedAt: Date?
    /// Daily bars for the current allowance period, anchored to the provider's
    /// own `next_reset_timestamp_utc`: % of the pool burned per calendar day.
    private(set) var dailyBudgetDays: [DailyBudgetDay]?
    /// This period's usage by source; the last one fetched, kept when a later
    /// fetch fails.
    private(set) var breakdown: GrokbotUsageBreakdown?
    var menuIsOpen = false {
        didSet { if menuIsOpen != oldValue { pollingLoop.wake() } }
    }

    private let settings: AppSettings
    /// Grok Bot authenticates against the Cursor account, so this borrows the
    /// existing Cursor session and its cookie store.
    private let auth: CursorAuthSession
    private let hourly: HourlyDeltaActivityStore
    private let daily: DailyQuotaDeltaStore
    /// Injected fetch seam (tests supply a fake); defaults to the live client.
    private let fetchSnapshot: (String, String?) async throws -> GrokbotSnapshot
    private let fetchBreakdown: (String) async throws -> GrokbotUsageBreakdown?
    private let logger = Logger(category: "Grokbot")
    @ObservationIgnored private var cancellables = Set<AnyCancellable>()

    /// Last observed reset instant, so the chart stays anchored if a later
    /// payload omits it.
    private var weeklyResetsAt: Date?
    /// Instant the current bars were built against, so earlier weeks shift from
    /// the same window.
    private var budgetReferenceNow: Date?

    @ObservationIgnored private(set) lazy var pollingLoop = PollingLoop(
        interval: { [weak self] in self?.pollInterval() },
        refresh: { [weak self] in await self?.performRefresh() ?? .skipped }
    )

    init(
        settings: AppSettings,
        auth: CursorAuthSession,
        hourly: HourlyDeltaActivityStore,
        daily: DailyQuotaDeltaStore,
        fetchSnapshot: ((String, String?) async throws -> GrokbotSnapshot)? = nil,
        fetchBreakdown: ((String) async throws -> GrokbotUsageBreakdown?)? = nil
    ) {
        self.settings = settings
        self.auth = auth
        self.hourly = hourly
        self.daily = daily
        self.fetchSnapshot = fetchSnapshot ?? { cookieHeader, accountEmail in
            try await GrokbotUsageClient(cookieHeader: cookieHeader, accountEmail: accountEmail).fetchSnapshot()
        }
        // A test that fakes the snapshot gets no breakdown unless it supplies
        // one, so it never reaches the live endpoint.
        self.fetchBreakdown = fetchBreakdown ?? (fetchSnapshot != nil ? { _ in nil } : { cookieHeader in
            try await GrokbotUsageClient(cookieHeader: cookieHeader).fetchBreakdown()
        })
        // The session is shared with Cursor. Signing out from either surface
        // drops this snapshot; its history is kept for the same account and
        // cleared when a different account signs in.
        auth.signedOut
            .sink { [weak self] in self?.clearSnapshot() }
            .store(in: &cancellables)
        auth.accountReset
            .sink { [weak self] in
                self?.clearSnapshot()
                self?.clearAccountHistory()
            }
            .store(in: &cancellables)
    }

    var headlineUsedPercent: Double? { snapshot?.usedPercent }
    var headlineResetsAt: Date? { snapshot?.resetsAt }
    var alertAccount: String? { auth.accountEmail }

    func start() {
        pollingLoop.start()
    }

    func stop() {
        pollingLoop.stop()
    }

    func clearSnapshot() {
        snapshot = nil
        dailyBudgetDays = nil
        breakdown = nil
        weeklyResetsAt = nil
        budgetReferenceNow = nil
        lastError = nil
        lastRefreshedAt = nil
    }

    /// Clears the previous account's hourly and daily history.
    private func clearAccountHistory() {
        hourly.clear()
        daily.clear()
    }

    /// Fetches now when the provider is enabled and restarts the poll wait from here.
    func refreshNow() async {
        await pollingLoop.refreshNow()
    }

    private func performRefresh() async -> PollOutcome {
        guard settings.isProviderEnabled(.grokbot), !isRefreshing else { return .skipped }
        isRefreshing = true
        defer { isRefreshing = false }

        guard let cookieHeader = auth.cookieHeader(), !cookieHeader.isEmpty else {
            lastError = "Sign in to Cursor to load your Grokbot allowance."
            return .skipped
        }

        let outcome = await ProviderRefresh.run(
            auth: auth,
            logger: logger,
            label: "Grokbot",
            fetch: { try await fetchSnapshot(cookieHeader, auth.accountEmail) },
            publish: { fresh in
                snapshot = fresh
                lastError = nil
                lastRefreshedAt = Date()
                logger.info("Grokbot refresh: \(fresh.usedPercent, format: .fixed(precision: 1))% of weekly pool used")

                if let resetsAt = fresh.resetsAt {
                    weeklyResetsAt = resetsAt
                }
                // The daily store recognizes a normal rollover or a provider-initiated
                // early reset from the payload's own start / reset instants and keeps
                // the earlier days as history. Hourly deltas restart their baseline
                // either way; an early reset keeps the hours already recorded today.
                let transition = daily.record(
                    windowUsedPercent: fresh.usedPercent,
                    at: fresh.fetchedAt,
                    window: QuotaWindow(start: fresh.periodStart, resetsAt: fresh.resetsAt),
                    periodDays: fresh.daysInPeriod()
                )
                switch transition {
                case .none: break
                case .rollover: hourly.beginNewWindow()
                case .earlyReset: hourly.beginNewWindow(keepingHours: true)
                }
                hourly.record(usedPercent: fresh.usedPercent, at: fresh.fetchedAt)
                budgetReferenceNow = fresh.fetchedAt
                dailyBudgetDays = Self.buildDailyBudgetDays(
                    spentByDay: daily.spentByDay,
                    resetsAt: fresh.resetsAt ?? weeklyResetsAt,
                    daysInPeriod: fresh.daysInPeriod(),
                    windowStart: daily.windowStart,
                    interruptedWindowStart: daily.interruptedWindowStart,
                    now: fresh.fetchedAt
                )
                return .success
            },
            reportError: { lastError = $0 }
        )
        if outcome == .success {
            await refreshBreakdown(cookieHeader: cookieHeader)
        }
        return outcome
    }

    /// Fetches the usage breakdown after a successful snapshot.
    ///
    /// The breakdown only adds detail to the panel, so a failure keeps the
    /// last one, is logged, and neither reports an error nor counts against
    /// the shared Cursor session. A result that arrives after a sign-out or
    /// account switch is dropped.
    private func refreshBreakdown(cookieHeader: String) async {
        let generation = auth.sessionGeneration
        do {
            let fresh = try await fetchBreakdown(cookieHeader)
            guard !Task.isCancelled, auth.isCurrent(generation) else { return }
            if let fresh {
                breakdown = fresh
            }
        } catch {
            logger.info("Grokbot usage breakdown unavailable: \(error.localizedDescription, privacy: .public)")
        }
    }

    private func pollInterval() -> TimeInterval? {
        settings.pollInterval(for: .grokbot, menuIsOpen: menuIsOpen)
    }

    /// Daily bars for the current allowance period, anchored to the provider's
    /// actual reset instant. Returns `[]` when no reset has ever been observed.
    ///
    /// `daysInPeriod` comes from the payload's own
    /// `current_period_start` → `next_reset_timestamp_utc` span (see
    /// `GrokbotSnapshot.daysInPeriod`). `windowStart` / `interruptedWindowStart`
    /// anchor the bars to a window the provider began early and lead them with the
    /// preserved days before it (see `DailyBudget.buildWeeklyWindowDays`).
    static func buildDailyBudgetDays(
        spentByDay: [Date: Double],
        resetsAt: Date?,
        daysInPeriod: Int = 7,
        windowStart: Date? = nil,
        interruptedWindowStart: Date? = nil,
        weekOffset: Int = 0,
        now: Date = Date(),
        calendar: Calendar = .current
    ) -> [DailyBudgetDay] {
        guard let resetsAt else { return [] }
        return DailyBudget.buildWeeklyWindowDays(
            limitUSD: 100,
            daysInPeriod: daysInPeriod,
            resetsAt: resetsAt,
            spentByDay: spentByDay,
            windowStart: windowStart,
            interruptedWindowStart: interruptedWindowStart,
            weekOffset: weekOffset,
            now: now,
            calendar: calendar
        )
    }

    /// Bars for the allowance window `weekOffset` steps before the current one.
    ///
    /// Reads the live daily store so the panel can browse earlier weeks without
    /// a new poll. `0` matches ``dailyBudgetDays``.
    func dailyBudgetDays(weekOffset: Int) -> [DailyBudgetDay] {
        Self.buildDailyBudgetDays(
            spentByDay: daily.spentByDay,
            resetsAt: snapshot?.resetsAt ?? weeklyResetsAt,
            daysInPeriod: snapshot?.daysInPeriod() ?? 7,
            windowStart: daily.windowStart,
            interruptedWindowStart: daily.interruptedWindowStart,
            weekOffset: weekOffset,
            now: budgetReferenceNow ?? Date()
        )
    }
}
