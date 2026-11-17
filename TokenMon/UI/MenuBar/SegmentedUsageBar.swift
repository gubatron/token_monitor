import SwiftUI

extension Color {
    static func product(_ token: ProductColor) -> Color {
        token.sRGB.color
    }

    static let usageRemainingTrack = Color.primary.opacity(0.12)
}

/// Pool bar: gapped rounded segments, then a muted track for what is left.
struct SegmentedUsageBar: View {
    /// One colored slice of the bar.
    struct Segment: Identifiable {
        var id: String
        /// Percent of the pool (0…100).
        var percent: Double
        var color: Color
    }

    let segments: [Segment]
    var height: CGFloat = 8

    private let segmentGap: CGFloat = 2
    private let minSegmentWidth: CGFloat = 3

    init(segments: [Segment], height: CGFloat = 8) {
        self.segments = segments
        self.height = height
    }

    /// SuperGrok products, each in its product color.
    init(products: [ProductUsage], height: CGFloat = 8) {
        self.init(
            segments: products.map {
                Segment(id: $0.id, percent: $0.percentOfPool, color: .product($0.colorToken))
            },
            height: height
        )
    }

    var body: some View {
        GeometryReader { geo in
            let visible = segments.filter { $0.percent > 0.05 }
            let used = min(100, visible.reduce(0.0) { $0 + max(0, $1.percent) })
            let remainder = max(0, 100 - used)
            let slotCount = visible.count + (remainder > 0.5 ? 1 : 0)
            let gapTotal = segmentGap * CGFloat(max(0, slotCount - 1))
            let usable = max(0, geo.size.width - gapTotal)

            let widths = Self.segmentWidths(
                percents: visible.map(\.percent) + (remainder > 0.5 ? [remainder] : []),
                usable: usable,
                minWidth: minSegmentWidth
            )

            HStack(spacing: segmentGap) {
                ForEach(Array(visible.enumerated()), id: \.element.id) { index, segment in
                    RoundedRectangle(cornerRadius: 2, style: .continuous)
                        .fill(segment.color)
                        .frame(width: widths[index], height: height)
                }
                if remainder > 0.5, let remainderWidth = widths.last {
                    RoundedRectangle(cornerRadius: 2, style: .continuous)
                        .fill(Color.usageRemainingTrack)
                        .frame(width: remainderWidth, height: height)
                }
            }
        }
        .frame(height: height)
    }

    /// Segment widths that sum to at most `usable`: proportional to `percents`,
    /// each floored at `minWidth`, then scaled down when the floors together
    /// overflow the track (several tiny slices would otherwise compress or spill
    /// past `usable`).
    static func segmentWidths(percents: [Double], usable: CGFloat, minWidth: CGFloat) -> [CGFloat] {
        let clamped = percents.map { max(minWidth, usable * CGFloat(max(0, $0) / 100)) }
        let total = clamped.reduce(0, +)
        guard usable > 0, total > usable else { return clamped }
        let scale = usable / total
        return clamped.map { $0 * scale }
    }
}
