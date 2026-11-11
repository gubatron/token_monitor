import AppKit
import SwiftUI

/// Quiet label-above-value cell for the 2×2 stats sheet.
struct MetricStat: View {
    let title: String
    let value: String
    var monospaced: Bool = true

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            PanelSectionHeader(title: title)
            Group {
                if monospaced {
                    Text(value).monospacedDigit()
                } else {
                    Text(value)
                }
            }
            .font(PanelTypography.bodyDigit)
            .foregroundStyle(.primary)
            .lineLimit(1)
            .minimumScaleFactor(0.55)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Unboxed 2×2 type sheet with a full-height / full-width hairline cross.
///
/// A sheet of figures counted on this computer carries a "This Mac" caption
/// under its cells, and VoiceOver reads each cell's title with "on this Mac".
struct MetricStatGrid: View {
    /// Where the figures come from; local ones are labelled.
    let source: UsageSource
    let stats: [MetricStat]

    init(source: UsageSource, _ stats: [MetricStat]) {
        self.source = source
        self.stats = stats
    }

    var body: some View {
        VStack(spacing: 0) {
            cells
            if source.tag != nil {
                separator.frame(height: 1)
                UsageSourceTag(source: source)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 7)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(Color(nsColor: .controlBackgroundColor))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .stroke(Color(nsColor: .separatorColor), lineWidth: 1)
        )
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private var cells: some View {
        let top = Array(stats.prefix(2))
        let bottom = Array(stats.dropFirst(2).prefix(2))
        return ZStack {
            VStack(spacing: 0) {
                row(top)
                row(bottom)
            }
            separator.frame(width: 1)
            separator.frame(height: 1)
        }
    }

    /// What VoiceOver reads for `stat`'s title: "Tokens this month on this
    /// Mac" for local figures, the plain title otherwise.
    func spokenTitle(of stat: MetricStat) -> String {
        source.qualifying(stat.title)
    }

    private var separator: some View {
        Rectangle().fill(Color(nsColor: .separatorColor))
    }

    private func row(_ cells: [MetricStat]) -> some View {
        HStack(spacing: 0) {
            ForEach(Array(cells.enumerated()), id: \.offset) { _, cell in
                cell
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(spokenTitle(of: cell))
                    .accessibilityValue(cell.value)
            }
        }
    }
}
