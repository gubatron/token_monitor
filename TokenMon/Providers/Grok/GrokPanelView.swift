import SwiftUI

/// Panel section showing the SuperGrok weekly pool, categories, and daily chart.
struct GrokPanelView: View {
    let auth: GrokAuthSession
    let poller: GrokUsagePoller
    let settings: AppSettings
    let openSignIn: () -> Void

    /// 0 = current calendar week; negative = past weeks.
    @State private var weekOffset: Int = 0

    var body: some View {
        if auth.needsSignIn, auth.lastAuthError != nil {
            sessionExpiredHeader
        } else if auth.isSignedIn, let snapshot = poller.snapshot {
            usageHeader(snapshot)
        } else if auth.isSignedIn {
            VStack(alignment: .leading, spacing: 8) {
                grokTitle
                Text(poller.isRefreshing ? "Refreshing…" : (poller.lastError ?? "Signed in — waiting for usage data."))
                    .font(PanelTypography.body)
                    .foregroundStyle(.secondary)
                if auth.needsSignIn {
                    ProviderSignInButton(provider: .grok, title: "Sign In Again…", action: openSignIn)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            signedOutHeader
        }
    }

    private var grokTitle: some View {
        ProviderHeaderRow(provider: .grok, title: "SuperGrok")
    }

    private var sessionExpiredHeader: some View {
        VStack(alignment: .leading, spacing: 8) {
            grokTitle
            Text(poller.lastError ?? "Session expired. Sign in again to load usage.")
                .font(PanelTypography.body)
                .foregroundStyle(.secondary)
            ProviderSignInButton(provider: .grok, title: "Sign In Again…", action: openSignIn)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func usageHeader(_ snapshot: WeeklyUsageSnapshot) -> some View {
        // The breakdown splits the products the snapshot carries, limited to the
        // user's visible set; the weekly bar shows the headline used% as one fill.
        let breakdownProducts = ProductCatalog.breakdownProducts(
            snapshot.products,
            visible: settings.visibleProductIDs
        )
        let week = poller.week(weekOffset: weekOffset)
        VStack(alignment: .leading, spacing: 10) {
            grokTitle

            PanelCard {
                PanelSectionHeaderRow(
                    title: snapshot.usagePool.sectionTitle,
                    trailing: "\(Int(snapshot.usedPercent.rounded()))% Used"
                )
                SlimUsageTrack(
                    label: "Weekly",
                    percent: snapshot.usedPercent,
                    color: MonitorProvider.grok.accentColor,
                    resetsAt: snapshot.resetsAt,
                    showsLabel: false
                )
            }

            if !breakdownProducts.isEmpty {
                GrokUsageBreakdownCard(products: breakdownProducts)
            }

            if let credits = snapshot.extraCreditsBalance, credits > 0 {
                PanelCard {
                    HStack {
                        Text("Extra credits")
                        Spacer()
                        Text(credits as NSDecimalNumber, formatter: Format.usdCurrency)
                    }
                    .font(PanelTypography.body)
                    .foregroundStyle(.primary)
                }
            }

            // The chart renders only when a provider reset defines a billing window.
            if let week {
                PanelCard {
                    DailyUsageChartView(
                        week: week,
                        onPreviousWeek: { weekOffset = WeekOffset.previous(weekOffset) },
                        onNextWeek: { weekOffset = WeekOffset.next(weekOffset) },
                        canGoNext: WeekOffset.canGoNext(weekOffset),
                        periodUsedPercent: weekOffset == 0 ? snapshot.usedPercent : nil,
                        resetsAt: weekOffset == 0 ? snapshot.resetsAt : nil
                    )
                }
                // The bars' hover label can extend past the card; keep it
                // above the views that follow.
                .zIndex(1)
            }

            if let error = poller.lastError {
                StaleDataCaption(message: error, lastRefreshedAt: poller.lastRefreshedAt)
                    .padding(.top, 2)
            }

            ProviderSignOutButton(provider: .grok) {
                auth.signOut()
            }
        }
    }

    private var signedOutHeader: some View {
        VStack(alignment: .leading, spacing: 8) {
            grokTitle
            Text("Sign in to load your usage.")
                .font(PanelTypography.body)
                .foregroundStyle(.secondary)
            ProviderSignInButton(provider: .grok, title: "Sign In…", action: openSignIn)
                .keyboardShortcut("s", modifiers: [.command])
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
