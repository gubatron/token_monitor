import SwiftUI

/// Lays subviews out left to right at their natural size, wrapping onto a new
/// row when the next one would pass the available width.
///
/// Rows are spread across the full width with equal gaps between subviews.
/// A wrapped last row keeps the minimum gap from the leading edge instead, so
/// a lone leftover subview is not pushed apart from the rest.
struct FlowLayout: Layout {
    /// Smallest gap between subviews on a row.
    var horizontalSpacing: CGFloat = 14
    var verticalSpacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache _: inout ()) -> CGSize {
        let sizes = subviews.map { $0.sizeThatFits(.unspecified) }
        let rows = Self.rows(sizes: sizes, maxWidth: proposal.width ?? .infinity, spacing: horizontalSpacing)
        let width = rows.map { rowWidth($0, sizes: sizes) }.max() ?? 0
        let height = rows.map { rowHeight($0, sizes: sizes) }.reduce(0, +)
            + verticalSpacing * CGFloat(max(0, rows.count - 1))
        return CGSize(width: proposal.width ?? width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal _: ProposedViewSize, subviews: Subviews, cache _: inout ()) {
        let sizes = subviews.map { $0.sizeThatFits(.unspecified) }
        let rows = Self.rows(sizes: sizes, maxWidth: bounds.width, spacing: horizontalSpacing)
        var y = bounds.minY
        for (rowIndex, row) in rows.enumerated() {
            let height = rowHeight(row, sizes: sizes)
            let xs = Self.xPositions(
                widths: row.map { sizes[$0].width },
                maxWidth: bounds.width,
                spacing: horizontalSpacing,
                spreads: rows.count == 1 || rowIndex < rows.count - 1
            )
            for (index, x) in zip(row, xs) {
                subviews[index].place(
                    at: CGPoint(x: bounds.minX + x, y: y + (height - sizes[index].height) / 2),
                    proposal: ProposedViewSize(sizes[index])
                )
            }
            y += height + verticalSpacing
        }
    }

    /// Subview indices per row: each row takes subviews until the next one
    /// would pass `maxWidth`. A subview wider than `maxWidth` gets a row alone.
    static func rows(sizes: [CGSize], maxWidth: CGFloat, spacing: CGFloat) -> [[Int]] {
        var rows: [[Int]] = []
        var current: [Int] = []
        var width: CGFloat = 0
        for (index, size) in sizes.enumerated() {
            let needed = current.isEmpty ? size.width : width + spacing + size.width
            if !current.isEmpty, needed > maxWidth {
                rows.append(current)
                current = [index]
                width = size.width
            } else {
                current.append(index)
                width = needed
            }
        }
        if !current.isEmpty {
            rows.append(current)
        }
        return rows
    }

    /// Leading x of each subview on a row. When `spreads` and the row has more
    /// than one subview, the gaps are equal and the row fills `maxWidth`, never
    /// closer than `spacing`; otherwise subviews sit `spacing` apart from 0.
    static func xPositions(widths: [CGFloat], maxWidth: CGFloat, spacing: CGFloat, spreads: Bool) -> [CGFloat] {
        let free = maxWidth - widths.reduce(0, +)
        let gap = spreads && widths.count > 1 && maxWidth.isFinite
            ? max(spacing, free / CGFloat(widths.count - 1))
            : spacing
        var x: CGFloat = 0
        return widths.map { width in
            defer { x += width + gap }
            return x
        }
    }

    /// Width of a row at the minimum gap.
    private func rowWidth(_ row: [Int], sizes: [CGSize]) -> CGFloat {
        let content: CGFloat = row.map { sizes[$0].width }.reduce(0, +)
        return content + horizontalSpacing * CGFloat(max(0, row.count - 1))
    }

    private func rowHeight(_ row: [Int], sizes: [CGSize]) -> CGFloat {
        row.map { sizes[$0].height }.max() ?? 0
    }
}
