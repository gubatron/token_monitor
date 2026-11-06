# Spec 01: Core, App, Platform and Features (Settings, Update, Overview)

Source: TokenMon macOS (Swift), branch `windows-port`. This document covers
`TokenMon/App/`, `TokenMon/Core/` (Networking, Polling, Storage, Usage, Utilities),
`TokenMon/Platform/` (Auth, `SystemWakeGate`, `ThresholdNotifier`),
`TokenMon/Features/` (Settings, Update, Overview) and their tests under
`TokenMonTests/`. Provider-specific clients, pollers and UI panels live in other
spec slices; this one names them only where the shared code calls into them.

The Rust port should reproduce everything below exactly unless a row in section 8
says how a macOS dependency maps onto Windows.

Conventions used here:

* "Ref-seconds" means seconds since **2001-01-01T00:00:00Z** (Apple reference date,
  Unix `978_307_200`). This is how Swift's default `JSONEncoder` encodes `Date`.
  Every on-disk store in this slice that encodes a `Date` through `JSONEncoder` uses
  ref-seconds as a JSON number (a double). **Do not switch to Unix seconds.** The
  Windows build will not read a Mac file, but the tests below check numbers like
  `811144800`, and they only make sense as ref-seconds.
* "Unix-seconds" means `timeIntervalSince1970`. It is used only by
  `ThresholdNotifier`'s defaults keys.
* "Local calendar" means the Gregorian calendar in the **current system time zone,
  re-read live** (`Calendar.autoupdatingCurrent` / `.current`). On Windows, use the
  current zone from `iana-time-zone`/`GetDynamicTimeZoneInformation` and re-read it
  when the zone changes (section 8).
* `MonitorProvider` raw values: `overview`, `grok`, `cursor`, `opencode`, `claude`,
  `chatgpt`, `openrouter`, `grokbot` (declared in that order, which is `allCases`
  order). `usageProviders` = `[grok, cursor, opencode, claude, chatgpt, openrouter, grokbot]`.
  `menuBarGraphProviders` = `[grok, grokbot, claude, cursor, opencode]`.
  Display names: Overview, Grok, Cursor, OpenCode, Claude, ChatGPT, OpenRouter, Grokbot.
  `signInProvider`: grokbot → cursor, every other → itself.
  `polls(p)`: `self == p || self == overview`.
  `normalizedOrder(raw)`: drop `overview`, unknowns and duplicates (keep the first
  occurrence), then append any missing usage providers in `usageProviders` order.

---

## 1. Modules

### 1.1 Core/Networking

#### 1.1.1 `ProviderURLSession` (HTTP client for every provider request)

* Ephemeral session: **no cookie jar** (`httpCookieStorage = nil`,
  `httpShouldSetCookies = false`, accept policy `never`), **no URL cache**
  (`urlCache = nil`, cache policy `reloadIgnoringLocalCacheData`).
* Credentials go only in explicit `Cookie:` / `Authorization:` headers. Any
  `Set-Cookie` from a provider is never stored automatically. The only exception is
  the explicit `applyRefreshedCookies` call (see 1.6.4).
* Rust: one `reqwest::Client` with `cookie_store(false)`, no cache layer, redirects
  at the default setting (Foundation follows them). Give it a sensible connect
  timeout; Foundation's default request timeout is 60 s.

#### 1.1.2 `AuthenticatedRequest`

`applyHeaders(cookieHeader, bearerToken, referer)`:

| Header | Value | When |
|---|---|---|
| `Cookie` | `cookieHeader` verbatim | non-nil and non-empty |
| `Authorization` | `Bearer <token>` | non-nil and non-empty |
| `Accept` | `application/json` | always |
| `User-Agent` | `TokenMon/<CFBundleShortVersionString>`, or `TokenMon/unknown` | always |
| `Referer` | `referer` | non-nil and non-empty |

`mapError(response, data)` returns a `UsageError?`. It never reads the body, so
the body never leaks into a message:

| Status | Result |
|---|---|
| 401 or 403 | `.unauthorized` |
| 429 | `.rateLimited(retryAfter: retryAfter(from: response))` |
| not in 200..<300 | `.badResponse("HTTP <status>")` |
| 2xx | `nil` |

`responseError(response, data)`: when `ProviderHTTP.isBotChallenge` is true it
returns `.badResponse("Request blocked by a bot-protection check (HTTP <status>). Retrying.")`
whatever the status. Otherwise it returns `mapError`.

`retryAfter(from:now:)` reads the `Retry-After` header:

1. Trim spaces. Missing or empty gives `nil`.
2. If the value parses as a double (Swift `Double(String)`, which accepts `inf`,
   `infinity`, `nan`, exponents and a leading `+`/`-`): NaN gives `nil`; otherwise
   return `clamp(value, 0, maxRetryAfter)`.
3. Otherwise parse an HTTP date in the format `EEE, dd MMM yyyy HH:mm:ss zzz`
   (locale `en_US_POSIX`, zone GMT), then return `clamp(date - now, 0, maxRetryAfter)`.
   If that fails, return `nil`.
4. `maxRetryAfter = 3600` seconds.

`perform` / `performWithResponse(request, session, forbidden?, map)`:

1. Send the request. A cancelled request (the task is cancelled, or the URL error
   is `cancelled`) is thrown as `CancellationError`. Pollers treat that as
   **skipped**, not a failure.
2. Any other transport error becomes `map(.network(error.localizedDescription))`.
3. A non-HTTP response becomes `map(.badResponse("Non-HTTP response"))`.
4. If `forbidden` is set and the status is 403 and the response is **not** a bot
   challenge, throw `forbidden`. This is for endpoints whose 403 refuses one
   feature while the session stays valid (Grokbot).
5. If `responseError` returns an error, throw `map(error)`.
6. Return `(data, httpResponse)`.

#### 1.1.3 `ProviderHTTP`

* `get` / `getWithResponse` / `post(json:)` resolve the path, apply the headers
  above, add any extra `headers` (they override earlier values), add
  `Origin: <origin>` when given, and for POST with JSON set
  `Content-Type: application/json` with a JSONSerialization body.
  `forbidden = context.forbiddenMessage.map { .custom(message:$0, usage:.badResponse($0)) }`.
  Errors are mapped with `ProviderError(usageError, context)`.
* `resolve(path, baseURL)`: append `/` to the base if it is missing, then resolve
  `path` against it as a relative URL (RFC 3986). Examples:
  `https://openrouter.ai/api/v1` + `key` gives `…/api/v1/key`; a root-relative
  `/api/x` replaces the path; an absolute URL is used as-is. An unresolvable path
  throws `badResponse(context, "Invalid path <path>")`.
* `setCookieHeaders(response)`: every header value whose name equals `set-cookie`,
  case-insensitively. Foundation folds repeated headers into one comma-joined value.
  In Rust, collect every `Set-Cookie` header separately; the cookie parser must
  still accept comma-folded values (see 1.6.4).
* `jsonObject(data, context)`: parse a JSON **object**. On failure:
  * the body looks like HTML and like a login page → `ProviderError.unauthorized(context)`
  * the body looks like HTML otherwise → `badResponse(context, "Unexpected HTML response")`
  * anything else → `badResponse(context, "Malformed response body")`
* `looksLikeHTML(data)`: decode the first 512 bytes as UTF-8. If that fails, the
  answer is false. Otherwise the answer is whether the text, with leading
  whitespace and newlines trimmed, starts with `<`.
* `looksLikeLoginPage(data)`: requires `looksLikeHTML`. Take the first **65 536**
  bytes, decode as Latin-1 and lowercase. If any **challenge marker** appears,
  return false. Otherwise return true if any **login marker** appears.
  * Challenge markers: `cf-chl`, `challenge-platform`, `cf_chl_opt`,
    `just a moment...`, `attention required! | cloudflare`, `cf-browser-verification`.
  * Login markers: `type="password"`, `type='password'`, `sign in`, `sign-in`,
    `signin`, `log in`, `log-in`, `login`, `authkit`, `workos`.
* `isBotChallenge(response, data)`: true if the response has a `cf-mitigated`
  header (any value), or if the status is 403 **and** `looksLikeHTML(data)`.
* `isUnauthorizedMessage(msg)`: the lowercased message contains `not_authenticated`
  or `unauthor`.

#### 1.1.4 Error types

`UsageError` cases and their `errorDescription`:

| Case | Message |
|---|---|
| `notSignedIn` | "Sign in to load usage." |
| `unauthorized` | "Session expired. Sign in again." |
| `network(m)` | "Network error: m" |
| `badResponse(m)` | "Response error: m" |
| `rateLimited(retryAfter: Double?)` | "Rate limited. Retrying later." |

`retryAfter` is non-nil only for `.rateLimited`. The type is `Equatable`.
`ProviderUsageError` is any error that exposes `.usageError`.

`ProviderErrorContext` has `displayName`, `notSignedInMessage`,
`unauthorizedMessage` and an optional `forbiddenMessage`:

| Context | displayName | notSignedIn | unauthorized | forbidden |
|---|---|---|---|---|
| claude | Claude | Sign in to Claude to load usage. | Claude session expired. Sign in again. | – |
| chatGPT | ChatGPT | Sign in to ChatGPT to load usage. | ChatGPT session expired. Sign in again. | – |
| cursor | Cursor | Sign in to Cursor to load usage. | Cursor session expired. Sign in again. | – |
| grok | Grok | Sign in to grok.com to load usage. | Session expired. Please sign in again. | – |
| grokbot | Grokbot | Sign in to Cursor to load your Grokbot allowance. | Cursor session expired. Sign in again. | Grok Bot is not available on this Cursor account. |
| openCode | OpenCode console | Sign in to the OpenCode console to load official Go usage. | OpenCode console session expired. Sign in again. | – |
| openRouter | OpenRouter | Add an OpenRouter API key to load usage. | OpenRouter rejected the API key. Check or replace it. | – |

`ProviderError` cases are `notSignedIn(ctx)`, `unauthorized(ctx)`,
`badResponse(ctx, m)`, `network(ctx, m)` and `custom(message, usage)`.

* `init(usageError, context)`: `rateLimited` becomes
  `.custom(message: "<displayName> is rate limiting requests. Retrying later.", usage: usageError)`.
  It keeps the original `UsageError`, so `retryAfter` survives. Every other case
  maps one-to-one.
* Descriptions: `notSignedIn` and `unauthorized` use the context strings;
  `badResponse` gives `"<displayName> response error: m"`; `network` gives
  `"<displayName> network error: m"`; `custom` gives its message.

#### 1.1.5 `Domain.matches(domain, hosts)`

Lowercase the domain and trim leading and trailing `.`. It matches when it equals a
host or ends with `.<host>`.

#### 1.1.6 `URL(staticString:)`

Parses a compile-time literal. A malformed literal triggers a debug assertion; in
release it becomes `file:///dev/null`. In Rust, a `const`/`Lazy` `Url` that panics
in tests is fine.

### 1.2 Core/Polling

#### 1.2.1 `BackoffTimer`

* `current` starts at **0**.
* `recordFailure()`: if `current == 0`, set it to `initial`; otherwise
  `current = min(current * 2, maximum)`.
* `reset()` sets `current = 0`.
* `PollingLoop` uses `initial = 30`, `maximum = 600` (30 s → 60 → 120 → 240 → 480 → 600).

#### 1.2.2 `PollOutcome`

The cases are `success`, `failure(retryAfter: Double?)` and `skipped`.

`init(error:)`: a `CancellationError` gives `.skipped`. Anything else gives
`.failure(retryAfter: (error as ProviderUsageError)?.usageError.retryAfter)`.

#### 1.2.3 `PollingLoop`

The loop runs on the main thread. It is injected with:

* `interval: () -> Double?`: nil (or ≤ 0) **parks** the loop.
* `backoff`: `BackoffTimer(30, 600)`.
* `now`: a clock.
* `sleep(seconds)`: the real sleeper clamps to `[0, 86_400]` seconds and throws when
  cancelled.
* `jitter: () -> Double`: defaults to uniform `[-0.1, 0.1]`.
* `refresh: async -> PollOutcome`.

A convenience initializer wraps a `Void` refresh that always reports `.success`
(the update checker uses it).

State: `waitTask`, `isStarted`, `isPaused`, `isRefreshing`, `lastRefreshAt`,
`backoff`, `retryAfterUntil`, `jitterFactor` (starts at 1).

| Method | Behaviour |
|---|---|
| `start()` | `isStarted = true`, `lastRefreshAt = nil`, `schedule()`. A non-nil interval refreshes **immediately**; a nil interval parks with no timer. |
| `stop()` | `isStarted = false`, cancel the wait. |
| `pause()` | `isPaused = true`, cancel the wait. `lastRefreshAt` is kept. |
| `resume()` | `isPaused = false`, `schedule()`. Refreshes at once when one is overdue. |
| `wake()` | No-op while refreshing; otherwise `schedule()`. Re-arms from the last refresh. |
| `refreshNow()` | No-op while refreshing; otherwise cancel the wait and `runRefresh()`. Runs even when not started, but `schedule()` afterwards does nothing unless started. |
| `refreshNow(ifOlderThan age)` | Return if `lastRefreshAt` exists and `now - lastRefreshAt < age`; otherwise `refreshNow()`. |
| `delayUntilDue()` | See below. |
| deinit | Cancel the wait. The wait task must hold only a **weak** reference to the loop. |

`delayUntilDue()`:

```
base = interval(); if base == nil || base <= 0 → return nil (park)
if lastRefreshAt == nil → return 0
wait = base; if backoff.current > 0 { wait = max(wait, backoff.current) }
due = lastRefreshAt + wait * jitterFactor
if retryAfterUntil != nil && retryAfterUntil > due { due = retryAfterUntil }
return max(0, due - now())
```

`schedule()`:

```
cancelWait()
guard isStarted && !isPaused, let delay = delayUntilDue() else return
waitTask = spawn { if delay > 0 { sleep(delay) or return on cancel }; if cancelled return; runRefresh() }
```

`runRefresh()`:

```
guard !isRefreshing; isRefreshing = true; outcome = await refresh(); isRefreshing = false
record(outcome)
schedule()
```

`record(outcome)`:

```
finishedAt = now(); lastRefreshAt = finishedAt
jitterFactor = 1 + clamp(jitter(), -0.1, 0.1)     // drawn after EVERY refresh
success:  backoff.reset(); retryAfterUntil = nil
failure(r): backoff.recordFailure(); retryAfterUntil = r.map { finishedAt + r }
skipped:  nothing else changes
```

Consequences that the tests check (section 7):

* The `Retry-After` deadline is **not** jittered and overrides a shorter wait.
* Backoff only lengthens the wait (`max(base, backoff)`).
* A skipped refresh still restarts the interval from that moment.

#### 1.2.4 `ProviderUsagePoller` protocol

Every provider poller implements:

* `menuIsOpen` (read/write). Pollers call `pollingLoop.wake()` in `didSet` when the
  value changes.
* `lastRefreshedAt`, `lastError`, `headlineUsedPercent: Double?`,
  `headlineResetsAt: Date?`, `alertAccount: String?`.
* `pollingLoop`.
* `start()`, `stop()`, `refreshNow()` (fetch now when enabled and restart the wait),
  and `clearSnapshot()` (drop the displayed snapshot but keep history).

A poller's loop uses `interval = settings.pollInterval(for: provider, menuIsOpen: menuIsOpen)`.

#### 1.2.5 `ProviderRefresh.run`

This is the fetch, stale-session and error step that every poller shares.

`PollingAuthSession`: `sessionGeneration`, `isCurrent(gen)`,
`recordAuthFailure(reason) -> Bool`, `recordAuthSuccess()`.

```
generation = auth.sessionGeneration
do {
  value = try await fetch()
  if task cancelled || !auth.isCurrent(generation) → return .skipped  (no publish, no success)
  auth.recordAuthSuccess(); return publish(value)
} catch CancellationError → .skipped
catch error {
  if !auth.isCurrent(generation) → return .skipped   (stale failure: no error reported, no streak)
  if error is ProviderError && usageError ∈ {unauthorized, notSignedIn} {
     onRejected(); auth.recordAuthFailure(reason: error.localizedDescription)
  }
  reportError(error.localizedDescription); log "<label> refresh failed: <desc>"
  return PollOutcome(error: error)
}
```

### 1.3 Core/Storage

#### 1.3.1 `AppSupport`

* `directoryName = "TokenMon"`, `legacyDirectoryName = "ModelMonitor"`.
* `isRunningTests`: the `XCTestConfigurationFilePath` environment variable is set.
  In Rust, use `cfg(test)` or an injected base directory.
* `baseDirectory`: the user's Application Support folder
  (`~/Library/Application Support`). Under tests it is
  `<tmp>/TokenMonTests-<UUID>/`.
* `directory(subdirectory = "TokenMon")`:
  1. If `subdirectory == "TokenMon"`, call `migrateLegacyDirectoryIfNeeded(base/ModelMonitor → base/TokenMon)`.
     This happens lazily on first use, which in practice is launch.
  2. Create the directory, with intermediate directories, mode **0700**.
  3. Re-apply mode 0700.
  4. Return it.
* `migrateLegacyDirectoryIfNeeded(legacy, current)`: move legacy to current only
  when legacy exists and current does **not**. Never merge or overwrite.
