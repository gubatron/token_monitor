import SwiftUI

/// Used-percent track: label left, percent right, 8px fill (matches Grok weekly bar).
///
/// Set `showsLabel` to `false` when the enclosing card's header already names
/// the pool and shows the percent in a pill, so the figure appears once.
struct SlimUsageTrack: View {
    let label: String
    let percent: Double
    var color: Color
    var resetsAt: Date?
    var caption: String?
    var showsLabel: Bool = true

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            if showsLabel {
                HStack(spacing: 8) {
                    Text(label)
                        .font(PanelTypography.bodyDigit)
                        .foregroundStyle(.primary)
                    Spacer()
                    Text("\(Int(Percent.clamp(percent).rounded()))% Used")
                        .font(PanelTypography.bodyDigit)
                        .foregroundStyle(.primary)
                }
            }

            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule()
                        .fill(Color.usageRemainingTrack)
                    Capsule()
                        .fill(color)
                        .frame(width: max(0, geo.size.width * CGFloat(Percent.clamp(percent) / 100)))
                }
            }
            .frame(height: 8)

            UsageLimitCaption(resetsAt: resetsAt, percent: percent, normalCaption: caption)
        }
    }
}

/// Refreshes exhausted-window countdowns without needing another provider poll.
struct UsageLimitCaption: View {
    let resetsAt: Date?
    let percent: Double
    var normalCaption: String?

    var body: some View {
        if percent >= 100, let resetsAt, resetsAt > Date() {
            TimelineView(.periodic(from: Date(), by: 60)) { context in
                caption(now: context.date)
            }
        } else {
            caption(now: Date())
        }
    }

    @ViewBuilder
    private func caption(now: Date) -> some View {
        if let text = Format.limitResetCaption(resetsAt, usedPercent: percent, now: now, normalCaption: normalCaption) {
            Text(text).resetCaption()
        }
    }
}
