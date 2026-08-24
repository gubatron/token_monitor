import AppKit
import SwiftUI

struct MenuBarLabelView: View {
    let selectedProvider: MonitorProvider
    let showSelectedProvider: Bool
    let snapshot: WeeklyUsageSnapshot?
    let openCodeSnapshot: OpenCodeSnapshot?
    let cursorSnapshot: CursorSnapshot?
    let claudeSnapshot: ClaudeSnapshot?
    let chatGPTSnapshot: ChatGPTSnapshot?
    let openRouterSnapshot: OpenRouterSnapshot?
    let isGrokSignedIn: Bool
    let showGrokBar: Bool
    let showGrokCategories: Bool
    let showOpenCodeBar: Bool
    let showCursorBar: Bool
    let showClaudeBar: Bool
    let visibleProductIDs: Set<String>

    @Environment(\.colorScheme) private var colorScheme

    private var labelID: String {
        let products = visibleProductIDs.sorted().joined(separator: ",")
        let used = snapshot.map { Int($0.usedPercent.rounded()) } ?? -1
        let openCodeUsed = openCodeSnapshot.map { Int($0.primaryUsedPercent.rounded()) } ?? -1
        let cursorUsed = cursorSnapshot.map { Int($0.usedPercent.rounded()) } ?? -1
        let claudeUsed = claudeSnapshot.map { Int($0.headlineUsedPercent.rounded()) } ?? -1
        let chatGPTUsed = chatGPTSnapshot.map { Int($0.headlineUsedPercent.rounded()) } ?? -1
        let openRouterUsed = openRouterSnapshot?.usedPercent.map { Int($0.rounded()) } ?? -1
        let parts = [
            "\(selectedProvider)-\(showSelectedProvider)-\(showGrokBar)-\(showGrokCategories)-\(showOpenCodeBar)-\(showCursorBar)-\(showClaudeBar)",
            "\(products)-\(used)-\(openCodeUsed)-\(cursorUsed)-\(claudeUsed)-\(chatGPTUsed)-\(openRouterUsed)-\(isGrokSignedIn)-\(colorScheme)"
        ]
        return parts.joined(separator: "-")
    }

    var body: some View {
        Image(nsImage: MenuBarStatusRenderer.image(
            selectedProvider: selectedProvider,
            showSelectedProvider: showSelectedProvider,
            snapshot: snapshot,
            openCodeSnapshot: openCodeSnapshot,
            cursorSnapshot: cursorSnapshot,
            claudeSnapshot: claudeSnapshot,
            chatGPTSnapshot: chatGPTSnapshot,
            openRouterSnapshot: openRouterSnapshot,
            isGrokSignedIn: isGrokSignedIn,
            showGrokBar: showGrokBar,
            showGrokCategories: showGrokCategories,
            showOpenCodeBar: showOpenCodeBar,
            showCursorBar: showCursorBar,
            showClaudeBar: showClaudeBar,
            visibleProductIDs: visibleProductIDs
        ))
        .renderingMode(.original)
        .id(labelID)
    }
}