* `realHomeDirectory`: `getpwuid_r(getuid())` home, falling back to
  `NSHomeDirectory()`. It is used by the OpenCode local DB reader.

#### 1.3.2 `FileBackedStringStore`

These are atomic per-key text files.

* File name: `<directory>/<filenamePrefix><key>.dat`. The default prefix is `auth_`.
* `value(forKey)`: the UTF-8 contents. A missing file gives nil silently. Other
  errors are logged and give nil.
* `set(value, forKey) -> Bool`:
  1. Create the staging file `<dir>/.<prefix><key>.<UUID>.tmp` with mode **0600**.
     If that fails, log and return false.
  2. `rename(2)` it onto the target, which is atomic and keeps 0600. If that fails,
     delete the staging file, log `strerror` and return false.
  3. Afterwards the directory contains **only** the target file (no stray staging
     files).
* `remove(forKey)`: delete the file. Missing is fine; other errors are logged.
* A write into a directory that does not exist returns `false`.

Files written by this slice. All live under `%TokenMonDir%`, which is
`~/Library/Application Support/TokenMon` on macOS:

| File | Writer | Content |
|---|---|---|
| `activity_grok_hourly_today.dat` | `HourlyDeltaActivityStore(storageKey:"grok_hourly_today")` | Hourly JSON (1.3.4) |
| `activity_claude_hourly_today.dat` | Hourly (`claude_hourly_today`) | Hourly JSON |
| `activity_grokbot_hourly_today.dat` | Hourly (`grokbot_hourly_today`) | Hourly JSON |
| `activity_claude_daily_usage.dat` | `DailyQuotaDeltaStore(storageKey:"claude_daily_usage")` | Daily JSON (1.3.5) |
| `activity_cursor_daily_usage.dat` | Daily (`cursor_daily_usage`) | Daily JSON |
| `activity_grokbot_daily_usage.dat` | Daily (`grokbot_daily_usage`) | Daily JSON |
| `activity_overview_hourly_history.dat` | `OverviewHourlyHistoryStore` | Overview JSON (section 4) |
| `<prefix>email.dat`, `<prefix>account.dat`, `opencode_auth_workspace.dat` | credential stores (1.6) | plain text |
| `<prefix>session.dat`, `openrouter_auth_key.dat` | **only** as a fallback when the secure store fails (1.6.2) | plain text secret |
| `grok_history.dat`, `history.store` (legacy) | GrokHistoryStore (other slice) | – |

Credential prefixes: `auth_` (Grok), `cursor_auth_` (Cursor, also used by
Grokbot), `claude_auth_`, `chatgpt_auth_`, `opencode_auth_`, `openrouter_auth_`.

#### 1.3.3 `LegacyCacheCleanup` (macOS only)

This one-time cleanup runs at launch, but not under tests. It has two independent
steps, each guarded by its own defaults flag:

* `legacyURLCacheRemoved`: clear `URLCache.shared`, then delete `Cache.db`,
  `Cache.db-shm`, `Cache.db-wal` and `fsCachedData/` inside
  `~/Library/Caches/<bundleId>/`. Nothing else in that folder is touched, nor are
  sibling folders. Set the flag even if the folder is missing.
* `legacyCookieJarRemoved`: call `HTTPCookieStorage.shared.removeCookies(since: .distantPast)`.
  In `~/Library/Cookies/` and `~/Library/HTTPStorages/`, delete only files named
  exactly `<bundleId>.binarycookies`, or starting with `<bundleId>.binarycookies_tmp_`
  and ending with `.dat`. Subfolders are left alone, and so is `.binarycookies.bak`.
* The function returns true when at least one step ran. It returns false without
  doing anything if the bundle id is empty.

Windows: there is no legacy data. Keep the function as a no-op, or drop it (see 8).
The tests in 7 are macOS-only.

#### 1.3.4 `HourlyDeltaActivityStore`

This store tracks one provider's **percentage-point growth per local hour today**.
It persists through `FileBackedStringStore(prefix "activity_")`.

Published state:

* `hourWeights: [Double; 24]`
* `dayStart: Date` (local midnight)

Private state: `lastUsedPercent: Double?` (the baseline), `dayKey: String`, and the
`persisted` payload last read or written.

On-disk JSON (Swift `JSONEncoder`; nil optionals are **omitted**):

```json
{"dayKey":"2026-10-10","dayStart":813304800,"hourWeights":[0,0,...24 numbers],"lastUsedPercent":30}
```

`dayStart` is ref-seconds. Old payloads may lack `dayKey`.

Constants: `Percent.noiseFloor = 0.05`, `Percent.resetDropFloor = 5`.

`init(store, storageKey, calendar = local, now)`:

1. Set `dayStart = startOfDay(now)`, `dayKey = DayKey(now)` and 24 zeros.
2. `loadOrReset(now)`. If the raw value is missing or does not decode, `persist()`
   (writes an empty day) and return. Otherwise set `persisted = payload` and
   compute `storedKey = payload.dayKey ?? DayKey.key(forStoredStartOfDay: payload.dayStart, preferredOffset: tzOffset(at: payload.dayStart))`.
   If `storedKey == dayKey` and there are 24 weights, restore the weights and
   `lastUsedPercent`. Otherwise use zeros and `lastUsedPercent = nil`. Then
   `persist()`, which rewrites a legacy payload once with `dayKey`.
3. Observe system time-zone changes. On a change, re-derive
   `dayStart = DayKey.startOfDay(dayKey)` in the new zone, if it differs.

`record(usedPercent, at: date)`:

```
key = DayKey(date)
if key != dayKey {
   yesterdayKey = DayKey(date - 1 calendar day)
   if yesterdayKey != dayKey { lastUsedPercent = nil }    // gap > 1 day: no baseline
   dayKey = key; dayStart = startOfDay(date); hourWeights = 24 zeros
}
defer { lastUsedPercent = usedPercent; persist() }
guard let previous = lastUsedPercent else return          // first sample only sets baseline
delta = used >= previous ? used - previous
      : (previous - used >= 5) ? used                     // window reset: credit post-reset value
      : 0                                                 // small downward tick = noise
guard delta >= 0.05 else return
hour = calendar.hour(date); hourWeights[hour] += delta
```

Midnight: when the previous sample was yesterday, the baseline carries over, so
growth across midnight is credited to the new day's first sampled hour.

Other operations:

* `clear(at: date = now)`: set `dayKey`/`dayStart` from `date`, zero the weights,
  `lastUsedPercent = nil`, `persist()`. Used on an account switch.
* `beginNewWindow(keepingHours = false)`: unless `keepingHours`, zero the weights.
  Set `lastUsedPercent = 0` so the first sample of the new window is credited whole,
  then `persist()`. Pass `keepingHours: true` after an early reset.
* `persist()`: build the payload. **Skip the write if it equals `persisted`.**
  Otherwise encode, write and update `persisted`.

#### 1.3.5 `QuotaWindow`, `QuotaWindowTransition` and `DailyQuotaDeltaStore`

`QuotaWindow { start: Date?, resetsAt: Date? }` holds the bounds the provider
reports. Grokbot and Cursor send both. Claude sends only `resets_at`.

`QuotaWindowTransition` is one of `none`, `rollover` or `earlyReset`. Constants:

* `skew = 300` seconds
* `movedThreshold = 3600` seconds

```
isRollover(prevReset?, nextReset?, periodDays):
  both non-nil && next > prev && (next - prev) >= max(1,periodDays) * 86400 * 0.5

classify(from previous, previousUsedPercent?, to next, usedPercent, periodDays, now):
  guard previous.resetsAt && next.resetsAt else → none
  oldStillRunning = previous.resetsAt > now + 300
  if !oldStillRunning → isRollover(...) ? rollover : none
  guard previousUsed != nil && previousUsed - usedPercent >= 5 else → none
  startMoved = both starts present && (next.start - previous.start) >= 3600   (signed: forward only)
  resetMoved = |next.resetsAt - previous.resetsAt| >= 3600
  → (startMoved || resetMoved) ? earlyReset : none
```

`DailyQuotaDeltaStore` accumulates percentage points **per local calendar day**.
It uses the `activity_` prefix.

* `spentByDay: [Date(localStartOfDay): Double]` is derived from
  `dayTotals: [DayKey: Double]`. It is re-derived whenever `dayTotals` changes and
  whenever the system time zone changes. Several keys landing on one start-of-day
  are summed.
* `retentionDays = 40`.
* Window state: `windowStart`, `interruptedWindowStart`, `observedStart`,
  `windowResetsAt`, `lastUsedPercent`.

On-disk JSON. Both day shapes are written; nil fields are omitted:

```json
{"dayTotals":{"2026-09-14":7,"2026-09-15":4},
 "days":[811058400,7,811144800,4],
 "lastUsedPercent":35,
 "windowStart":813093922.556,"interruptedWindowStart":...,
 "observedStart":813093922.556,"windowResetsAt":813698722.556}
```

* `days` is a Swift `[Date: Double]`, encoded as a **flat array alternating
  `[ref-seconds, value, …]`**. It holds the same totals keyed by the local
  start-of-day instant. On load, `days` is read only when `dayTotals` is missing.
* Every date field is ref-seconds.

`init(store, storageKey, calendar = local, now = Date.init)` calls `load()`:

1. Missing or undecodable data: `persist()` and return.
2. `days = payload.dayTotals ?? convertLegacyDays(payload.days)`. Legacy conversion
   is `DayKey.key(forStoredStartOfDay: instant, preferredOffset: tz.secondsFromGMT(instant))`,
   summing collisions.
3. `prune(days, now())`, restore every field, then `persist()` (rewrites the
   legacy format once).
4. Observe time-zone changes and republish `spentByDay`.

`record(windowUsedPercent, at date, window?, periodDays = 7, calendar?) -> transition`.
`persist()` always runs at the end (deferred):

```
transition = window != nil ? classify(from: QuotaWindow(observedStart, windowResetsAt),
                                      previousUsedPercent: lastUsedPercent, to: window,
                                      usedPercent, periodDays, now: date) : none
if window && transition != none → applyTransition(...)
if window.start   → observedStart = window.start
if window.resetsAt → windowResetsAt = window.resetsAt
guard previous = lastUsedPercent else { lastUsedPercent = used; return transition }
delta = (same rule as hourly: grow / ≥5 drop credits `used` / else 0)
lastUsedPercent = used
guard delta >= 0.05 else return transition
dayTotals[DayKey(date)] += delta; prune(dayTotals, now: date)
return transition
```

`applyTransition(t, window, periodDays, date)`:

```
impliedStart(r) = r - max(1,periodDays) calendar days
oldWindowEnd   = t == rollover ? windowResetsAt(old) : nil
candidate      = window.start ?? oldWindowEnd ?? impliedStart(window.resetsAt)
interrupted    = t == earlyReset ? (observedStart(old) ?? impliedStart(windowResetsAt(old))) : nil
beginNewWindow(startingAt: min(candidate ?? date, date), interruptedWindowStart: interrupted)
```

`beginNewWindow(startingAt ws = now, interruptedWindowStart?, calendar?)`:

* Keep only `dayTotals` entries whose key is **strictly less than**
  `DayKey(ws)`. This drops the boundary day and later days.
* `lastUsedPercent = 0`, `windowStart = ws`,
  `interruptedWindowStart = interrupted`, then `persist()`.

`clear()`: nil every window field and the baseline, empty `dayTotals`, `persist()`.

`prune(days, now)`: `cutoff = DayKey.adding(-40, to: DayKey(now))`; keep keys
`>= cutoff` (string comparison, which sorts chronologically).

`persist()`: skip the write when the payload equals the last persisted payload.

Callers that see `.rollover` or `.earlyReset` reset their own per-window state, for
example hourly `beginNewWindow(keepingHours: transition == .earlyReset)`. That
logic lives in the provider pollers.

### 1.4 Core/Usage

#### 1.4.1 `Percent`

* `noiseFloor = 0.05`, `resetDropFloor = 5`.
* `clamp(v) = min(100, max(0, v))`.

#### 1.4.2 `DayKey`

A `yyyy-MM-dd` string that is independent of time zone. Keys sort
chronologically as strings.

* `key(for: date, calendar)`: the year/month/day components in `calendar`. The year
  is zero-padded to 4 digits; month and day to 2.
* `startOfDay(for: key, calendar)`: split on `-`; there must be exactly 3 integers.
  Build the date at **12:00** on that day, then take `startOfDay`. Using noon handles
  DST zones that have no 00:00. A malformed key gives nil.
* `adding(days, to key, calendar)`: `key(startOfDay(key) + days calendar days)`.
* `key(forStoredStartOfDay instant, preferredOffset)`: legacy conversion.
  ```
  s = round(instant.unixSeconds); sod = ((s % 86400) + 86400) % 86400
  candidates = [-sod, 86400 - sod] filtered to -43200...50400
  offset = candidate closest to preferredOffset (first wins on tie), default 0
  return key(instant + offset, UTC calendar)
  ```

#### 1.4.3 `UsagePool`

The cases are `hourly`, `weekly`, `monthly` and `mixed`.

* `displayName` is Hourly, Weekly, Monthly or Mixed.
* `sectionTitle = "Usage Pool <displayName>"`.
* `combining(pools)`: exactly one unique value gives that value. Anything else,
  including an empty list, gives `mixed`.

#### 1.4.4 `DailyTokenUsage` / `DailyTokenUsageTally`

`DailyTokenUsage` has these fields:

* `inputTokens`, `outputTokens`, `cacheReadTokens`, `cacheWriteTokens`: i64
* `costUSD: f64?`
* `hasUnpriced: bool`
* `models: [Model{name, tokens}]`

`totalTokens` is the sum of the four token counts.

`add(model, input, output, cacheRead, cacheWrite, costUSD?)`:

1. `tokens` is the sum of the four inputs. Add each to its counter.
2. If the cost is set, `costUSD = (costUSD ?? 0) + cost`. Otherwise, if
   `tokens > 0`, set `hasUnpriced = true`.
3. If `tokens > 0` and the model name is non-empty, merge into `models` by name.
4. Sort `models` by tokens descending, then by name ascending.

`DailyTokenUsageTally(calendar)`: `days: [DayKey: DailyTokenUsage]`.
`add(at: date, …)` adds to `days[DayKey(date)]`.

`[DailyBudgetDay].attachingTokens(calendar, f)` sets `day.tokens = f(DayKey(day.date))`
when that is non-nil. The dictionary overload returns `self` unchanged when the map
is empty.

#### 1.4.5 `DailyBudgetDay`, `WeekOffset`, `DailyBudget`

`DailyBudgetDay { date (startOfDay), spentUSD, budgetUSD, isPriorWindow = false, tokens? }`.
On quota charts the "USD" fields hold percentage points.

* `percentOfBudget = budget > 0 ? clamp(spent / budget * 100) : 0`.
* `isOverBudget = spent > budget && budget > 0`.

`WeekOffset` (0 is the current window; negative values are the past):

* `previous(o) = o - 1`
* `next(o) = min(0, o + 1)`
* `canGoNext(o) = o < 0`

`DailyBudget` functions. All use the local calendar unless one is passed:

* `budgetPerDay(limit, days) = limit > 0 && days > 0 ? limit/days : 0`.
* `daysInBillingCycle(start, end) = max(1, dayDiff(startOfDay(start), startOfDay(end)))`.
  If the component is missing, use 30.
* `buildDays(periodStart, periodEnd, limit, spentByDay, now?)`:
  * `start = sod(periodStart)`, `endDay = sod(periodEnd)`.
  * `includeEndDay = now != nil && now >= start && now < periodEnd && sod(now) == endDay`.
  * `exclusiveEnd = includeEndDay ? endDay + 1 day : endDay`.
  * `totalDays = daysInBillingCycle(start, exclusiveEnd)`.
  * `periodDays = daysInBillingCycle(start, endDay)`.
  * `perDay = budgetPerDay(limit, periodDays)`. The share stays the same on the extra
    reset-day bar.
  * One bar for each of `totalDays` days from `start`, with
    `spent = spentByDay[sod] ?? 0`.
* `weeklyPeriodStart(resetsAt, daysInPeriod, now)`:
  * `count = max(1, n)`, `resetDay = sod(resetsAt)`.
  * If `now >= resetsAt`: start at `base = resetDay` and, up to 520 times, advance by
    `count` days while `sod(base + count - 1 days) < sod(now)`.
  * Otherwise `base = resetDay - count days`.
  * Return `sod(base)`.
* `buildWeeklyWindowDays(limit, daysInPeriod, resetsAt, spentByDay, windowStart?, interruptedWindowStart?, weekOffset = 0, now)`:
  * `count = max(1, n)`, `perDay = limit / count`,
    `weekStart = weeklyPeriodStart(...)`.
  * If `weekOffset != 0`: return exactly `count` bars from
    `weekStart + weekOffset*count` days. There is no prior-window lead-in.
  * If `windowStart != nil && windowStart <= now && sod(windowStart) >= weekStart`:
    set `weekStart = sod(windowStart)` and mark the window as anchored.
  * If anchored **and** `interruptedWindowStart` is set:
    `earliest = weekStart - (count-1) days`,
    `first = max(sod(interrupted), earliest)`. Prepend
    `dayDiff(first, weekStart)` bars flagged `isPriorWindow = true`.
  * Then append `count` normal bars from `weekStart`.
