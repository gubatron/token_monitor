import SQLite3
@testable import TokenMon
import XCTest

/// Week-arrow coverage for the OpenCode month budget.
final class OpenCodeMonthBudgetWeekTests: XCTestCase {
    private var dbURL: URL!

    override func setUpWithError() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("tokenmon-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        dbURL = dir.appendingPathComponent("opencode.db")

        var db: OpaquePointer?
        XCTAssertEqual(sqlite3_open(dbURL.path, &db), SQLITE_OK)
        defer { sqlite3_close(db) }

        let schema = """
        CREATE TABLE session (
            time_created INTEGER NOT NULL,
            cost REAL NOT NULL,
            tokens_input INTEGER NOT NULL,
            tokens_output INTEGER NOT NULL,
            tokens_cache_read INTEGER NOT NULL,
            tokens_cache_write INTEGER NOT NULL,
            time_archived INTEGER,
            model TEXT NOT NULL,
            id TEXT
        );
        CREATE TABLE message (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            data TEXT NOT NULL
        );
        """
        XCTAssertEqual(sqlite3_exec(db, schema, nil, nil, nil), SQLITE_OK)
    }

    override func tearDownWithError() throws {
        if let dbURL {
            try? FileManager.default.removeItem(at: dbURL.deletingLastPathComponent())
        }
        dbURL = nil
    }

