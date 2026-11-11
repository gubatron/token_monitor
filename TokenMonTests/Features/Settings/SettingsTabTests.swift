@testable import TokenMon
import XCTest

/// Which tab Settings opens on, and the sign-in state that decides it. The
/// AppModel cases run under the test host, whose sessions use in-memory
/// credential stores.
@MainActor
final class SettingsTabTests: XCTestCase {
    func testOpensOnAccountsWhenNothingIsSignedIn() {
        XCTAssertEqual(SettingsTab.initial(isAnyProviderSignedIn: false), .accounts)
    }

    func testOpensOnGeneralWhenAProviderIsSignedIn() {
        XCTAssertEqual(SettingsTab.initial(isAnyProviderSignedIn: true), .general)
    }

    func testTabsKeepTheirDisplayOrder() {
        XCTAssertEqual(SettingsTab.allCases, [.general, .providers, .accounts, .data])
    }

    func testRawValueRoundTrips() {
        for tab in SettingsTab.allCases {
            XCTAssertEqual(SettingsTab(rawValue: tab.rawValue), tab)
        }
        XCTAssertNil(SettingsTab(rawValue: "nonexistent"))
    }

    func testFreshModelHasNoProviderSignedIn() {
        let model = AppModel()
        XCTAssertFalse(model.isAnyProviderSignedIn)
        XCTAssertEqual(SettingsTab.initial(isAnyProviderSignedIn: model.isAnyProviderSignedIn), .accounts)
    }

    func testCookieSessionCountsAsSignedIn() {
        let model = AppModel()
        model.claudeAuth.save(cookieHeader: "sessionKey=abc")
        XCTAssertTrue(model.isAnyProviderSignedIn)
        model.claudeAuth.signOut()
        XCTAssertFalse(model.isAnyProviderSignedIn)
    }

    func testOpenRouterKeyCountsAsSignedIn() {
        let model = AppModel()
        XCTAssertTrue(model.openRouterAuth.saveAPIKey("sk-or-v1-test"))
        XCTAssertTrue(model.isAnyProviderSignedIn)
        model.openRouterAuth.signOut()
        XCTAssertFalse(model.isAnyProviderSignedIn)
    }

    func testEveryCookieProviderCountsAsSignedIn() {
        let sessions: [(String, (AppModel) -> ProviderAuthSession)] = [
            ("grok", { $0.grokAuth }),
            ("claude", { $0.claudeAuth }),
            ("chatgpt", { $0.chatGPTAuth }),
            ("cursor", { $0.cursorAuth }),
            ("opencode", { $0.openCodeAuth })
        ]
        for (name, session) in sessions {
            let model = AppModel()
            session(model).save(cookieHeader: "session=abc")
            XCTAssertTrue(model.isAnyProviderSignedIn, name)
        }
    }
}