* `mondayOfWeek(now)`: `today - ((weekday + 5) % 7)` days, where weekday 1 is Sunday.
* `buildMondayWeekDays(periodStart, periodEnd, limit, spentByDay, priorWindowSpentByDay = [:], historyByDay = [:], weekOffset = 0, now)`:
  * `periodDays = buildDays(…, now)`.
  * `perDay = periodDays.first?.budget ?? budgetPerDay(limit, max(1, count))`.
  * `spentInPeriod` is a map from the period bars (last value wins).
  * `weekStart = mondayOfWeek(now) + weekOffset*7`.
  * For each of the 7 days, with `key = sod(day)`:
    * `weekOffset != 0 && key < sod(periodStart)`: `spent = prior[key] ?? history[key] ?? 0`
      (not flagged).
    * else if `key < sod(periodStart)` and `prior[key]` exists: use it, flagged
      `isPriorWindow`.
    * else `spent = spentInPeriod[key] ?? 0`.
* `subscriptionMonth(knownStart?, resetsAt?, now?) -> (start, end)?`:
  * Both known with `reset > start`: `(start, reset)`, anchor = reset, `endOffset = 0`.
  * Only start: `end = start + 1 month` (must be > start), anchor = start,
    `endOffset = 1`.
  * Only reset: `start = reset - 1 month`, anchor = reset, `endOffset = 0`.
  * Otherwise nil.
  * If `now` is given, repeat up to 520 times while `end <= now`:
    `advanced = anchor + (endOffset+1) months` (must be > end);
    `start = end; end = advanced; endOffset += 1`.
  * Months are added from the **anchor** with day clamping, so a Jan 31 anchor gives
    Feb 28 and then Mar 31.
* `buildSubscriptionMonthLast7Days(limit, spentByDay, knownStart?, resetsAt?, historyByDay, weekOffset, now)`:
  compute `subscriptionMonth`, then `buildMondayWeekDays`. Returns
  `(days, sod(bounds.start))`, or nil.
* `elapsedDaysThroughToday(periodStart, now) = today < start ? 0 : dayDiff + 1`.
* `AllowancePeriod` is `weekly` or `monthly`.

Pace maths (in `DailyBudgetPace.swift`):

* `weeklyPacePeriodStart(days)`: `sod` of the first bar that is not `isPriorWindow`.
* `monthlyPacePeriodStart(days, knownStart?, resetsAt?, now)`:
  * If `subscriptionMonth` succeeds: `sod(start)`.
  * Otherwise `daily = days.first?.budget ?? 0` and
    `dip = daysInPeriod(fromDailyBudget: daily, fallback: 0)`. If `resetsAt` is set
    and `dip > 0`, return `sod(resetsAt) - dip days`. Otherwise nil.
* `pacePeriodStart(period, …)` dispatches to one of the two above.
* `daysInPeriod(fromDailyBudget d, fallback) = d > 0 ? max(1, round(100 / d)) : max(0, fallback)`.
* `paceHeadroom(days, periodConsumed, elapsedDaysInPeriod?, resetsAt?, now) -> PaceHeadroom?`:
  ```
  days = days.filter(!isPriorWindow); guard first = days.first, first.budget > 0 else nil
  periodLength = max(1, daysInPeriod(first.budget, fallback: days.count))
  elapsed = elapsedDaysInPeriod != nil ? clamp(elapsed, 0, periodLength)
            : count of days with sod(date) <= sod(now)
  earned = first.budget * elapsed; consumed = max(0, periodConsumed)
  headroom = earned - consumed
  if resetsAt && resetsAt > now && elapsed >= periodLength {
     daysUntilReset = (resetsAt - now) / 86400
     headroom = (first.budget * periodLength - consumed) - first.budget * daysUntilReset
  }
  → PaceHeadroom{dailyBudget: first.budget, earned, periodConsumed: consumed, headroomToday: headroom, resetsAt}
  ```
* `paceCaption(p)` with `eps = 0.05`. The checks run in this order:
  1. `consumed <= 0.001`:
     * `dailyBudget <= 0` → nil
     * `earned > daily + eps` → `"%.1f%% usage left today from unused prior days"` (headroom)
     * otherwise → `"No usage yet · %.1f%% today"` (dailyBudget)
  2. `headroom < 0` → `"Usage %.1f%% over today's allowance"` (−headroom)
  3. `headroom > daily + eps` → `"%.1f%% usage left today from unused prior days"`
  4. `headroom < eps` → `"On pace for today"`
  5. otherwise → `"%.1f%% usage left today"`

### 1.5 Core/Utilities

* `AppLog.subsystem = "com.modelmonitor.app"`.
  `AppIdentity.userAgent = "TokenMon/<version|unknown>"`. Logger categories used
  here: `Cache`, `FileStore`, `Keychain`, `Alerts`, `Update`, plus each provider's
  category.
* `Format.tokens(Int64)`:
  ```
  v < 1000 → "\(count)"
  k = roundHalfAwayFromZero(v/1000)      ; k < 1000 → "%.0fK"
  m = v/1e6; dec = round1(m) < 10 ? 1 : 0; shown = round_dec(m)
  shown < 1000 → "%.{dec}fM"
  else → "%.1fB" of round1(v/1e9)
  ```
  `round1(x) = roundHalfAway(x*10)/10`. Examples: 999 gives "999"; 1 000 gives
  "1K"; 1 500 gives "2K"; 2 500 gives "3K"; 999 499 gives "999K"; 999 500 gives
  "1.0M"; 1 234 567 gives "1.2M"; 1 250 000 gives "1.3M"; 9 949 999 gives "9.9M";
  9 950 000 gives "10M"; 123 000 000 gives "123M"; 999 499 999 gives "999M";
  999 500 000 gives "1.0B"; 2 500 000 000 gives "2.5B".
* `Format.usd(x)`: en_US currency, two decimals, `$` symbol. 4.1 gives "$4.10";
  0 gives "$0.00".
* `Format.hourLabel(h)`: 0 gives "12a", 12 gives "12p", 1–11 give "<h>a",
  13–23 give "<h-12>p".
* `Format.date(date, dateFormat, timeZone?)`: a cached formatter per
  `(format, zone id)`, locale `en_US_POSIX`. A nil zone means the current system
  zone at call time. The cache is cleared on a system time-zone change, and
  `cachedFormatterCount` exposes its size.
* `Format.resetDate(...)`: as `Format.date`, then replace `AM` with `am` and `PM`
  with `pm`. `resetCaption(date, format = "EEE dd MMMM h:mma")` gives
  `"Resets " + resetDate`, for example "Resets Thu 16 July 6:57pm".
* `ISO8601DateFormatter.parseFlexible(s)`: RFC 3339 with an optional fractional
  part. A time zone (`Z`, `±HH:MM` or `±HHMM`) is **required**. Fractional precision
  is kept (microseconds included). A date-only string, a string without a zone, or
  garbage gives nil. In Rust, use `chrono::DateTime::parse_from_rfc3339`, plus a
  fallback for the `+0530` form.
* `JSON` helpers over `serde_json::Value`:
  * `number(v)`:
    * Numbers that are not booleans give f64.
    * Strings are trimmed of spaces and then parsed as a double.
    * An object returns `number(obj["val"]) ?? number(obj["value"])`.
    * Anything else is nil.
    * The result must be finite: `"1e400"`, `"inf"` and `"NaN"` give nil, and so
      does `true`.
  * `decimal(v)`: the same rules, but the value keeps its written digits ("19.99"
    stays exactly 19.99). Use `rust_decimal` or keep the string. A string must parse
    as a finite double first; "1.5abc" gives nil.
  * `string(v)` is the string only. `nested(dict, keys)` walks objects.
    `firstDouble`, `firstString` and `firstDecimal` return the first key that gives
    a value.
  * `firstBool(dict, keys, fallback = false)`: for each key, a bool is returned as-is
    and a number `n` returns `n != 0`. Otherwise `fallback`.
* `ObservationLoop(coalescing = 0, initial = true, track, handle)`: a reactive
  helper. `track` reads observable state; whenever any value it read changes, the
  loop waits `coalescing`, re-runs `track` once and calls `handle(latest)`. Bursts
  collapse to one call with the latest value. If `initial` is true, `handle` also
  receives the first reading synchronously. `cancel()` stops it. In Rust, use an
  explicit "inputs changed" signal on a channel with a debounce timer (section 8).

### 1.6 Platform/Auth: credential storage and sessions

#### 1.6.1 `CredentialStore` interface

```
value(key) -> String?
set(value, key) -> Bool
remove(key)
```

Implementations:

* `FileBackedCredentialStore(prefix)`: delegates to `FileBackedStringStore`
  (files are 0600).
* `InMemoryCredentialStore`: used by tests.
* `KeychainCredentialStore(accountPrefix, vault)`: account =
  `accountPrefix + key` in the shared `KeychainVault`.
* `SecretRoutingCredentialStore(secure, files, secretKeys)`: the production store.

#### 1.6.2 `SecretRoutingCredentialStore`

Production is `live(filenamePrefix, secretKeys)`:
`secure = KeychainCredentialStore(prefix)`, `files = FileBackedCredentialStore(prefix)`.

Secret keys:

* `["session"]` for every `ProviderAuthSession`.
* `["key"]` for OpenRouter.

Other keys (`email`, `account`, `workspace`) always use the file store.

| Op | Secret key | Non-secret |
|---|---|---|
| init | For each secret key, sorted: if a file holds a value, `secure.set` it and read it back. If the read-back matches, delete the file; otherwise keep it and log "Keychain migration failed…". | – |
| value | A value in the file store wins (the migration failed earlier), otherwise the secure value. | file |
| set | If `secure.set` succeeds, delete the file and return true. Otherwise log and return `files.set(...)`. | file |
| remove | `secure.remove`, then `files.remove`. | file |

#### 1.6.3 `KeychainVault`: one secure item for every secret

* Service `com.modelmonitor.app.credentials`, account `vault`. The item is a generic
  password, `AfterFirstUnlockThisDeviceOnly`, labelled `TokenMon (<account>)`.
* Payload JSON: `{"entries":{"<prefix><key>":"<secret>",…},"checkedSeparateItems":["<account>",…]}`.
  The second field is a set, encoded as an array.
* State: `unloaded`, `loaded(payload)` or `unavailable`. Access is serialized with a
  lock, and the vault is read once and then cached.
* `readVault()`:
  * Success with JSON that decodes: `loaded`.
  * Success with JSON that does not decode: `unavailable`, logged.
  * Item not found: `loaded(empty)`.
  * Any other status (denied and so on): `unavailable`.
* After the first load, `applyPendingRemovals()` runs (below).
* `value(account)`:
  * Not loaded: nil.
  * In `entries`: return it.
  * Already in `checkedSeparateItems`: nil.
  * Otherwise `adoptSeparateItem(account)`: read the per-account legacy item
    (same service). Success decodes UTF-8; not-found or a denial give nil and are
    logged. Insert the value if found, always add the account to
    `checkedSeparateItems`, and `write`. If the write succeeds and a value was found,
    delete the separate item.
* `set(value, account)`:
  * Not loaded: false.
  * Otherwise `entries[account] = value`, add the account to the checked list, and
    return `write(payload)`.
* `remove(account)`:
  * Not loaded: record a **pending removal** and return.
  * Otherwise remove the entry, add the account to the checked list, and always try
    to delete the separate item (not-found is fine). Write only if something
    changed.
* `write(payload)`:
  1. Encode, then `update`. If the item is not found, `add`. A failure returns false.
  2. Read the item back and decode it. If it differs, log "read-back mismatch" and
     return false.
  3. On success, update the cached state.
* Pending removals are stored under the defaults key `keychainVaultPendingRemovals`
  (a string array, no duplicates), or in memory when no defaults object is
  injected. On load, each pending account is removed and marked checked, and its
  separate item is deleted. The record is cleared when nothing changed or the write
  succeeded.
* Once the vault is unavailable, **every read returns nil and every write is refused
  for the rest of the process**. This stops it from overwriting secrets it could not
  read.

#### 1.6.4 `ProviderAuthSession`

This is the cookie session shared by Grok, Cursor (also used by Grokbot), Claude,
ChatGPT and OpenCode.

`ProviderAuthConfig` fields:

* `storeFilenamePrefix`, `logCategory`
* `extraStoreKeys`: for example `["workspace"]` for OpenCode
* `signOutHosts`: also the base host for parsing refreshed cookies (the first entry)
* `capturePolicy`, `isDomain`
* `accountIdentityCookie`: Claude uses `"lastactiveorg"`; nil means the account is
  identified by email

Observable state: `isSignedIn`, `accountEmail`, `needsSignIn` (read/write, starts
true), `lastAuthError`, `consecutiveAuthFailures` and `sessionGeneration`
(starts 0).

Events:

* `accountReset` fires only when a **different** account signs in. Pollers clear the
  snapshot and the hourly and daily history on this event.
* `signedOut` fires on explicit sign-out. Pollers clear the snapshot only.

Constants: `authFailureThreshold = 3`; `secretStoreKeys = ["session"]`.

Store selection in `init`:

* An injected `store` is used as-is.
* Otherwise, if a `directory` is given, `FileBackedCredentialStore(dir, prefix)`.
* Otherwise `SecretRoutingCredentialStore.live(prefix, ["session"])`.

`init` then calls `refreshFromDisk()`.

| Method | Effect |
|---|---|
| `refreshFromDisk()` | `isSignedIn = loadCookieHeader() non-empty`, `accountEmail = store["email"]`, `needsSignIn = !isSignedIn`, `consecutiveAuthFailures = 0`, `sessionGeneration += 1`. |
| `recordAuthFailure(reason)` | Increment the counter. Below 3: log "auth rejection n of 3" and return false. At 3: `markSessionInvalid(reason)` and return true. |
| `recordAuthSuccess()` | Counter = 0. |
| `markSessionInvalid(reason?)` | `needsSignIn = true`; `lastAuthError = reason` if non-nil; `clearBrowserState()`; `isSignedIn = false`; `accountEmail = nil`; counter = 0. **No** `accountReset`. The stored `account` identity is kept. |
| `isCurrent(gen)` | `gen == sessionGeneration && isSignedIn && !needsSignIn`. |
| `cookieHeader()` / `loadCookieHeader()` | Read `session`. Run `pruneCookieHeader`; if pruning changed it, write the pruned value back. Return it. |
| `pruneCookieHeader(h, policy)` | No allowlist: return `h`. Otherwise split on `;` and trim. If **no** pair's name is essential, return `h` unchanged. Otherwise keep only the essential pairs, joined with `"; "`. |
| `applyRefreshedCookies(setCookieHeaders, now)` | See below. |
| `saveAccountEmail(e)` | Write `email`, set `accountEmail`. If the config has no identity cookie, call `noteAccountIdentity(e)`. |
| `noteAccountIdentity(id)` | `prev = store["account"]`. If equal, return. Otherwise write `account = id`; if `prev != nil`, fire `accountReset`. |
| `save(cookieHeader)` | Write `session`; `isSignedIn = true`; `needsSignIn = false`; counter = 0; `sessionGeneration += 1`. |
| `captureCookiesFromWebKit()` | Await any pending clear task, then `WebKitCookieCapture.capture(policy, signInDataStore)`. Nil: `lastAuthError = policy.failureMessage`, return false. Otherwise `adopt(result)` and return true. |
| `adopt(result)` | `save(result.cookieHeader)`. Identity is the identity-cookie value (case-insensitive name, non-empty) or `result.email`; if present, `noteAccountIdentity`. If `email` is present, write it and set it; otherwise **remove** the stored email and set it to nil. Then `isSignedIn = true`, `needsSignIn = false`, `lastAuthError = nil`. |
| `signOut()` | `clearBrowserState()`; `isSignedIn = false`; `accountEmail = nil`; `needsSignIn = true`; `lastAuthError = nil`; counter = 0; fire `signedOut`. The `account` identity is kept. |
| `clearBrowserState()` | `sessionGeneration += 1`; remove `session`, `email` and each extra key; clear `HTTPCookieStorage.shared` cookies for `https://<signOutHost>`; cancel the previous clear task and start one that deletes **all** cookies in this provider's isolated WebKit store plus the cookies matching `isDomain` in the default WebKit store. |

`applyRefreshedCookies(setCookieHeaders, now)` (rolling NextAuth session):

1. Return if the list is empty, nothing is stored, or there is no sign-out host.
2. Parse every `Set-Cookie` value against `https://<firstSignOutHost>/`. One value
   may hold several cookies separated by commas, and `Expires` dates contain commas
   themselves. Keep only **essential** names, with the last value per lowercased
   name winning, in first-seen order.
3. A cookie is **live** when its value is non-empty and its expiry is in the future
   or absent. `Max-Age=0` or a past `Expires` means it is dead.
4. `renewedFamilies` = the essential prefix families of the live refreshed cookies.
5. Keep a stored pair if its name was refreshed. Drop it if it belongs to a renewed
   family. Keep it otherwise.
6. For each refreshed cookie:
   * live and present: replace the value
   * live and absent: append it
   * dead and present: remove it
   * dead and absent: ignore it
7. Join with `"; "`. If the result differs, write `session` **without** bumping
   `sessionGeneration`. The result can be an empty string.