    private func insertAssistantMessage(
        sessionID: String,
        timeCreated: Date,
        cost: Double,
        input: Int64,
        output: Int64 = 0,
        cacheRead: Int64 = 0,
        cacheWrite: Int64 = 0,
        providerID: String,
        modelID: String
    ) {
        var db: OpaquePointer?
        XCTAssertEqual(sqlite3_open(dbURL.path, &db), SQLITE_OK)
        defer { sqlite3_close(db) }

        let data = """
        {"role":"assistant","cost":\(cost),"providerID":"\(providerID)","modelID":"\(modelID)",\
        "tokens":{"input":\(input),"output":\(output),"cache":{"read":\(cacheRead),"write":\(cacheWrite)}}}
        """
        let sql = """
        INSERT INTO message (id, session_id, time_created, time_updated, data) \
        VALUES (?, ?, ?, ?, ?)
        """
        var stmt: OpaquePointer?
        XCTAssertEqual(sqlite3_prepare_v2(db, sql, -1, &stmt, nil), SQLITE_OK)
        defer { sqlite3_finalize(stmt) }

        let ms = Int64(timeCreated.timeIntervalSince1970 * 1000)
        sqlite3_bind_text(stmt, 1, "msg_\(UUID().uuidString)", -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
        sqlite3_bind_text(stmt, 2, sessionID, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
        sqlite3_bind_int64(stmt, 3, ms)
        sqlite3_bind_int64(stmt, 4, ms)
        sqlite3_bind_text(stmt, 5, data, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
        XCTAssertEqual(sqlite3_step(stmt), SQLITE_DONE)
    }

    private var utc: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC") ?? .gmt
        return calendar
    }

    private func goSpend(_ cost: Double, at date: Date) {
        insertAssistantMessage(
            sessionID: "ses_go",
            timeCreated: date,
            cost: cost,
            input: 1_000,
            providerID: "opencode-go",
            modelID: "m"
        )
    }

    /// Spend from the morning before a mid-day reset belongs to the old
    /// month: it must not shape the new month's first bar.
    func testMonthDailyBudgetSplitsTheResetDayAtTheResetInstant() throws {
        let calendar = utc
        func at(_ day: Int, _ hour: Int) -> Date {
            calendar.date(from: DateComponents(year: 2026, month: 11, day: day, hour: hour))!
        }
        goSpend(5, at: at(18, 9))
        goSpend(1, at: at(18, 15))
        goSpend(1, at: at(19, 10))

        let budget = try XCTUnwrap(OpenCodeLocalStats.monthDailyBudgetDays(
            limitUSD: 60,
            usedPercent: 10.0 / 3,
            periodResetsAt: calendar.date(from: DateComponents(year: 2026, month: 12, day: 18, hour: 14)),
            now: at(19, 12),
            dbURL: dbURL,
            calendar: calendar
        ))
        let spent = { (day: Int) in budget.days.first { calendar.isDate($0.date, inSameDayAs: at(day, 12)) }?.spentUSD ?? -1 }
        XCTAssertEqual(spent(18), 5.0 / 3, accuracy: 0.001)
        XCTAssertEqual(spent(19), 5.0 / 3, accuracy: 0.001)
    }

    /// Days of the current week before the billing period show the previous
    /// period's spend, flagged as the prior window, instead of 0%.
    func testMonthDailyBudgetShowsPreviousPeriodDaysOnTheCurrentWeek() throws {
        let calendar = utc
        func at(_ day: Int, _ hour: Int = 12) -> Date {
            calendar.date(from: DateComponents(year: 2026, month: 11, day: day, hour: hour))!
        }
        goSpend(3, at: at(16))
        goSpend(2, at: at(19))

        let budget = try XCTUnwrap(OpenCodeLocalStats.monthDailyBudgetDays(
            limitUSD: 60,
            usedPercent: 10.0 / 3,
            periodResetsAt: calendar.date(from: DateComponents(year: 2026, month: 12, day: 18, hour: 0)),
            now: at(19),
            dbURL: dbURL,
            calendar: calendar
        ))
        let monday = try XCTUnwrap(budget.days.first { calendar.isDate($0.date, inSameDayAs: at(16)) })
        XCTAssertEqual(monday.spentUSD, 5, accuracy: 0.001) // $3 of $60
        XCTAssertTrue(monday.isPriorWindow)
        let thursday = try XCTUnwrap(budget.days.first { calendar.isDate($0.date, inSameDayAs: at(19)) })
        XCTAssertEqual(thursday.spentUSD, 10.0 / 3, accuracy: 0.001)
        XCTAssertFalse(thursday.isPriorWindow)
    }

    /// Spend before the console window leaves the current week's scale undiluted,
    /// and chevron-left still paints that earlier week from local history.
    func testMonthDailyBudgetWeekOffsetKeepsHistoryOutOfHeadlineScale() throws {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC") ?? .gmt
        func day(_ month: Int, _ day: Int, hour: Int = 12) -> Date {
            calendar.date(from: DateComponents(year: 2026, month: month, day: day, hour: hour))!
        }

        // Aug 20 2026 is a Thursday, so the current Monday week starts Aug 17.
        // Jul 20 is four Mondays earlier and before the Aug 5 period start.
        insertAssistantMessage(
            sessionID: "ses_hist",
            timeCreated: day(7, 20),
            cost: 6,
            input: 1_000,
            providerID: "opencode-go",
            modelID: "m"
        )
        insertAssistantMessage(
            sessionID: "ses_hist",
            timeCreated: day(8, 18),
            cost: 4,
            input: 1_000,
            providerID: "opencode-go",
            modelID: "m"
        )

        let now = day(8, 20)
        let resetsAt = day(9, 5, hour: 0)
        let current = try XCTUnwrap(
            OpenCodeLocalStats.monthDailyBudgetDays(
                limitUSD: 60,
                usedPercent: 20,
                periodResetsAt: resetsAt,
                now: now,
                dbURL: dbURL,
                calendar: calendar
            )
        )
        XCTAssertEqual(current.days.map(\.spentUSD).reduce(0, +), 20, accuracy: 0.01)
        XCTAssertEqual(
            current.days.first { calendar.isDate($0.date, inSameDayAs: day(8, 18)) }?.spentUSD ?? 0,
            20,
            accuracy: 0.01
        )

        let previous = try XCTUnwrap(
            OpenCodeLocalStats.monthDailyBudgetDays(
                limitUSD: 60,
                usedPercent: 20,
                periodResetsAt: resetsAt,
                weekOffset: -4,
                now: now,
                dbURL: dbURL,
                calendar: calendar
            )
        )
        XCTAssertTrue(calendar.isDate(previous.days[0].date, inSameDayAs: day(7, 20)))
        XCTAssertEqual(previous.days[0].spentUSD, 10, accuracy: 0.01) // 6 / 60 * 100
        XCTAssertFalse(previous.days[0].isPriorWindow)
    }
}
