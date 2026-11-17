import SwiftUI

/// Lays subviews out left to right at their natural size, wrapping onto a new
/// row when the next one would pass the available width.
struct FlowLayout: Layout {
    var horizontalSpacing: CGFloat = 14
    var verticalSpacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache _: inout ()) -> CGSize {
        let sizes = subviews.map { $0.sizeThatFits(.unspecified) }
        let rows = Self.rows(sizes: sizes, maxWidth: proposal.width ?? .infinity, spacing: horizontalSpacing)
        let width = rows.map { row in rowWidth(row, sizes: sizes) }.max() ?? 0
        let height = rows.map { row in rowHeight(row, sizes: sizes) }.reduce(0, +)
            + verticalSpacing * CGFloat(max(0, rows.count - 1))
        return CGSize(width: proposal.width ?? width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal _: ProposedViewSize, subviews: Subviews, cache _: inout ()) {
        let sizes = subviews.map { $0.sizeThatFits(.unspecified) }
        var y = bounds.minY
        for row in Self.rows(sizes: sizes, maxWidth: bounds.width, spacing: horizontalSpacing) {
            var x = bounds.minX
            let height = rowHeight(row, sizes: sizes)
            for index in row {
                subviews[index].place(
                    at: CGPoint(x: x, y: y + (height - sizes[index].height) / 2),
                    proposal: ProposedViewSize(sizes[index])
                )
                x += sizes[index].width + horizontalSpacing
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

    private func rowWidth(_ row: [Int], sizes: [CGSize]) -> CGFloat {
        row.map { sizes[$0].width }.reduce(0, +) + horizontalSpacing * CGFloat(max(0, row.count - 1))
    }

    private func rowHeight(_ row: [Int], sizes: [CGSize]) -> CGFloat {
        row.map { sizes[$0].height }.max() ?? 0
    }
}