The secret key is `session`; the other keys are `email`, `account` and
`workspace` (OpenCode). OpenRouter uses its own session (`openrouter_auth_`,
secret key `key`) with the same generation and streak semantics.

#### 1.6.5 `WebKitCookieCapture`

`Policy` fields:

* `isDomain`, `isPreferredSessionCookie`, `looksLikeAuthCookie`
* `includeAllDomainCookiesWhenSessionFound` (default true)
* `essentialCookieNames` and `essentialCookiePrefixes`: both lowercased
* `maxAttempts` (default 1; providers use 4)
* `retryDelayNanoseconds` (default 400 ms)
* `failureMessage`

Helpers:

* `hasEssentialCookieAllowlist`: either set is non-empty.
* `isEssential(name)`: the lowercased name is in the names set, or equals a prefix,
  or starts with `<prefix>.`.
* `essentialFamily(name)`: the matching prefix, if any.

`capture(policy, store)`: up to `maxAttempts` times, read every cookie and run
`select`. If the selection is non-empty, return:

* `cookies`
* `cookieHeader`: `"name=value"` pairs joined with `"; "`
* `email`: from `extractEmail`

Between attempts, sleep for the retry delay. Return nil if every attempt fails.

`select(cookies, policy)`:

```
relevant = cookies where isDomain(domain); empty → nil
if allowlist: essential = relevant.filter(isEssential(name)); return essential.contains(isPreferredSessionCookie) ? essential : nil
preferred = first preferred; authish = filter looksLikeAuthCookie
preferred → includeAll ? relevant : [preferred]
authish non-empty → includeAll ? relevant : authish
else nil
```

`extractEmail(cookies)`: first try cookies named `email` or `user_email`
(case-insensitive), percent-decoded, that pass `isPlausibleEmail`. Then try any
cookie whose percent-decoded value passes. Otherwise nil.

`isPlausibleEmail(v)`:

* At most 254 characters.
* Exactly one `@`.
* The local part is 1–64 characters, drawn from alphanumerics plus ``.!#$%&'*+-/=?^_`{|}~``.
* The domain is drawn from alphanumerics plus `.` and `-`.
* The domain splits on `.` into at least 2 labels. No label is empty, and none
  starts or ends with `-`.
* The TLD has at least 2 characters, all letters.

Provider policies are defined in the provider slices. The allowlist tests in 7
check them:

| Provider | Hosts | Essential names / prefixes | Preferred session cookie |
|---|---|---|---|
| Grok | `grok.com`, `x.ai` (x.com/twitter.com are auth hosts only) | `sso`, `sso-rw` | sso / sso-rw |
| Cursor | cursor hosts | `workoscursorsessiontoken` | WorkosCursorSessionToken |
| Claude | claude hosts | `sessionkey`, `lastactiveorg` | `sessionKey` |
| ChatGPT | chatgpt hosts | prefix `__secure-next-auth.session-token` | that family |
| OpenCode | `opencode.ai`, `auth.opencode.ai` | `auth`, `provider`, `__host-console_session` | console session |

### 1.7 Platform: `SystemWakeGate`

This gate pauses polling across system sleep and resumes it once the network is
back.

* `fallbackDelay = 30` seconds.
* State: `isNetworkSatisfied` (starts true), `isAwaitingNetwork`,
  `pathUpdatedSinceSleep`, `fallbackTask`.

| Event | Behaviour |
|---|---|
| `systemWillSleep()` | `isAwaitingNetwork = false`, `pathUpdatedSinceSleep = false`, cancel the fallback, call `onSleep()`. AppModel pauses every `PollingLoop`. |
| `systemDidWake()` | `isAwaitingNetwork = true`. If `pathUpdatedSinceSleep && isNetworkSatisfied`, call `finishWake()` now. Otherwise (re)start the fallback task: sleep 30 s, then `finishWake()`. |
| `pathChanged(satisfied)` | `isNetworkSatisfied = satisfied`, `pathUpdatedSinceSleep = true`. If satisfied and `isAwaitingNetwork`, call `finishWake()`. |
| `finishWake()` | Only while awaiting. Clear the flag, cancel the fallback, call `onReady()`. AppModel resumes every loop. |

Path changes outside a wake do nothing. The gate is created only in the real app,
not under tests.

### 1.8 `ThresholdNotifier`: see section 5

---

## 2. `AppSettings`: every key

The store is `UserDefaults.standard`. Under tests it is the throwaway suite
`com.modelmonitor.app.tests.appmodel`, emptied on each launch. Every property is
observable. Arrays of providers are stored as raw-value string arrays.

| Key | Type (stored) | Default | Clamp / validation | Controls |
|---|---|---|---|---|
| `showCategoriesInMenuBar` | Bool | `true` | – | Grok category segments in the menu-bar image ("Grok categories" toggle) |
| `menuBarGraphProviders` | [String], sorted | `["grok"]` (see migration) | ∩ `{grok, grokbot, claude, cursor, opencode}` | Which providers' bar graphs are pinned in the menu bar; also feeds `needsPolling` |
| `showSelectedProviderInMenuBar` | Bool | `false` | – | Menu bar shows only the selected provider (icon + % + bar) instead of the pinned composite |
| `checksForUpdates` | Bool | `true` | – | Automatic update checks every 6 h (and the menu "update available" row) |
| `activePollSeconds` | Int | `60` | clamp **15…300**, on load and on set (step 15 in UI) | Poll interval while the dropdown is open |
| `idlePollSeconds` | Int | `300` | clamp **60…3600** (step 60) | Poll interval while closed |
| `thresholdEnabled` | Bool | `false` | – | Usage alert on/off |
| `thresholdPercent` | Double | `80` | clamp **50…99** (slider step 1) | Alert threshold |
| `alertProviders` | [String], sorted | all usage providers | unknowns dropped (∩ usageProviders). A saved **empty** array stays empty. | Providers the alert watches. They keep polling while the alert is on. |
| `selectedProvider` | String | `"grok"` | unknown → grok; `overview` is valid | Selected dropdown tab |
| `enabledProviderIDs` | [String], sorted | all usage providers | load: ∩ usageProviders, empty → all. Set: an empty or non-subset value **reverts** to the old one. | Provider on/off (lifecycle gate). Disabling the selected tab sets `selectedProvider = overview`, which is persisted. |
| `providerOrder` | [String], normalized order | `usageProviders` | `normalizedOrder` on load and on set (the normalized value is persisted) | Order of tabs, Overview cards and composite menu-bar graphs |
| `visibleProductIDs` | [String] (unordered) | all known `["build","api","chat","imagine","voice","other"]` | Load: lowercase. A saved empty array stays empty. A non-empty array keeps ∩ known; if that is empty, all known. | Grok product categories shown in the breakdown |
| `collapsedPanelSections` | [String], sorted | `[]` | unknown values dropped; values `claudeModels`, `openCodeModels` | Closed dropdown sections (`isExpanded = !contains`) |
| *(launch at login)* | not in defaults | read from the OS | – | `launchAtLogin` and `launchAtLoginNeedsApproval` |

Write behaviour:

* These properties persist only when the value actually changes:
  `showCategories`, `menuBarGraphs`, `checksForUpdates`, `showSelected`,
  `alertProviders`, `collapsedPanelSections`, `providerOrder`, `enabledProviderIDs`
  and `selectedProvider`.
* `activePoll`, `idlePoll`, `thresholdEnabled`, `thresholdPercent` and
  `visibleProductIDs` write on every set.

**Migration of per-provider graph flags.** This runs when `menuBarGraphProviders`
is absent. For each of these pairs:

* `(grok, showGrokBarInMenuBar)`, default **true**
* `(opencode, showOpenCodeBarInMenuBar)`, default false
* `(cursor, showCursorBarInMenuBar)`, default false
* `(claude, showClaudeBarInMenuBar)`, default false
* `(grokbot, showGrokbotBarInMenuBar)`, default false

read the Bool (or its default), include the provider when it is true, and
**remove** the old key. Then save the set.

Derived values:

* `visibleUsageProviders` = `orderedUsageProviders` filtered to the enabled ones.
* `orderedUsageProviders` = `normalizedOrder(providerOrder)`.
* `alertWatches(p)` = `thresholdEnabled && alertProviders.contains(p)`.
* `isProviderEnabled(p)` = `enabledProviderIDs.contains(p)`.
* `needsPolling(p)`:
  ```
  p != overview && enabled(p) &&
  (selectedProvider == p || selectedProvider == overview || menuBarGraphProviders ∋ p || alertWatches(p))
  ```
* `pollSeconds(menuIsOpen)` = `menuIsOpen ? active : idle`.
* `pollInterval(p, menuIsOpen)` = `needsPolling(p) ? pollSeconds : nil`.
* `moveProvider(moving, target)`: `from` and `to` are indexes into the current
  order. Move `moving` so it ends up at index `to` (Swift
  `move(fromOffsets:[from], toOffset: from < to ? to+1 : to)`), then assign
  `providerOrder`. Example: claude onto grok gives `[claude, grok, …]`.
* `setMenuBarGraph(p, shown)` and `showsMenuBarGraph(p)`.
* `setExpanded(bool, section)`.

**Launch at login** (`SMAppService.mainApp`):

* `launchAtLogin` is true when the status is `enabled` or `requiresApproval`.
  `launchAtLoginNeedsApproval` is true when the status is `requiresApproval`.
* Setting `launchAtLogin` calls `register()` or `unregister()`; errors are
  swallowed. It then calls `refreshLaunchAtLoginStatus()`, which re-reads the status
  and reverts the toggle to the real state without recursing (guard flag
  `isRevertingLaunchAtLogin`).
* The Settings window calls `refreshLaunchAtLoginStatus()` when it appears.

Other defaults keys used by this slice:

| Key | Type | Meaning |
|---|---|---|
| `legacyURLCacheRemoved` | Bool | 1.3.3 |
| `legacyCookieJarRemoved` | Bool | 1.3.3 |
| `keychainVaultPendingRemovals` | [String] | 1.6.3 |
| `thresholdNotified.<scope>` | Double | last notified threshold (section 5) |
| `thresholdNotifiedResetsAt.<scope>` | Double (**Unix**-seconds) | the period that record belongs to |

### 2.1 Settings window

Window "TokenMon", id `preferences`. Default size 480×640, minimum content
460×520, font size 14, grouped form style.

On appear it refreshes the launch-at-login status. On disappear it calls
`hideDockIfNoWindows`.

It has four tabs: **General**, **Providers**, **Accounts** and **Data**.

**General**

* Section "Startup":
  * Toggle "Launch at Login".
  * When approval is pending: the caption "Waiting for approval in System Settings › General › Login Items." and a button "Open Login Items…" that opens the system pane.
* Section "Updates":
  * Toggle "Check automatically" (`checksForUpdates`).
  * Row "Version <CFBundleShortVersionString or unknown>" with a button titled
    `updateChecker.actionTitle`. It runs `performPrimaryAction()` and is disabled
    when `!canAct`.
  * A caption showing `statusMessage` when set.
  * Footer: "Checks GitHub for a newer release. Update downloads the release and replaces this copy of TokenMon."
* Section "Refresh":
  * Stepper "While menu is open", showing "<n>s", over 15…300 in steps of 15.
  * Stepper "While idle", showing "<n>s", over 60…3600 in steps of 60.
  * Refresh status. If the selected provider is Overview, show one line per
    registered provider: "<Name>: <date abbreviated, time shortened>". Otherwise
    show "Last refresh: <time>" plus the poller's `lastError` in red caption.
    Nothing is shown when there has been no refresh.
* Section "Alerts":
  * Toggle "Notify when usage exceeds threshold".
  * When the alert is on:
    * A slider "Alert at <n>%" over 50–99, step 1, with min and max labels "50%"
      and "99%".
    * One toggle per **visible** usage provider (`alertProviders` membership).
    * Footer: "Providers the alert watches keep refreshing in the background."
* An unnamed section with the button "Quit TokenMon".

**Providers**

* Section "Providers": one row per provider in `orderedUsageProviders`. Each row has:
  * a drag handle (≡) for reordering. On drag-enter over another row it calls
    `moveProvider(dragging, target)` with a 0.12 s animation.
  * a 16 px logo
  * a toggle `<displayName>` that adds or removes the provider from
    `enabledProviderIDs`. It is **disabled when it is the only enabled provider**.

  Footer: "Turn a provider on to show its tab in the menu. Drag a row to set the order of tabs, Overview cards, and menu bar graphs."
* Section "Menu Bar":
  * Toggle "Show selected provider only".
  * When that is off: for each of `[grok, grokbot, claude, cursor, opencode]`, a
    toggle "<Name> bar graph". Directly after Grok's comes the toggle
    "Grok categories" (`showCategoriesInMenuBar`).
  * Footer when on: "The menu bar shows only the active provider's icon, percentage, and usage bar."
    Footer when off: "Choose which graphs stay pinned in the menu bar."

**Accounts**

* Section "Accounts": rows for Grok, Claude, ChatGPT, Cursor and OpenCode, in that
  order. Each row has:
  * a logo and the provider name
  * a subtitle: the email, or "Signed in" when signed in without one, or
    "Not signed in". It is truncated in the middle.
  * when signed in: the buttons "Re-authenticate…" (opens the sign-in window) and
    "Sign Out" (destructive, calls `auth.signOut()`)
  * when signed out: "Sign In…"
  * `lastAuthError` in a red caption

  Footer: "Grokbot uses your Cursor account."
* Section "OpenRouter":
  * When signed in: "Connected with" / "OpenRouter API key", and "Sign Out", which
    also clears the draft.
  * Otherwise: "Not connected", a secure field with placeholder `sk-or-v1-…`, and a
    "Save API Key" button, disabled while the trimmed draft is empty. On success it
    clears the draft and runs `poller.refreshNow()`. `lastAuthError` shows in red.
  * Validation lives in the OpenRouter slice:
    * empty: "Enter an OpenRouter API key."
    * wrong format: "That doesn't look like an OpenRouter key (expected sk-or-…)."

**Data**

* Section "SuperGrok Categories": one toggle per known product id, labelled with
  the display name:
  * build: Grok Build
  * api: API
  * chat: Chat
  * imagine: Imagine
  * voice: Voice
  * other: Other

  Footer: "Categories shown in the Grok breakdown."
* Section "SuperGrok History":
  * "Export history" with the buttons "CSV…" and "JSON…". These open a save dialog
    with default name `grok-usage.csv` or `grok-usage.json` and write
    `GrokExportService.export(history.allSnapshots(), format)`. Errors show in red,
    and a new export clears the old error.
  * "Clear Local History" (destructive), which calls `history.clear()`.
  * If `history.storeFailed`: "History storage is unavailable — usage is kept for this session only and will not survive a relaunch." in red.
  * Footer: "Clearing history does not reset your SuperGrok weekly pool."

The sign-in window group has id `signin` and one window per `signInProvider`
(default 920×700, content minimum size). It shows `ProviderSignInSheet` with the
session's sign-in config title. On success it calls `AppModel.signInCompleted`.
Closing it calls `hideDockIfNoWindows`.

---

## 3. Update checker

### 3.1 Feed

* `owner = "faulknerpearce"`, `repository = "token_monitor"`.
* `GET https://api.github.com/repos/faulknerpearce/token_monitor/releases/latest`.
  * Headers: `Accept: application/vnd.github+json` and
    `X-GitHub-Api-Version: 2022-11-28`. Timeout 15 s.
  * The request uses `URLSession.shared`, so Foundation's default User-Agent applies.
    **Rust must set a User-Agent** (for example `TokenMon/<ver>`) because GitHub
    rejects requests without one.
* Errors (the message prefix is "Update check failed: "):
  * transport failure: `network(localizedDescription)`
  * non-HTTP response: `badResponse("No HTTP response")`
  * status not 2xx: 403 gives `"GitHub rate limit reached; will retry later"`;
    anything else gives `"HTTP <code>"`
* Cadence: `checkInterval = 6 * 3600` seconds. The check runs on a `PollingLoop`
  whose interval is `checksForUpdates ? 21600 : nil`. Every check counts as success,
  so there is no backoff, but the ±10% jitter still applies. `start()` runs only
  when `checksForUpdates` is on, and the loop refreshes immediately at launch.

### 3.2 `AppVersion`

Parsing:

1. Trim whitespace and newlines.
2. Drop everything from the first `+` (build metadata).
3. Drop a release-name prefix. This applies when the first character is a letter
   and a separator (`-`, `_` or space; the first one found) follows a run of letters
   and digits, and the rest (leading spaces dropped, optional `v`/`V`) starts with a
   digit. The text becomes the rest.
4. Drop a leading `v` or `V`.
5. An empty string gives nil.
6. Split once on `-` into the core and the prerelease suffix.
7. Split the core on `.`, keeping empty parts. Every part must be an integer, or the
   result is nil.
8. `prerelease` is the suffix when it is non-empty.

`description` is `"a.b.c"` plus `"-pre"` when there is a prerelease.

Ordering:

* Compare the numeric components with missing ones treated as 0, so
  `1.4 == 1.4.0` and `1.10 > 1.9`.
* When the numbers are equal, a prerelease sorts before the final release.
* Two prereleases compare dot-separated identifiers:
  * numeric vs numeric: numerically
  * numeric vs alphanumeric: numeric first
  * otherwise: lexically
  * if one list is a prefix of the other, the shorter sorts first
* Equality is `!(a<b) && !(b<a)`.

Examples:

* `TokenMon-1.8.0` gives 1.8.0.
* `TokenMon v1.8.0` gives 1.8.0.
* `TokenMon-v1.8.0-beta.2` gives 1.8.0-beta.2.
* `TokenMon-nightly`, `TokenMon-`, `nightly`, the empty string and `1.x.2` all give
  nil.
* `1.8.0+5` equals `1.8.0`.

### 3.3 `ReleaseFeed.newerRelease(data, current)`

1. The root must be a JSON object, otherwise
   `badResponse("Unexpected release payload")`.
2. If `firstBool(["draft"])` or `firstBool(["prerelease"])` is true, return nil.
3. `tag = firstString(["tag_name", "name"])`, parsed with `AppVersion`. Failure
   gives `badResponse("Release carries no usable version tag")`.
4. If `version <= current` or it is a prerelease, return nil.
5. `pageURL = html_url`, or `https://github.com/faulknerpearce/token_monitor/releases/latest`.
6. Assets: each `assets[]` entry with `name` and `browser_download_url` passes
   `isRepositoryReleaseDownload`. `sha256` comes from `digest`.
7. `archive = preferredAsset(ext "zip")`, `installerPackage = preferredAsset(ext "pkg")`,
   `publishedAt = parseFlexible(published_at)`.

Helpers:

* `preferredAsset`: keep names (lowercased) that end with `.<ext>` and do not
  contain `dsym`. Prefer the first name starting with `tokenmon`, otherwise the
  first match.
* `sha256(fromDigest)`: split once on `:`. The left side must be `sha256`
  (case-insensitive) and the hex must be 64 hex characters. Returns it lowercased.
* `isRepositoryReleaseDownload(url)`:
  * The scheme is `https` with no user or password, and the port is absent or 443.
  * The host is `github.com`, case-insensitive.
  * The path components are exactly `["/", owner, repo, "releases", "download", tag, file]`,
    7 in total, with none equal to `..` or `.`.
  * Owner and repo compare case-insensitively; `releases` and `download` are
    case-sensitive.
  * **Rust caution:** the `url` crate normalizes `..` away. Validate the raw path
    segments before parsing, or reject any raw input containing `/../` or `/./`.
* `isTrustedDownload(url)`: either a repository download, or plain HTTPS whose host
  is in `{objects.githubusercontent.com, release-assets.githubusercontent.com}`.
* `AvailableRelease ==` compares `version.description` and `pageURL`.

### 3.4 `UpdateChecker` state machine

Observable: `availableRelease`, `isChecking`, `isInstalling`, `statusMessage` and
`lastError`.

`actionTitle`, in priority order:

1. "Installing update…"
2. "Checking for updates…"
3. "Update to <v>…"
4. "Check for Updates"

`canAct = !isChecking && !isInstalling`.

* `start()`: only if `checksForUpdates`, `loop.start()`. `stop()` stops the loop.
* `settingChanged()` (an `ObservationLoop` on `checksForUpdates` calls it, with
  `initial: false`): on, `loop.start()` (immediate check). Off, `loop.stop()` and
  clear `availableRelease`, `lastError` and `statusMessage`.
* `checkNow()` is the background check; `checkManually()` is the user-initiated one;
  `performPrimaryAction()` installs if a release is available and checks manually
  otherwise.

`performCheck(userInitiated)`:

```
guard userInitiated || checksForUpdates, !isChecking else return
guard currentVersion else { if userInitiated { statusMessage = "This build has no version number to compare." }; return }  // no request
isChecking = true; if userInitiated { statusMessage = nil }
fetch → (after await) guard userInitiated || checksForUpdates else return   // discard result
availableRelease = newerRelease(...)  (nil clears an earlier one); lastError = nil
if release → statusMessage = nil   elif userInitiated → statusMessage = "You're up to date."
on error (same guard): lastError = desc; if userInitiated { statusMessage = desc }
isChecking = false
```

### 3.5 Install flow (`installAvailableUpdate`)

1. Do nothing if there is no release or an install is already running.
2. No zip asset: open the release page with the message "No installer was published — opening the release page.".
3. `!AppInstaller.canReplaceRunningApp(installedAppURL)` means a manual install:
   * Translocated path (contains `/AppTranslocation/`): open the page with
     "Move TokenMon into Applications, then update."
   * Otherwise, if there is a `.pkg`: set `statusMessage = "TokenMon cannot replace itself here. Downloading the installer package — open it to update."`
     and open the pkg URL in the browser.
   * Otherwise open the page with "TokenMon cannot replace itself from this folder."
4. No `sha256` on the zip: open the page with "The release has no checksum to verify — opening the release page."
   The zip is never requested.
5. No current version or bundle id: open the page with "This build has no version or bundle identifier to check against — opening the release page."
6. `isInstalling = true` until the end. Build the `Expectation`:
   `sha256`, bundle id `com.modelmonitor.app`, current version, and the running
   team id (nil when ad-hoc).
7. `AppInstaller.installUpdate`:
   1. The URL must be a repository download, otherwise
      `download("The release file is not one of this project's GitHub releases.")`.
      No request is made.
   2. The destination must be replaceable, otherwise `notWritable`. No request is
      made.
   3. Work directory `<tmp>/tokenmon-update-<UUID>/`, removed afterwards.
   4. **Download** to `TokenMon.zip`:
      * Timeout 60 s, over an ephemeral session whose redirect delegate refuses any
        redirect to a URL that is not trusted.
      * A non-2xx status or a non-HTTP response gives "The release file was not available.".
      * The final URL must be trusted, otherwise "The release file is not hosted on GitHub.".
      * A `Content-Length` over the limit gives "The release file is unexpectedly large.".
      * Stream in 64 KiB chunks. When the cumulative size exceeds
        **80 MiB (83 886 080 bytes)**, fail with the same "unexpectedly large" error.
      * Compute SHA-256 while streaming; the result is lowercase hex.
      * Transport errors give `download(desc)`.
   5. A digest that differs from `expectation.sha256` (lowercased) gives
      `verification("The download does not match the release checksum.")`.
   6. **Extract**:
      * Run `zipinfo -t`. A non-zero exit, or output without
        `<N> bytes uncompressed,`, gives `archive("The release zip could not be opened.")`.
      * A declared size over **256 MiB** gives "The release zip is unexpectedly large.".
      * Run `ditto -x -k` into `unpacked/`. Failure gives "could not be opened".
      * Walk the tree: any symlink gives "The release zip contains a symbolic link.";
        a cumulative size over 256 MiB gives "unexpectedly large"; if the enumerator
        cannot be created, "The release zip could not be read.".
      * `unpacked/TokenMon.app` must be a directory and not a symlink, otherwise
        "The release zip does not contain TokenMon.app at its top level.".
   7. **Stage and swap**:
      * Copy to `<destParent>/.TokenMon-update-<UUID>.app`. Always remove that path
        afterwards. A copy error gives `install(desc)`.
      * `verify` (below).
      * `replaceItemAt(destination, staged)`; an error gives `install(desc)`.
      * A failure at any step leaves the installed app unchanged, and no staged
        sibling is left behind.
   8. **Verify**:
      * The signature must validate against the requirement
        `identifier "com.modelmonitor.app"`, plus
        `and anchor apple generic and certificate leaf[subject.OU] = "<TEAM>"` when a
        team id is expected. Flags: all architectures, strict, nested code.
        * Not signed: "The download is not a signed app."
        * Requirement cannot be built: "The signing requirement could not be built."
        * Invalid: "The code signature is not valid for TokenMon."
      * `Info.plist` `CFBundleIdentifier` must match, otherwise "The download is not TokenMon.".
      * `CFBundleShortVersionString` must parse and be greater than the current
        version, otherwise "The download is not newer than this version.".
      * If the app has no signing team and carries the `com.apple.quarantine`
        xattr: "macOS would block this unnotarized download from opening.".
8. On success, relaunch: run `/bin/sh -c 'while /bin/kill -0 "$1" 2>/dev/null; do /bin/sleep 0.2; done; /usr/bin/open "$2"' tokenmon-relaunch <pid> <appPath>`,
   then terminate the app. If launching the helper throws:
   `statusMessage = "TokenMon <v> is installed. Quit and reopen TokenMon to finish."`.
9. Any install error: log it and open the release page with
   `error.localizedDescription`.

`Failure` descriptions:

* `download(m)`: "Could not download the update: m"
* `archive(m)`: "Could not read the update: m"
* `verification(m)`: "The update failed verification: m"
* `install(m)`: "Could not install the update: m"
* `notWritable`: "TokenMon cannot replace itself from this folder."

`canReplaceRunningApp(bundleURL)`: the extension is `.app`, the path is not
translocated, and both the parent folder and the bundle are writable.

**Menu row** (`UpdateAvailableIndicator`): shown only while `availableRelease` is
set. It is a plain tinted caption button with the icon `arrow.down.circle`:

* label "TokenMon <v> is available", or "Installing TokenMon <v>…" while installing
* tooltip "Download, verify, and install the update"
* action `performPrimaryAction`, disabled when `!canAct`
* `statusMessage` shown below it as caption2

---

## 4. Overview feature

### 4.1 Data model

* `OverviewSeries` (persisted raw values; **do not rename**): `grok`, `grokbot`,
  `cursor`, `claude`, `openCodeGo`, `openCodeZen`.
* `OverviewDaySeries { weights: [OverviewSeries: [f64;24]], tokens: [OverviewSeries: [i64;24]] }`.
  * Weights are percentage points of each provider's own allowance per hour: Grok
    and Grokbot weekly, Claude its 5-hour window, Cursor and OpenCode the weekly
    share of the monthly plan.
  * `hasActivity`: any weight value is greater than 0.
  * `merging(other)`: per series and per hour, take the max. Arrays of different
    length are padded with 0. A series present on only one side is kept.
  * `addOpenCode(hourly, monthlyLimitUSD, dayStart)`: only when `hourly.dayStart`
    is on the same day and there are 24 hours. It sets
    `weights[.openCodeGo/.openCodeZen]` from
    `hourly.overviewProviderQuotaHourWeights(monthlyLimitUSD)` and the tokens from
    `overviewProviderHourTokenCounts()`. Those formulas are in the OpenCode slice.
* `QuotaNormalization.averageWeeksPerMonth = 365.2425 / 12 / 7`.

### 4.2 Building today's series (`AppModel.overviewSeries(on: date)`)

`dayStart = sod(date)`. A provider contributes only when it is **enabled** and
signed in (`isSignedIn && !needsSignIn`); OpenCode needs only to be enabled.

| Series | Source |
|---|---|
| grok | `grokHourly.hourWeights` if `grokHourly.dayStart` is the same day and there are 24 values |
| claude | `claudeHourly` (same rule) |
| grokbot | `grokbotHourly`, gated on the **Cursor** session |
| cursor | `cursorPoller.dayHourlyUsage` on the same day: `quotaHourWeights` (24) → weights; `hourTokenWeights` (24) → tokens |
| openCodeGo/Zen | `openCodePoller.dayHourlyUsage` via `addOpenCode`. The limit is the monthly window's `limitUSD`, or `OpenCodeWindowKind.monthly.defaultLimitUSD`. |

### 4.3 History persistence (`OverviewHourlyHistoryStore`)

* File `activity_overview_hourly_history.dat`, storage key `overview_hourly_history`.
* `retentionDays = 30`, counting today.
* JSON is written with **sorted keys**. The enum-keyed maps encode as JSON objects:
  ```json
  {"days":{"2026-10-22":{"tokens":{"cursor":[0,…24]},"weights":{"claude":[…24],"grok":[…24]}}},"version":1}
  ```
* Load: decode `days`. Any failure gives an empty history; it does not throw.
* `record(series, at: date)`:
  ```
  key = DayKey(date); changed = false
  if series.hasActivity || days[key] != nil {
     merged = (days[key] ?? empty).merging(series); if merged != days[key] { days[key] = merged; changed = true }
  }
  oldest = DayKey.adding(-(30-1), to: key); if any key < oldest { drop them; changed = true }
  if changed { save() }
  ```
  An empty day is never created.
* `series(on: date)` returns `days[DayKey(date)]`. `earliestDay` is the start of the
  minimum key.
* Wiring: `ObservationLoop(coalescing: 100 ms, initial: true)` tracks
  `overviewSeries(on: now)` and calls `record(series, at: now)`.

### 4.4 Overview panel

1. Header row:
   * a 24 px TokenMon logo, the title "TokenMon", spacer
   * `v<version>` (default `1.0.0`) in tertiary
   * a settings cog that opens Preferences
2. One card per `settings.visibleUsageProviders`, in order. Every card has:
   * a 14 px logo, a title and a meta label (uppercase)
   * "<round(percent)>% Used"
   * an 8 px capsule track at 12% primary opacity, filled
     `width * clamp(percent)/100` in the accent colour
   * a caption

   | Provider | Title / meta | Percent | Caption | Sign-in / out |
   |---|---|---|---|---|
   | Grok | "Grok" "— Weekly" | `snapshot.usedPercent` | `resetCaption(resetsAt)`, or "No Grok data yet" when there is no snapshot | Sign In when `!isSignedIn \|\| needsSignIn`; Sign Out when signed in and `!needsSignIn` |
   | OpenCode | "OpenCode" "— Monthly" | monthly window `usedPercent` ?? `snapshot.monthlyUsedPercent` ?? 0 (purple fill) | monthly `resetsAt` caption, or "No OpenCode data yet" | Sign In when `needsSignIn && snapshot == nil`; Sign Out when `!needsSignIn` |
   | Cursor | "Cursor" "— Monthly" | `usedPercent` | reset caption / "No Cursor data yet" | same rule |
   | Claude | "Claude" "— Weekly" | `sevenDay.usedPercent` | `sevenDay.resetsAt` / "No Claude data yet" | same rule |
   | ChatGPT | "ChatGPT" "— Codex" | `headlineUsedPercent` | reset / "No ChatGPT data yet" | same rule |
   | Grokbot | "Grokbot" "— Weekly" | `usedPercent` | reset / "No Grokbot data yet" | "Sign In to Cursor…" opens Cursor sign-in when `cursorAuth.needsSignIn && snapshot == nil`; no sign-out |
   | OpenRouter | "OpenRouter" "— Credits" | `usedPercent`. When a snapshot exists but `usedPercent == nil`, the text shows `usd(keyUsageUSD)` instead of "% Used". | no budget source: "No credit limit — <$> spent on this key"; with `remainingUSD`: "<$> of credits left"; no snapshot: "No OpenRouter data yet" | Sign In (selects the OpenRouter tab) when `needsSignIn && snapshot == nil`; Sign Out when `!needsSignIn` |
3. **Daily usage card.** It is re-evaluated at every local midnight using an explicit
   schedule:

   `midnightSchedule(from: date) = [sod(date) + k days for k in 0...7]`

   That is 8 entries, the first being today's midnight, each one a local midnight
   (DST-safe).

### 4.5 Daily usage card and chart

**Card**

* `dayOffset` starts at 0. Today's data comes live from `todaySeries(now)`; earlier
  days come from `history.series(on: day)`.
* Title: offset 0 gives "Usage Today"; −1 gives "Usage Yesterday"; any other offset
  gives `"Usage " + format(day, "EEE, MMM d")`.
* `usage = ProviderDayHourlyUsage.build(dayStart, series)`. Pass nil when it is
  empty.
* Empty message: today gives "No Grok, Grokbot, OpenCode, Cursor, or Claude activity yet today.";
  past days give "No activity recorded on this day.".
* Navigation is shown when `canGoBack || dayOffset < 0`.
  * `canGoBack(day, earliest)` = `earliest != nil && sod(earliest) < sod(day)`.
  * Back decrements the offset (enabled only when `canGoBack`).
  * Next sets `min(0, offset + 1)` (enabled when `offset < 0`).

**`ProviderDayHourlyUsage.build`**

* Every input array is padded or truncated to 24. Each hourly delta is
  `max(0, w)`.
* `activity` is the sum of the six deltas: grok, openCodeGo, openCodeZen, cursor,
  claude, grokbot.
* Each provider's share is `delta / activity * 100`, or 0 when there is no activity.
* Tokens: openCodeGo, openCodeZen and cursor use `max(0, t)`. grok and claude tokens
  are optional (nil when not supplied). The Overview path never supplies them, so
  only Cursor and OpenCode show tokens.
* `maxActivity` is the max over hours; the day is empty when `maxActivity <= 0`.
  Each hour has `hasActivity = activity > 0`.

**Chart**

* Track height 68 pt, stem width 12, corner radius 5. The total frame height is
  68 + 19.
* Visible hours: `min(6, minActiveHour)…max(22, maxActiveHour)`. Without
  activity, 6…22.
* Bar height:
  ```
  fraction = activity > 0 && max > 0 ? min(1, activity/max) : 0
  height = fraction > 0 ? min(track, max(3, track*fraction)) : 0
  ```
  The max is taken over the visible hours.
* Stack order, bottom to top: grok, grokbot, cursor, claude, openCodeGo,
  openCodeZen. Only slices with activity > 0 are drawn.
* Slice heights with `segmentHeights(acts, total, minimum: 1)`:
  ```
  sum = Σ max(0,a); if sum<=0 || total<=0 → zeros
  h_i = max(0,a_i)/sum*total; shown = indices with h>0
  if shown.count>1 && minimum*shown.count <= total:
     lift each shown h<minimum to minimum, accumulate lifted; subtract lifted from tallest shown slice
  ```
* An empty hour shows a 2 pt capsule at 12% primary opacity.
* Axis labels: `axisHours(range) = {lower, upper} ∪ {h in range : h % 3 == 0 && h - lower >= 2 && upper - h >= 2}`.
  A label shows the hour as 12 for 0 and 12, `h-12` for 13–23, and `h` otherwise.
* Hover tooltip:
  `"<hourLabel>: " + segments.joined(" · ")`, each segment being
  `"<Title> %.2f%% <quotaLabel>"` plus `" · <Format.tokens(t)>"` when there are
  tokens and t > 0.
  * Titles: Grok, Grokbot, Cursor, Claude, OpenCode Go, OpenCode Zen.
  * quotaLabel: grok and grokbot give "of weekly"; claude gives "of 5-hour window";
    cursor and openCode give "of weekly share".
  * Grokbot never shows tokens.
* Legend: the providers that have activity in the visible hours, in stack order,
  each an 8×10 swatch with a title (Grok, Grokbot, Cursor, Claude, Go, Zen).
* Colours: provider accent colours; OpenCode Go is orange at 0.85 opacity and
  OpenCode Zen is purple at 0.78.

---

## 5. Threshold notifier

`evaluate(provider, usedPercent, settings, account?, resetsAt?)`:

```
guard settings.alertWatches(provider) else return              // enabled && provider ∈ alertProviders
threshold = settings.thresholdPercent
scope = provider == .grok ? (account ?? "default") : "<provider.rawValue>.<account ?? "default">"
key = "thresholdNotified." + scope            // Double: last notified threshold
periodKey = "thresholdNotifiedResetsAt." + scope   // Double: Unix seconds of resetsAt
last = defaults[key]
if last != nil && resetsAt != nil {
   recorded = defaults[periodKey]
   if recorded != nil && resetsAt.unix > recorded + 3600 {     // later period (1 h slack)
       remove key, periodKey; last = nil
   } else if recorded == nil {
       defaults[periodKey] = resetsAt.unix                     // adopt the first period seen
   }
}
if usedPercent < threshold {
   if last != nil && usedPercent < last - 5 { remove key, periodKey }   // re-arm (strict <)
   return
}
if last != nil && last >= threshold { return }                 // already notified at/above this threshold
defaults[key] = threshold; if resetsAt { defaults[periodKey] = resetsAt.unix }
deliver(ThresholdAlert(provider, usedPercent, threshold))
```

* The comparison `usedPercent >= threshold` fires at exactly the threshold.
* Raising the threshold above the recorded one fires again, because `last < threshold`.
* Re-arming compares against the **recorded threshold**, not the usage. With
  threshold 75: 81 fires; 74 keeps quiet (74 is not below 70); 69 clears the record;
  76 then fires again.
* State survives relaunch and is scoped per provider and per account.
* Notification:
  * title "TokenMon Alert"
  * body `"%@ is at %.0f%% (threshold %.0f%%)."`, where %@ is
    "Weekly SuperGrok usage" for Grok and "<displayName> usage" otherwise
  * default sound
  * id `"<provider.rawValue>-usage-threshold-<Int(threshold)>"`
  * immediate trigger
* Example: Grok at 85.4 with threshold 80 gives
  "Weekly SuperGrok usage is at 85% (threshold 80%).".
* Permission is requested at launch (not in tests) only when the status is not yet
  determined, with options alert and sound. A denial is logged.
* Wiring: for every registered provider, an
  `ObservationLoop(initial: true, coalescing: 0)` tracks `thresholdEnabled`,
  `thresholdPercent`, `alertProviders`, the poller's `headlineUsedPercent`,
  `headlineResetsAt` and `alertAccount`. It calls `evaluate` when
  `headlineUsedPercent` is non-nil.

---

## 6. App lifecycle

### 6.1 Launch (the `AppModel.init` order matters)

1. `defaults = makeDefaults()`: standard, or the emptied test suite.
2. `settings = AppSettings(defaults)`. This runs the graph-flag migration and the
   clamps on load.
3. `notifier = ThresholdNotifier(defaults)`.
4. Create the auth sessions. Each session's init triggers:
   * the legacy directory move (`ModelMonitor` → `TokenMon`) on first
     `AppSupport.directory()`
   * the file-to-Keychain secret migration (`SecretRoutingCredentialStore.init`)
   * the vault load and separate-item adoption, on first read
5. Create the stores:
   * `GrokHistoryStore` (imports the legacy `history.store` into `grok_history.dat`
     once; other slice)
   * hourly stores: `grok_hourly_today`, `claude_hourly_today`, `grokbot_hourly_today`
   * daily stores: `claude_daily_usage`, `cursor_daily_usage`, `grokbot_daily_usage`
   * `OverviewHourlyHistoryStore`

   Each store rewrites a legacy payload format once on load.
6. Create the pollers:
   * Grok
   * OpenCode
   * Cursor (daily: cursor)
   * Claude (hourly + daily)
   * ChatGPT
   * OpenRouter
   * Grokbot (on the **Cursor** auth; hourly + daily)
7. `UpdateChecker(settings)`.
8. `ProviderRegistry` of `(provider, poller, auth)`. Grokbot's auth is the same
   object as Cursor's. `all` is in `MonitorProvider.allCases` order.
   `pollers(sharingSessionWith: p)` returns every entry whose auth object is
   identical.
9. Start the observers:
   * **Overview** (100 ms, initial).
   * **checksForUpdates** (initial false): `updateChecker.settingChanged()`.
   * **Polling inputs** (100 ms, initial false): tracks
     `usageProviders.map(needsPolling)`, `activePollSeconds` and `idlePollSeconds`.
     On change, `wake()` every loop.
   * **menuBarGraphProviders** (initial false): diff against the last set; for each
     newly added provider, `refreshNow()` its poller.
   * **Usage alerts**, one per provider (initial true).
10. Real app only: `notifier.requestAuthorizationIfNeeded()`,
    `providers.startAll()` (each loop refreshes now or parks),
    `updateChecker.start()`, the termination observer (flushes `grokHistory` on quit),
    and `SystemWakeGate(onSleep: pause all loops, onReady: resume all loops)`.
11. `AppDelegate.applicationDidFinishLaunching`:
    * Dismiss any window the system auto-presented, then do so once more on the next
      run-loop turn. A window is dismissed when it is visible, can become key and is
      not a non-activating panel. Nothing is dismissed once the user has opened a
      window themselves.
    * `LegacyCacheCleanup.runOnce()` (not under tests).
    * `applicationShouldTerminateAfterLastWindowClosed = false`.

Constants: `changeCoalescing = 100 ms`; `menuOpenFreshness = 15` seconds.

### 6.2 Menu open and close (`setMenuOpen(isOpen)`)

* Set `poller.menuIsOpen` for **every** poller. Each poller wakes its loop when the
  value changes, so the interval switches between active and idle.
* When opening (not in tests): for each provider where `needsPolling`, call
  `loop.refreshNow(ifOlderThan: 15)`.

### 6.3 Sign-in completed (`signInCompleted(p)`)

`refreshNow()` every poller sharing `p`'s session. A Cursor sign-in refreshes both
Cursor and Grokbot. Then `hideDockIfNoWindows()`.

### 6.4 Windows and dock (macOS presentation)

* `revealWindow()`:
  * `userOpenedWindow = true`, `revealGeneration += 1`
  * activation policy `.regular`, so the dock icon shows
  * activate the app

  It runs before opening Settings or a sign-in window.
* `hideDockIfNoWindows()`:
  * Capture the generation, then after **0.25 s**, if the generation is unchanged
    and no visible activating window remains, set the activation policy to
    `.accessory`, which hides the dock icon.
  * Called on Settings or sign-in disappear and after sign-in completes.

### 6.5 Wake handling

See 1.7. Sleep pauses every loop and keeps `lastRefreshAt`. On wake the gate waits
for a satisfied network path (or 30 s), then resumes. A resumed loop refreshes at
once if it is overdue.

### 6.6 Account events (provider pollers)

* `signedOut`: `clearSnapshot()`. History is kept.
* `accountReset`: `clearSnapshot()` and clear the hourly and daily stores.
* Invalidation after 3 rejections: the snapshot is kept and `lastError` is shown;
  history is kept.

### 6.7 Quit

The Settings "Quit TokenMon" button terminates the app. The termination observer
flushes `grokHistory`. Other stores write synchronously on each change.

---

## 7. Test parity checklist

The **Dep** column shows what a test depends on: `P` means it depends on
provider-slice types, which the Rust port can satisfy with fakes or the real
provider crates; `mac` means it is macOS-only and has a suggested Windows analog.

### App

| File | Test | Asserts | Dep |
|---|---|---|---|
| App/AppModelPollingTests | testMenuOpenStateReachesEveryPoller | `setMenuOpen(true/false)` sets `menuIsOpen` on every registered poller | P |
| | testSessionSharingFollowsTheRegistry | `pollers(sharingSessionWith: cursor)` and `(grokbot)` have 2 each; claude has 1 | P |
| | testSignInWindowFollowsTheSession | grokbot.signInProvider == cursor; every other provider maps to itself | |

### Core/Networking

| File | Test | Asserts |
|---|---|---|
| AuthenticatedRequestTests | testBuildsRequestWithCookieHeader | Cookie, `Accept: application/json` and Referer are set |
| | testAddsBearerTokenWhenPresent | `Authorization: Bearer tok123` |
| | testUnauthorizedStatusMapsToUnauthorized | 401 and 403 give `.unauthorized` |
| | testTooManyRequestsMapsToRateLimitedWithRetryAfterSeconds | 429 with `Retry-After: 120` gives `rateLimited(120)`; `.retryAfter == 120` |
| | testTooManyRequestsWithoutRetryAfter | 429 alone gives `rateLimited(nil)` |
| | testRetryAfterParsesHTTPDate | "Wed, 21 Oct 2015 07:28:00 GMT" at now 07:27:00 gives 60 |
| | testRetryAfterIsClamped | "99999999999", "inf" and "Fri, 31 Dec 9999 23:59:59 GMT" give 3600; "-5" gives 0; "nan" gives nil |
| | testCancelledRequestThrowsCancellationError | A cancelled in-flight request throws a cancellation, not a network error |
| | testRateLimitedProviderErrorKeepsRetryAfter | `ProviderError(.rateLimited(30), .cursor)` keeps retryAfter and reads "Cursor is rate limiting requests. Retrying later." |
| | testNon2xxMapsToBadResponse | 500 gives `badResponse("HTTP 500")` |
| | testNon2xxDoesNotLeakResponseBody | The body's secret is not in the message |
| | testNoErrorOnSuccessRange | 200 and 204 give nil |
| | testForbiddenHTMLIsTransientChallenge | 403 with an HTML body gives the bot-challenge badResponse |
| | testCloudflareMitigatedHeaderIsTransientChallenge | 403 with a `cf-mitigated` header and a JSON body gives the challenge badResponse |
| | testForbiddenJSONAndUnauthorizedStillRejectSession | 403 with JSON gives unauthorized; 401 with HTML gives unauthorized |
| | testTwoHundredHTMLIsUnauthorizedOnlyForLoginPages | `jsonObject` on a login page gives `.unauthorized(.cursor)`; on a challenge page or a maintenance page it gives `badResponse("Unexpected HTML response")` |
| | testProviderSessionHasNoCookieJarOrCache | No cookie storage, no cookie setting, accept policy never, no URL cache |
| | testAppliesAppUserAgent | `User-Agent == AppIdentity.userAgent` |
| | testStaticURLParsesLiteral | host == grok.com |
| UsageErrorTests | testLocalizedDescriptions | Every case has a non-empty description |
| | testEquality | Equality includes the payload |
| | testClientMappingsToCommonAuthCases | `ProviderError.unauthorized`/`notSignedIn` give the matching `usageError` (cursor, openCode, grok) |

### Core/Polling

| File | Test | Asserts |
|---|---|---|
| BackoffTimerTests | testStartsAtZero | current == 0 |
| | testDoublesGeometrically | 30, 60, 120 |
| | testCapsAtMaximum | 10 or more failures give 600 and stay there |
| | testResetOnSuccess | (15, 300): 15, 30, then reset to 0 |
| PollingLoopTests (manual clock and sleeper) | testRefreshesImmediatelyThenSleepsTheFullInterval | start runs 1 refresh then sleeps 300; firing runs refresh 2 then sleeps 300 |
| | testNilIntervalParksWithoutATimerUntilWoken | nil: 0 refreshes and 0 pending; set 60 + wake: refresh and sleep 60; nil + wake cancels the pending wait |
| | testWakeWithShorterIntervalRefreshesWhenOverdue | after 100 s, interval 60 + wake: immediate refresh, sleep 60, 1 pending |
| | testWakeRearmsTheRemainingWaitWhenNotYetDue | after 20 s, interval 60 + wake: sleep 40, no refresh |
| | testRefreshNowRestartsTheWaitFromThatRefresh | after 250 s, refreshNow gives refresh 2 and the next sleep is 300 |
| | testFailuresBackOffExponentiallyAndSuccessResets | interval 10, failures: sleeps 30, 60, then 10 after a success |
| | testRetryAfterIsHonoured | failure(retryAfter 500) gives a sleep of 500 |
| | testSkippedRefreshKeepsTheInterval | skipped gives a sleep of 10 |
| | testJitterStretchesTheWaitAndIsClampedToTenPercent | jitter 0.1 gives 330; −0.5 is clamped and gives 270 |
| | testStopCancelsThePendingWait | 0 pending, 1 cancelled, 1 refresh |
| | testPauseCancelsTheWaitAndResumeRefreshesWhenOverdue | pause gives 0 pending; resume after 1000 s refreshes |
| | testDeinitCancelsTheWait | A sleeping loop is not retained; its wait is cancelled |
| | testOutcomeClassifiesCancellationAndRateLimits | Cancellation gives skipped; rateLimited(42) gives failure(42); network gives failure(nil) |
| ProviderRefreshTests | testSuccessPublishesAndResetsTheStreak | success, published [7], recordAuthSuccess once, no errors |
| | testResultForAStaleGenerationIsDropped | A generation bumped during the fetch gives skipped; nothing published; no success |
| | testRejectedSessionCountsTowardTheStreak | unauthorized gives failure(nil), onRejected once, recordAuthFailure once, 1 error |
| | testOtherFailuresDoNotCountAgainstTheSession | URLError gives failure; no rejection or streak; 1 error |
| | testFailureForAStaleGenerationIsIgnored | skipped; no streak; no error reported |
| | testCancellationIsSkipped | skipped; no error |
| PollerSessionGuardTests (P) | testGrokStaleUnauthorizedDoesNotInvalidateNewSession | sign-out + new session during the fetch, then 401: still signed in |
| | testGrokCurrentUnauthorizedInvalidatesSession | 3 rejections in a row (signed in after each of the first 2): `needsSignIn`, not signed in |
| | testGrokStaleTransportErrorDoesNotClobberNewSession | A stale timeout leaves `lastError` nil and the session signed in |
| | testCursorStaleSuccessIsDropped | snapshot stays nil |
| | testCursorStaleUnauthorizedDoesNotInvalidateNewSession | still signed in |
| | testCursorCurrentUnauthorizedInvalidatesSession | invalidated after 3 |
| | testClaudeStaleUnauthorizedDoesNotInvalidateNewSession / testClaudeCurrentUnauthorizedInvalidatesSession | as above |
| | testChatGPTStaleUnauthorizedDoesNotInvalidateNewSession / testChatGPTCurrentUnauthorizedInvalidatesSession | as above |
| | testChatGPTTransientUnauthorizedRecoversOnNextPoll | after one 401, the counter is 1 and the session is signed in; a success resets it to 0 and a snapshot exists |
| | testChatGPTFailureAfterDataReportsError | success (lastError nil), then network failure (snapshot kept, lastError set, session still signed in), then success (lastError nil) |
| | testChatGPTRefreshedCookieIsPersisted | Set-Cookie renewal plus `_ga` gives the stored header `__Secure-next-auth.session-token=renewed` |
| | testOpenRouterStaleUnauthorizedDoesNotInvalidateNewKey / testOpenRouterCurrentUnauthorizedInvalidatesKey | same semantics for the API key |
| | testGrokbotStale… / testGrokbotCurrentUnauthorizedInvalidatesSession | on the Cursor session |
| | testOpenCodeStaleSuccessIsDropped | snapshot nil |
| | testOpenCodeLateSuccessDoesNotRestoreWorkspaceID | the workspace stays cleared after sign-out |
| | testOpenCodeStale/CurrentUnauthorized… | as above |
| | testOpenCodeLocalEstimatePublishesWhenStartedSignedOut | snapshot set; `dataSourceLabel == "Local estimate"` |
| | testOpenCodeLocalEstimateSuppressedAfterMidPollSignOut | snapshot nil |

### Core/Storage

| File | Test | Asserts |
|---|---|---|
| AppSupportTests | testDirectoryIsCreatedAsDirectory | The directory exists and is a directory |
| | testDefaultUsesExpectedSubdirectory | last component "TokenMon", parent is the base |
| | testTestHostUsesTemporaryBaseDirectory | Under tests the base is under tmp, not the real Application Support |
| | testMovesModelMonitorDirectoryWhenCurrentIsAbsent | legacy is moved; `auth_session.dat` is present in the new folder |
| | testDoesNotOverwriteExistingCurrentDirectory | both folders are kept; no merge |
| | testRealHomeDirectoryIsTheUserHome | equals the home directory, from any thread |
| | (AppModelTestIsolationTests) testTestHostUsesThrowawayDefaultsSuite | test defaults are separate from standard |
| LegacyCacheCleanupTests (mac) | testRemovesOnlyCacheFilesAndRecordsCompletion | Only the 4 cache items are removed; others and siblings are kept; flag set |
| | testRunsOnlyOnce | The second run returns false and deletes nothing |
| | testMissingCacheFolderStillCompletes | true and the flag is set |
| | testClearsTheGivenURLCache | The injected cache is emptied |
| | testRemovesOnlyThisAppsCookieJarFiles | The jar and its `_tmp_*.dat` are deleted in Cookies and HTTPStorages; other jars, `.bak` and subfolders are kept |
| | testCookieJarStepRunsOnceOnItsOwnFlag | Runs even when the cache flag is set; never twice |
| | testClearsTheGivenCookieStorage | `removeCookies(since: distantPast)` is called |
| DailyQuotaDeltaStoreTests | testGrowthIsAttributedToTheSampledDay | 10 then 25 gives 15 today |
| | testWindowResetAttributesPostResetValueToTheSampledDay | 40, 60, 25 gives 20 + 25 = 45 |
| | testGrowthContinuesAfterWindowReset | 60, 25, 30 gives 30 |
| | testTinyDeltaIsIgnoredAsNoise | 10 then 10.01 gives 0 |
| | testSmallDownwardNoiseIsNotCreditedAsReset | 50 then 49.9 gives 0 |
| | testOldDaysArePrunedToRetentionWindow | day −41 is pruned; today is 11; count 1 |
| | testBeginNewWindowClearsDaysAndNextSampleIsCreditedAsReset | after begin, empty; 25 gives 25 |
| | testBeginNewWindowCreditsRisingFirstSample | 20, begin, 30 gives 30 |
| | testReloadRestoresPersistedDaysAndBaseline | 10, 22, reload, 27 gives 17 |
| | testEarlyResetPreservesPriorDaysAndCreditsFirstPostResetSample | earlyReset; day −2 = 20, day −1 = 20, today = 4; windowStart today 08:00; interrupted = day −3 08:00 |
| | testEarlyResetBaselineCreditsFirstSampleWhole | 40, then (new window) 8, 11 gives 11 today; baseline 11 |
| | testEarlyResetDropsOnlyTheBoundaryDay | 8 recorded today in the old window, then a reset at 2 gives today = 2; day −1 kept at 20 |
| | testNormalRolloverStillStartsFreshWindowAndKeepsEarlierDays | rollover; day −1 = 40; today = 3; windowStart today 08:00; interrupted nil |
| | testDropWithUnchangedWindowIsARebaseNotAnEarlyReset | none; interrupted nil |
| | testMovedWindowWithoutUsageDropIsNotAReset | none; today = 2 |
| | testEarlyResetWithoutPeriodStartDerivesStartFromResetInstant | resets_at only: earlyReset; day −1 = 25; windowStart = new reset − 7 days; interrupted = old reset − 7 days |
| | testWindowMetadataSurvivesRelaunchSoAnEarlyResetWhileClosedIsStillSeen | reload keeps `windowResetsAt`; the reset is detected; fields persist |
| | testPayloadWithoutWindowMetadataDecodes | legacy `{days:[…], lastUsedPercent}` loads; first windowed sample gives none and delta 5 |
| | testClearDropsWindowMetadata | all window fields nil; days empty |
| | testClassifyNeedsProviderMetadataOnBothSides | missing resetsAt on either side gives none |
| | testClassifyEarlyResetRequiresAUsageDrop | 60→2 earlyReset; 60→58 none; nil previous none |
| | testClassifyIgnoresResetInstantJitter | +30 s gives none; +3 days gives earlyReset |
| | testClassifyLateEarlyResetIsNotMistakenForRollover | old reset tomorrow, drop 90→1, moved: earlyReset |
| | testClassifyRolloverNeedsOldResetPassedAndHalfPeriodAdvance | +7 days gives rollover; +1 day gives none |
| | testUnchangedSampleDoesNotRewriteTheFile | the file is not rewritten for the same state; it is rewritten after a change |
| | testInstantKeyedPayloadLoadsAndIsRewritten | Denver: the flat `days` array loads; rewritten with `"2026-09-14":7` and `"days":[` |
| | testInstantKeyedPayloadWithWindowMetadataLoads | window fields as ref-seconds (813093922.556 and so on) |
| | testDaysKeepTheirDatesAfterATimeZoneChange | recorded in Denver, read in Tokyo: "2026-09-15" = 6 |
| | testPruneKeepsFortyDaysInTheInjectedCalendar | the day is kept at +31 days and pruned at +41 days |
| HourlyDeltaActivityStoreTests | testGrowthWithinAWindowIsAttributedToTheSampledHour | hour 9 = 15 |
| | testWindowResetAttributesPostResetValueInsteadOfDroppingIt | 95 then 15: hour 10 = 15 |
| | testTinyDeltaIsIgnoredAsNoise / testSmallDownwardNoiseIsNotCreditedAsReset | 0 |
| | testDifferentKeysDoNotCollideOnDisk | A = 10, B = 1 |
| | testClearResetsSeriesAndBaseline | zeros; next sample sets the baseline (0); then +7 |
| | testBeginNewWindowCreditsRisingFirstSample | 30 |
| | testBeginNewWindowKeepingHoursPreservesTodaysSeries | hour 9 = 15 kept; hour 11 = 4 |
| | testUnchangedSampleDoesNotRewriteTheFile | as for the daily store |
| | testFirstDeltaAfterMidnightIsCredited | 23:55 → 00:05 gives hour 0 = 7; dayStart is the new midnight |
| | testGapOfSeveralDaysStartsWithoutBaseline | a 2-day gap gives a total of 3 (only the later delta) |
| | testInstantKeyedPayloadRestoresTodayAndIsRewritten | weights restored; +3 at hour 15; rewritten with `"dayKey":"2026-10-10"` |
| | testInstantKeyedPayloadFromAnotherDayStartsFresh | sum 0 |

### Core/Usage

| File | Test | Asserts |
|---|---|---|
| DayKeyTests | testKeyRoundTripsThroughStartOfDay | Denver ref 811144800 + 15 h gives "2026-09-15"; the start round-trips; "garbage" gives nil |
| | testAddingDaysCrossesMonths | −40 from 2026-10-10 gives 2026-08-31; +1 from 2026-12-31 gives 2027-01-01 |
| | testStoredStartOfDayKeepsItsDateAcrossZones | Denver midnight read in 5 zones gives "2026-09-15" |
| | testStoredStartOfDayFromEasternAndHalfHourZones | Tokyo, Kolkata, Auckland and LA round-trip 2026-07-16 |
| DailyTokenUsageTests | testAddMergesModelsAndSortsByTokens | total 116; cost 3.5; models [B:100, A:16] |
| | testUnpricedUsageIsFlagged | cost nil plus flag; empty model name counts only toward totals |
| | testTallyGroupsByLocalDayAndAttachesToBars | UTC day split; attaches to the matching bar only (P for the next two) |
| | testOpenCodeDailyTokens | 2 rows give 668 tokens, cache 600, cost 1 |
| | testCursorEventsRollUpPerDay | chargedCents 150 gives $1.5; model tokens 334 |
| UsagePoolTests (P) | testCombiningCollapsesCadences, testSectionTitles, testGrokIsWeekly, testGrokbotPoolsFollowPeriodLength (7 days weekly, 30 days monthly, 14 days mixed), testClaudePools, testChatGPTPools, testCursorIsMonthly, testOpenCodeMixedPools, testOpenCodeSingleWindowKeepsItsCadence | cadence mapping |

### Core/Utilities

| File | Test | Asserts |
|---|---|---|
| FormatTests | testTokens* (7 tests) | the examples in 1.5 |
| | testUSDFormatsCurrencyWithoutTilde | "$4.10", "$0.00" |
| | testHourLabelTwelveHourClock | 0 "12a", 12 "12p", 5 "5a", 11 "11a", 13 "1p", 15 "3p", 23 "11p" |
| | testParseFlexiblePlainAndFractional | "…20:25:00Z" gives 1784233500; ".123Z" gives +0.123 |
| | testParseFlexibleRejectsGarbage | garbage, date-only and zone-less all give nil |
| | testParseFlexibleHandlesOffsetsAndMicroseconds | −06:00, +0530 and .123456+00:00 |
| | testResetDateMatchesNaiveFormatterAndLowercasesMeridian | "EEE h:mma" ends in "pm" |
| | testResetDateUsesTheRequestedTimeZone | UTC "18:57"; Tokyo "03:57" |
| | testResetDateDefaultsToCurrentTimeZone | a nil zone uses the current zone |
| | testSystemTimeZoneChangeClearsFormatterCache | the cache count goes to 0 after a zone-change notification |
| JSONTests | testNumberFromVariousTypes, testNumberRejectsBooleansAndNonFiniteValues, testNumberDescendsIntoValAndValueDicts, testString, testNestedTraversesKeys, testFirstDoubleTriesKeysInOrder, testFirstString, testFirstDecimal, testFirstDecimalKeepsMoneyPrecision, testFirstBool | the rules in 1.5 (`"1e400"` nil, `{"val":"0.1"}` gives exactly 0.1, `"1.5abc"` nil, numeric 0/1 accepted as a bool) |
| ObservationLoopTests | testHandlesTheFirstReadingAndEachChange | [0] then [0, 1]; an unrelated property does not trigger |
| | testBurstIsHandledOnceWithTheLatestValue | 30 ms coalescing: 1, 2, 3 gives [3] |
| | testCancelStopsHandling | no calls after cancel |

### Platform

| File | Test | Asserts |
|---|---|---|
| SystemWakeGateTests | testWakeWaitsForASatisfiedPath | sleep, wake, false: only "sleep"; then true gives "ready"; the fallback is cancelled |
| | testPathThatCameBackDuringWakeResumesImmediately | sleep, path true, wake: ready at once |
| | testFallbackResumesWhenThePathNeverChanges | 30 s fallback gives ready |
| | testPathChangesOutsideAWakeDoNothing | no events |
| ThresholdNotifierTests | testFiresWhenCrossingThreshold, testDoesNotFireBelowThreshold, testDoesNotRefireAtSameThreshold, testReArmsAfterFivePointDrop, testExactBoundaryCountsAsCrossing | the `shouldNotify` rules |
| | testEvaluateFiresOnceAndDoesNotRefireWhileAbove | [85] |
| | testEvaluatePersistsAcrossInstances | a new instance stays quiet |
| | testEvaluateIsAccountScoped | 2 accounts give 2 alerts |
| | testEvaluateIsProviderScoped | keys `thresholdNotified.a@b.com` and `thresholdNotified.claude.a@b.com` |
| | testNotificationBodyNamesTheProvider | exact bodies |
| | testEvaluateReArmsAfterFivePointDrop | 81 fires; 74 quiet; 69 clears; 76 fires |
| | testEvaluateNoOpWhenDisabled | nothing |
| | testEvaluateReArmsForNewBillingPeriod | +300 s is the same period; +7 days fires again; then quiet |
| | testEvaluateAdoptsPeriodForRecordWithoutOne | a record without a period adopts the first one seen |
| | testEvaluateSkipsProvidersTheAlertDoesNotWatch | only claude fires |
| Auth/CredentialStoreTests | testInjectedStoreBacksSession, testSignOutRemovesCredentialThroughStore, testFileBackedCredentialStoreDelegates | the store seam |
| | testFilePermissionsAreUserOnly | mode 0600 (Windows: owner-only ACL) |
| | testMigrationMovesSecretFileIntoSecureStore | the session moves; email stays in the file |
| | testMigrationFailureKeepsFileAndKeepsWorking | the file stays authoritative |
| | testSecureWriteRemovesLeftoverFile | |
| | testProvidersShareOneVaultItem | one item only |
| | testVaultCachesReads | reads == 1 |
| | testSeparateItemMovesIntoVault | adoption |
| | testSignedOutAccountIsNotRestoredFromSeparateItem | an undeletable legacy item stays signed out |
| | testRefusedSeparateItemIsNotRetried | no re-read |
| | testRefusedVaultIsNeverOverwritten | set returns false; the item bytes are unchanged |
| | testRemovalWhileVaultUnavailableIsAppliedOnNextLoad | the pending list is applied, then cleared |
| | testKeychainStoreReportsFailedWrite | false |
| | testKeychainStoreVerifiesWriteByReadingBack | corruption keeps the file |
| | testFileStoreFirstWriteIsAtomicAndLeavesNoStagingFile | only `activity_k.dat` exists |
| | testFileStoreReportsFailedWrite | false for a missing directory |
| Auth/ProviderAuthSessionTests | testStartsWithSignedOutState, testSaveAndLoadAccountEmailPersists, testMarkSessionInvalidClearsState, testGenerationInvalidatesInFlightRefresh, testIsCurrentRequiresUsableSession | 1.6.4 |
| | testLoadNarrowsStoredCookieJarToAllowlist | "sso=z; sso-rw=w", persisted |
| | testLoadKeepsJarWhenEssentialsAbsent | unchanged |
| | testLoadKeepsChunkedEssentialFamily | chunks `.0` and `.1` only |
| | testLoadLeavesJarWhenOnlyNonEssentialPresent | unchanged |
| | testSessionSurvivesRejectionsBelowThreshold, testAuthSuccessResetsRejectionStreak | threshold 3 |
| | testInvalidationDoesNotResetAccountData, testSignOutKeepsAccountDataUntilAnotherAccountSignsIn, testEmailIdentifiedAccountChangeResetsAccountData, testCookieIdentifiedAccountChangeResetsAccountData | `accountReset` rules |
| | testCaptureReplacesPreviousAccountEmail | a nil email removes the stored one |
| | testRefreshedCookiesParseCommaFoldedHeader, testRefreshedDeletionCookieRemovesChunk, testRenewedUnchunkedTokenReplacesStoredChunks, testRefreshedPastExpiryRemovesCookie (result "") | `applyRefreshedCookies` |
| Auth/AuthInvalidationTests (P) | testClaudeInvalidationKeepsHistory, testClaudeFailureAfterDataReportsError, testClaudeSignOutKeepsHistoryUntilAnotherAccountSignsIn, testGrokbotForbiddenDoesNotInvalidateCursorSession (4 × 403-forbidden: counter 0, lastError = forbidden message), testGrokbotKeepsHistoryWhileSignedOut, testGrokInvalidationAndSignOutKeepHourlyDeltas | history retention |
| Auth/WebKitCookieCaptureTests | testEssentialNamesNarrowWhatIsStored, testAllowlistWithoutSessionCookieCapturesNothing, testEmptyAllowlistKeepsBroadCapture, testOtherDomainCookiesAreIgnored | `select` |
| | testExtractsEmailNamedCookie (`dev%40example.com` gives decoded), testExtractsUserEmailNamedCookie, testFallsBackToShortValueContainingAt, testRejectsImplausibleEmailValues (`a@b`, `@example.com`, `user@`, `x@y@z.com`, `token@abc.12`, `user@example.com; path=/`, `"q"@example.com`, `https://u@example.com`, `not-an-email@localhost`), testIgnoresLongValuesWithSpaces, testReturnsNilWithoutEmailLikeCookies, testPrefersExplicitEmailCookieOverFallback | email extraction |
| Auth/ProviderCookieAllowlistTests (P) | testCursorPolicyStoresOnlyTheSessionToken, testClaudePolicyKeepsSessionKeyAndOrg, testOpenCodePolicyKeepsConsoleAndSiteCookies, testForeignDomainCookiesAreIgnored, testGrokPolicyDropsXSessionCookies, testChatGPTPolicyStoresOnlyTheSessionCookie, testChatGPTPolicyKeepsChunkedSessionAndDropsCSRF, testChatGPTPolicyRejectsCSRFOnlyJar, testGrokDomainExcludesXAndTwitter, testGrokPolicyWithoutSessionCookieCapturesNothing | real policies |

### Features

| File | Test | Asserts |
|---|---|---|
| Settings/AppSettingsTests | testActivePollClampedToMinimum / Maximum, testIdlePollClamped | 15/300; 60/3600 |
| | testValuesPersistAcrossInstances | active poll and graph set persist |
| | testPanelSectionsDefaultOpenAndPersist, testPanelSectionIgnoresUnknownSavedValues | |
| | testShowSelectedProviderDefaultsOffAndPersists, testThresholdAlertDefaultsOffAndPersists | |
| | testNeedsGrokPollingFollowsBarAndProvider, testThresholdAlertKeepsEveryProviderPolling, testAlertProvidersLimitAlertPolling, testNeedsOpenCodeAndCursorPollingOnOverview, testNeedsPollingOffWhenBarsHiddenAndOtherTabSelected, testNeedsGrokbotPollingFollowsBarAndProvider, testNeedsClaudePollingFollowsBarAndProvider, testDisabledProviderDoesNotPoll, testDisabledGrokbotDoesNotPollEvenWithBarEnabled, testNeedsPollingFollowsTabAndDefaultGraphs | the `needsPolling` formula |
| | testEnabledProvidersDefaultToAllUsageProviders, testVisibleUsageProvidersFollowUsageProviderOrder, testProviderOrderDefaultsToUsageProviders, testProviderOrderPersistsAndAppendsNewProviders, testMoveProviderInsertsAtTargetSlot, testNormalizedOrderDropsOverviewAndDuplicates, testEnabledProvidersPersistAcrossInstances, testCannotRemoveLastEnabledProvider, testStoredOverviewAndUnknownValuesAreIgnored, testStoredValidSubsetSurvivesLoad, testProviderOrderPersistsTheNormalizedValue | provider list rules |
| | testThresholdPercentIsClampedOnLoad (250 gives 99; −5 gives 50), testThresholdPercentIsClampedOnSet (10 gives 50, persisted) | |
| | testRetiredVisibleProductsFallBackToAllKnown, testEmptyVisibleProductsIsPreserved, testVisibleProductsDropUnknownIds | |
| | testDisablingTheSelectedProviderFallsBackToOverview (persisted), testDisablingAnotherProviderKeepsTheSelection | |
| | testLaunchAtLoginShowsOnWhileAwaitingApproval, testLaunchAtLoginRegistersAndRevertsOnFailure | fake login item |
| | testPerProviderGraphFlagsBecomeTheGraphSet, testGraphSetDefaultsToGrokOnly | migration |
| Update/AppVersionTests | testParsesPlainAndTaggedVersions, testParsesBuildMetadata, testParsesReleaseNamePrefix, testRejectsUnparseableTags, testComparesNumericallyNotLexically, testShorterVersionsPadWithZero, testPrereleaseSortsBelowItsFinalRelease, testPrereleaseComparesNumericIdentifiersNumerically | 3.2 |
| Update/ReleaseFeedTests | testReportsNewerRelease, testSameVersionIsNotAnUpdate, testOlderVersionIsNotAnUpdate, testDraftsAndPrereleasesAreIgnored, testMissingPageURLFallsBackToReleasesPage, testArchiveURLPrefersTokenMonZipOnGitHub, testUntrustedArchiveIsIgnored, testAssetDigestAndInstallerPackageAreRead (uppercase hex is lowercased), testDSYMArchiveIsNotChosen, testMalformedDigestIsDropped, testTrustedDownloadIsLimitedToThisRepositorysReleases (4 trusted / 8 untrusted URLs; the CDN URL is not a "repository download"), testUnparseableTagThrows | 3.3 |
| Update/UpdateCheckerTests | testNewerReleaseIsPublished, testSameOrOlderReleaseIsIgnored, testDraftAndPrereleaseAreIgnored, testRateLimit403ReportsFriendlyError, testServerErrorIsReported, testMissingBundleVersionSkipsCheck (0 requests), testManualCheckRunsWhenAutomaticChecksAreOff ("You're up to date."), testManualCheckSurfacesANewerRelease (actionTitle "Update to 1.5.1…"), testChecksDisabledSkipsRequest, testNotWritableInstallOffersInstallerPackage (opens `.pkg`, 1 request), testMissingDigestOpensReleasePageInsteadOfInstalling | 3.4 / 3.5 |
| Update/AppInstallerTests (mac) | testInstallsVerifiedUpdateInPlace, testChecksumMismatchIsRefused, testWrongBundleIdentifierIsRefused, testDowngradeIsRefused, testSameVersionIsRefused, testUnsignedAppIsRefused, testAppModifiedAfterSigningIsRefused, testSymbolicLinkInArchiveIsRefused, testAppNestedBelowTopLevelIsRefused, testDownloadOutsideThisRepositoryIsRefused (0 requests), testReadOnlyInstallFolderIsRefused (notWritable, 0 requests), testDownloadStopsAtSizeLimit, testDownloadReturnsSHA256OfBody, testCanReplaceRunningAppChecksTheGivenBundle, testDeclaredUncompressedBytesParsesZipinfoSummary | After every failure the installed app is untouched and has no siblings |
| Overview/OverviewHourlyHistoryTests | testOpenCodeHoursAreAddedForTheSameDay, testOpenCodeHoursFromAnotherDayAreIgnored (P), testRecordKeepsTheLargerValueForEachHour, testAnEmptyDayIsNotSaved, testHistoryReloadsFromDisk, testDaysPastTheRetentionWindowAreDropped (Oct 31 drops Oct 1), testSeriesKeysAreSavedByName, testSegmentHeightsAreProportionalAndSumToTheBar ([1,3]/40 gives 10/30), testThinSlicesAreLiftedWithoutOverflowingTheBar (gives 1, 1, 18), testSlicesStayProportionalWhenTheBarIsTooShortForMinimums, testBarHeightFollowsActivityWithAVisibleMinimum (30, 60, 3, 0), testVisibleHoursWidenToIncludeLateAndEarlyActivity, testAxisLabelsMatchTheDefaultDayAndNeverCrowd ({6,9,12,15,18,22} and {2,6,9,12,15,18,21,23}), testDayTitles, testBackArrowNeedsAnEarlierSavedDay | section 4 |
| Overview/ProviderHourlyUsageTests | testShortArraysArePaddedToTwentyFourHours, testLongArraysAreTruncatedToTwentyFourHours, testMidnightScheduleStartsAtTodayThenEachLocalMidnight (8 entries; DST-safe; Denver 2026-03-08) | |

Support fixtures:

* `ManualScheduler`: a manual clock plus a sleeper resolved with `fire()`; tracks
  `pendingCount`, `cancelledCount`, `nextSleep()` and `settle()`.
* `NeverRespondingURLProtocol`.
* `ObservationProbe`.

Rust needs the same fakes: an injectable clock and sleeper, an HTTP mock
(`wiremock` or `httpmock`), and a recording toast sink.

---

## 8. macOS-specific dependencies and suggested Windows equivalents

| macOS dependency | Used for | Suggested Windows / Rust equivalent |
|---|---|---|
| `NSStatusItem` + borderless `NSPanel`, `LSUIElement` | Menu-bar item and dropdown | `tray-icon` + a borderless always-on-top popup window (`tao`/`winit` or WebView2/egui) positioned near the tray; no taskbar button for the popup |
| Activation policy `.regular`/`.accessory` (dock show/hide, 0.25 s grace) | Dock icon only while a window is open | Not needed. Settings and sign-in windows show a taskbar button while open; keep `revealWindow`/`hideDock` as no-ops. Add a **single-instance** guard (named mutex `Local\TokenMon`), which macOS gets for free. |
| SwiftUI auto-presented first window and its dismissal | Launch shows only the tray | Simply do not open any window at startup |
| `UserDefaults.standard` | `AppSettings`, notifier records, cleanup flags, pending removals | JSON file `%LOCALAPPDATA%\TokenMon\settings.json`, written atomically, or `HKCU\Software\TokenMon`. Keep the exact key names and types from section 2 so the tests port unchanged. Tests use a temp-dir store. |
| `~/Library/Application Support/TokenMon` (0700), files 0600, `rename(2)` atomic writes | All `*.dat` stores | `%LOCALAPPDATA%\TokenMon\` (device-local, matching "ThisDeviceOnly") via `dirs::data_local_dir()`. Restrict the DACL to the current user (owner-only, no inheritance) on the folder and files. Atomic write: write a temp file in the same dir, then `MoveFileExW(MOVEFILE_REPLACE_EXISTING \| MOVEFILE_WRITE_THROUGH)` or `ReplaceFileW` (`tempfile::NamedTempFile::persist`). |
| `ModelMonitor` → `TokenMon` folder move | Legacy migration | Not applicable. Keep `migrate_legacy_directory_if_needed(from, to)` as a generic helper (it is tested). |
| Keychain (`SecItem*`, generic password, `AfterFirstUnlockThisDeviceOnly`), one vault item | Secrets (`session` cookies, OpenRouter `key`) | **Windows Credential Manager** (`CredWriteW`, `CRED_TYPE_GENERIC`, `CRED_PERSIST_LOCAL_MACHINE`, target `com.modelmonitor.app.credentials/vault`) via the `keyring` crate. **Caveat:** the credential blob is limited to 2 560 bytes (`CRED_MAX_CREDENTIAL_BLOB_SIZE`), and a vault with several cookie headers (ChatGPT's chunked NextAuth JWT) can exceed that. Recommended: a DPAPI-protected vault file (`CryptProtectData`, user scope, optional entropy) at `%LOCALAPPDATA%\TokenMon\vault.bin` behind the same `KeychainBackend` trait (read/add/update/delete with status codes), keeping the vault semantics (read-back verify, `unavailable` lock-out, pending removals). Separate-item adoption is not needed. |
| `HTTPCookieStorage.shared`, `URLCache.shared`, `LegacyCacheCleanup` | Removing the legacy shared jar and cache | Not applicable; `reqwest` keeps no jar or cache. Omit, or keep a no-op with the defaults flags. |
| `WKWebsiteDataStore.nonPersistent()` per provider; cookie read and delete | Sign-in browser and cookie capture | **WebView2** (`webview2-com`/`wry`). Use a separate `CoreWebView2Environment` user-data folder per provider in a temp directory (deleted on sign-out), or InPrivate (`IsInPrivateModeEnabled`). Read cookies with `CookieManager.GetCookiesAsync("")` filtered by `isDomain`, and delete them with `DeleteAllCookies`. Keep capture retries (4 × 400 ms). |
| `NWPathMonitor` | Network-ready after wake | `INetworkListManager` events (`INetworkListManagerEvents::ConnectivityChanged`), or `NotifyNetworkConnectivityHintChange` / `NotifyIpInterfaceChange` (Win 10 2004+). "Satisfied" means IPv4 or IPv6 internet connectivity. |
| `NSWorkspace.willSleep/didWake` notifications | `SystemWakeGate` | `RegisterSuspendResumeNotification` or `WM_POWERBROADCAST` on a hidden window: `PBT_APMSUSPEND` maps to `systemWillSleep`; `PBT_APMRESUMEAUTOMATIC` / `PBT_APMRESUMESUSPEND` map to `systemDidWake` (deduplicate, since both can arrive). Also handle Modern Standby (`GUID_CONSOLE_DISPLAY_STATE` / `PowerRegisterSuspendResumeNotification`). |
| `NSSystemTimeZoneDidChange` | Stores re-derive `spentByDay`/`dayStart`; formatter cache cleared | `WM_TIMECHANGE` (and `WM_SETTINGCHANGE` "intl") on the hidden window. Re-read the zone (`iana-time-zone` + `chrono-tz`, or `GetDynamicTimeZoneInformation`) and broadcast an internal "tz-changed" event. Never cache the local offset. |
| `Calendar.autoupdatingCurrent` | Local day and hour bucketing | `chrono::Local` (re-queried) or a `Tz` resolved per call. Day arithmetic must be **calendar-day** based (DST-safe), as in `DayKey.startOfDay` via noon. |
| `UNUserNotificationCenter` | Threshold alerts | Windows toast via WinRT `ToastNotificationManager` (`windows` crate or `winrt-notification`/`tauri-winrt-notification`). Requires an AppUserModelID (set `SetCurrentProcessExplicitAppUserModelID("TokenMon")` and register a Start-menu shortcut with that AUMID in the installer). Title "TokenMon Alert", same body, tag = the request id. A notification-permission prompt is not needed. |
| `SMAppService.mainApp` (launch at login, `requiresApproval`) | Launch at Login toggle | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\TokenMon = "<exe path>"`. "Needs approval" maps to the user having disabled it in Task Manager: check `HKCU\...\Explorer\StartupApproved\Run\TokenMon` (first byte `0x03` = disabled). Show "Disabled in Task Manager › Startup apps." with a button that opens `ms-settings:startupapps`. Keep `register`/`unregister`/`status` behind the `LoginItemService` trait. |
| `SMAppService.openSystemSettingsLoginItems()` | "Open Login Items…" | `ShellExecute("ms-settings:startupapps")` |
| `NSWorkspace.shared.open(url)` | Opening release pages and pkg downloads | `ShellExecuteW` / the `open` crate |
| `NSSavePanel` | History export | `IFileSaveDialog` via the `rfd` crate |
| `NSApp.terminate` | Quit | Exit the event loop; run the flush hooks (`grokHistory.flush`) |
| `Bundle.main` `CFBundleShortVersionString`, bundle id | User-Agent, update compare | `env!("CARGO_PKG_VERSION")`; keep the identifier `com.modelmonitor.app` as a constant (also the keyring service) |
| `os.Logger` (subsystem `com.modelmonitor.app`) | Logging | `tracing` + a rolling file appender in `%LOCALAPPDATA%\TokenMon\logs`. Never log secrets. |
| `getpwuid_r` real home | OpenCode DB path | `dirs::home_dir()` / `SHGetKnownFolderPath(FOLDERID_Profile)` |
| `@Observable` + `withObservationTracking` (`ObservationLoop`) | Reactive wiring | An explicit change bus: settings and pollers emit `Changed(field)` events on a channel; each "loop" subscribes to the fields it reads, debounces (`tokio::time::sleep(100ms)` reset on each event) and re-reads. Keep the `initial` flag semantics. |
| `@MainActor`, `Task`, `Task.sleep` | Concurrency | A single UI-thread state owner plus a `tokio` runtime. `PollingLoop` takes an injected `Clock` + `Sleeper` (trait) so the manual-scheduler tests port. Cancel with `AbortHandle`/`CancellationToken`. |
| `URLSession` ephemeral, cancellation becomes `CancellationError` | HTTP | `reqwest` (rustls or native-tls) without cookies. Map a dropped or aborted future to a `Cancelled` error that is classified `Skipped`. |
| GitHub releases `.zip` with `TokenMon.app`, `.pkg` | In-app update | A Windows asset, for example `TokenMon-<ver>-windows-x64.zip` (containing `TokenMon.exe`) and/or `.msi`. **`preferredAsset` must be adapted:** prefer `tokenmon*` names with the Windows suffix, skip `dsym`/`pdb`, and use the `.msi` as the "installer package" fallback. Keep the digest (`sha256:`), the trusted-URL rules and the size caps (80 MiB download, 256 MiB extracted). |
| `zipinfo` / `ditto` | Extraction and size checks | The `zip` crate: sum the declared `size()` values before extracting (cap 256 MiB). Reject entries with absolute paths, `..`, symlink attributes (Unix mode `S_IFLNK` in external attributes) and reparse points. Require `TokenMon.exe` at the top level. Re-walk the extracted tree, rejecting `FILE_ATTRIBUTE_REPARSE_POINT` and enforcing the size cap. |
| Code signature (`SecStaticCode`, requirement, team id) | Verification | **Authenticode**: `WinVerifyTrust(WINTRUST_ACTION_GENERIC_VERIFY_V2)` on the staged exe (and DLLs). Compare the signer certificate subject or thumbprint with the running exe's signer, the analog of the team id. Unsigned dev builds verify the version resource only (`VerQueryValue` `ProductName == "TokenMon"`, `FileVersion` greater than the current one), mirroring the ad-hoc path. |
| `com.apple.quarantine` xattr | Refusing quarantined ad-hoc builds | Mark-of-the-Web, the `Zone.Identifier` ADS. The app's own download has none; refuse an **unsigned** build that carries `:Zone.Identifier` with `ZoneId=3`, because SmartScreen would block it. |
| App Translocation, writability of the `.app` and its folder | `canReplaceRunningApp` | Running from a non-writable location (`Program Files` without elevation) or from a temp/zip-extract folder means a manual install: download the `.msi` or open the release page. Check that the exe's directory is writable (try creating a temp file). |
| `FileManager.replaceItemAt` (atomic swap) | Swap the app | A running exe cannot be overwritten, but it can be **renamed**. Stage to `<dir>\.TokenMon-update-<uuid>\`, verify, rename `TokenMon.exe` to `TokenMon.old-<uuid>.exe`, move the new exe into place (`MoveFileExW`), and roll back on failure. Delete `*.old-*.exe` on next launch. For a multi-file app, swap the whole folder through a helper. |
| `/bin/sh` relaunch loop (`kill -0` every 0.2 s, then `open`) | Relaunch after the update | Spawn the new exe with `--wait-for-pid <pid>`. It calls `OpenProcess(SYNCHRONIZE)` + `WaitForSingleObject` (or polls every 200 ms), then continues startup. Then exit the current process. |
| XCTest detection (`XCTestConfigurationFilePath`) | Test isolation | `cfg(test)` plus constructor injection of the base directory, settings store, credential store and clock |

Files to keep identical in name and format on Windows, inside the TokenMon data
folder: every `activity_*.dat` (Hourly, Daily and Overview JSON as specified, with
dates in **ref-seconds**), `<prefix>email.dat`, `<prefix>account.dat` and
`opencode_auth_workspace.dat`. Secrets live in the DPAPI vault or Credential
Manager and must never be written to `.dat` files, except through the failure
fallback in 1.6.2.
