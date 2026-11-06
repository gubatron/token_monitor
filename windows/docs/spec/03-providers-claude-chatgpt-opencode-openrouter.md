# Spec 03 — Providers: Claude, ChatGPT, OpenCode, OpenRouter

Source of truth: `TokenMon/Providers/{Claude,ChatGPT,OpenCode,OpenRouter}/*.swift` and
`TokenMonTests/Providers/{Claude,ChatGPT,OpenCode,OpenRouter}/*.swift` on branch `windows-port`
(HEAD `3ac5c6b`). This document lets a Rust implementer reach behavioural parity without reading
Swift. Shared Core pieces (ProviderHTTP, ProviderAuthSession, PollingLoop, DailyBudget,
DailyQuotaDeltaStore, HourlyDeltaActivityStore, Format, DayKey, credential stores, cookie capture)
are specified by other spec files; they are referenced here by name with only the behaviour these
providers depend on.

Conventions:

- "Bars" = the Daily Budget bar chart (`NavigableDailyBudgetCard`).
- `DayKey` = `yyyy-MM-dd` string of a calendar day in a given calendar/time zone (zero-padded,
  4-digit year).
- All `$/1M` prices are USD per one million tokens.
- "Percent.clamp(x)" = `min(100, max(0, x))`.
- "JSON.number(v)" = accepts a JSON number (not a boolean), or a numeric string (whitespace-trimmed,
  parsed as f64), or an object `{"val": …}` / `{"value": …}` (recursively); rejects NaN/±Inf;
  anything else → none.
- "ISO8601 flexible" = RFC 3339 / ISO-8601 internet date-time **with or without fractional
  seconds**, timezone required (`Z` or `±hh:mm`). Fractional precision kept.
- Default request headers on every provider request built by `ProviderHTTP`/`AuthenticatedRequest`:
  `Accept: application/json`, `User-Agent: TokenMon/<app version>`, optional `Cookie`, optional
  `Authorization: Bearer <token>`, optional `Referer`. Ephemeral HTTP client: no cookie jar, no cache,
  redirects followed.
- Shared response mapping (Core): bot challenge (`cf-mitigated` header present, or 403 with HTML body)
  → transient `badResponse("Request blocked by a bot-protection check (HTTP <s>). Retrying.")`;
  401/403 → `unauthorized`; 429 → `rateLimited(retryAfter)`; other non-2xx → `badResponse("HTTP <s>")`;
  transport → `network`.
- Three-strike rule (Core, `authFailureThreshold = 3`): 3 consecutive `unauthorized`/`notSignedIn`
  on the live session generation invalidate the session; any success resets the count.

---

## 0. Cross-provider summary

| Provider | Credential | Store prefix / keys | Headline % (menu bar / alerts) | headlineResetsAt | alertAccount | Daily Budget bars |
|---|---|---|---|---|---|---|
| Claude | claude.ai cookies `sessionKey`, `lastActiveOrg` | `claude_auth_` : `session` (secret), `email`, `account` (= lastActiveOrg value) | `(fiveHour ?? sevenDay).usedPercent ?? 0` | `fiveHour.resetsAt ?? sevenDay.resetsAt` | `auth.accountEmail` | Weekly window, 7 bars, from `DailyQuotaDeltaStore("claude_daily_usage")` + Claude Code tokens on hover |
| ChatGPT | chatgpt.com `__Secure-next-auth.session-token[.N]` → bearer access token | `chatgpt_auth_` : `session`, `email`, `account` | `(primary ?? secondary).usedPercent ?? 0` | `primary.resetsAt ?? secondary.resetsAt` | `auth.accountEmail` | none |
| OpenCode | opencode.ai cookies `auth`, `provider`, `__Host-console_session` (+ `wrk_…` workspace id) | `opencode_auth_` : `session`, `email`, `account`, `workspace` (file, not secret) | `monthlyUsedPercent` (monthly window, else weekly, else 0) | always `nil` | `auth.accountEmail` | Monthly subscription period, Monday-week bars, local SQLite spends scaled to headline |
| OpenRouter | API key `sk-or-…` | `openrouter_auth_` : `key` (secret) | `snapshot.usedPercent` (nil when no budget) | always `nil` | always `nil` | none |

Every poller: `performRefresh` is skipped (returns `.skipped`) if the provider is disabled in
settings or a refresh is already in flight (`isRefreshing`). Poll interval comes from
`settings.pollInterval(for: <provider>, menuIsOpen:)` (Core). Setting `menuIsOpen` to a different
value wakes the polling loop. `refreshNow()` = `pollingLoop.refreshNow()`.

---

## 1. Claude

### 1.1 Auth (`ClaudeAuthSession`, `ClaudeSignIn`)

| Item | Value |
|---|---|
| Hosts (`claudeHosts`) | `claude.ai`, `www.claude.ai`, `api.claude.ai` (domain match = exact host or dot-suffix) |
| Sign-in start URL | `https://claude.ai/new` |
| Sign-in window title / subtitle | `Sign in to Claude` / `Sign in to your Claude account. This window finishes on its own once you return to claude.ai.` |
| `isAuthHost(host, path)` (allowed to load, never captured) | `host` contains `clerk.claude` OR `accounts.google` OR `github.com` OR `appleid.apple`; OR `path` contains `/login` OR `/signin` |
| `isReturnPage(url)` (triggers capture) | host (lowercased) is exactly `claude.ai` or `www.claude.ai`, AND lowercased path does **not** contain `login`, `signin`, or `/auth` |
| Preferred session cookie | name == `sessionKey` (case-sensitive) |
| `looksLikeAuthCookie` | lowercased name == `sessionkey` or `lastactiveorg`, or contains any of `session`, `token`, `auth`, `claude` |
| `includeAllDomainCookiesWhenSessionFound` | true |
| Essential cookie allowlist (case-insensitive) | `sessionkey`, `lastactiveorg` |
| Capture `maxAttempts` | 4 |
| Capture failure message | `No Claude session cookie found. Finish signing in to claude.ai, then click Capture Session.` |
| Store filename prefix / log category | `claude_auth_` / `ClaudeAuth` |
| `extraStoreKeys` | none |
| Sign-out hosts | same as hosts |
| Account identity | value of cookie `lastactiveorg` (case-insensitive name) from the capture; a capture with a different non-empty value than the stored `account` fires `accountReset` (clears hourly + daily history) |
| Email | only from the shared capture (email-shaped cookie value, Core); Claude never fetches an email |

Sign-out (Core `ProviderAuthSession.signOut`): bumps `sessionGeneration`, removes `session` and
`email`, clears provider browser cookies, sets `needsSignIn = true`, fires `signedOut`. The
`account` identity is kept.

### 1.2 HTTP endpoint

| Field | Value |
|---|---|
| Method / URL | `GET https://claude.ai/api/organizations/{org}/usage` |
| `{org}` | `ClaudeUsageClient.organizationID(fromCookieHeader:)` (below), inserted verbatim (no URL-encoding) |
| Headers | defaults + `Cookie: <stored pruned header>` + `Referer: https://claude.ai/` |
| No org id | throw `ProviderError.custom(message: "Claude organization id not found in session. Sign in again.", usage: .notSignedIn)` — counts as a strike |
| Error context | `.claude` (shared mapping) |

`organizationID(fromCookieHeader:)`: split header on `;`; for each pair split on the first `=`;
need exactly 2 parts; name = part0 trimmed of spaces/tabs, must equal **`lastActiveOrg`
(case-sensitive)**; value = part1 trimmed; empty value → `nil`; first match wins; none → `nil`.

### 1.3 Response parsing (`ClaudeUsageResponse.parse`)

Body decode uses Core `ProviderHTTP.jsonObject`: body must be a JSON object; otherwise:
HTML whose first 64 KiB (Latin-1, lowercased) contains a login marker (`type="password"`,
`type='password'`, `sign in`, `sign-in`, `signin`, `log in`, `log-in`, `login`, `authkit`, `workos`)
and no Cloudflare challenge marker → `unauthorized(.claude)`; other HTML →
`badResponse("Unexpected HTML response")`; non-HTML → `badResponse("Malformed response body")`.
(HTML = first 512 bytes, UTF-8, trimmed, starts with `<`.)

Fields:

```
five_hour: { utilization: number, resets_at: ISO8601 string } | null
seven_day: { utilization, resets_at } | null
limits: [ { kind, group: "session"|"weekly"|..., percent: number, resets_at: string|null,
            scope: null | { model: { id: string|null, display_name: string|null }, surface },
            is_active: bool } ]
```

Algorithm:

1. `limits` = `root.limits` if it is an array of objects, else `[]`.
2. `fiveHour = window(root.five_hour) ?? limitWindow(limits, "session")`.
3. `sevenDay = window(root.seven_day) ?? limitWindow(limits, "weekly")`.
4. `window(raw)`: raw must be an object and `JSON.number(raw.utilization)` must exist (else none) →
   `usedPercent = clamp(utilization)`, `resetsAt = ISO8601flexible(resets_at)` if it is a string
   (numbers not accepted), `scopeName = nil`.
5. `limitWindow(limits, group)`: candidates = entries where `group` is a string equal to `group`
   AND `JSON.number(percent)` exists. Rank each as tuple `(unscoped ? 1 : 0, is_active === true ? 1 : 0, percent)`;
   pick the lexicographic max (on ties the **first** encountered wins). Result:
   `usedPercent = clamp(percent)`, `resetsAt` as above, `scopeName = scopeName(scope)`.
6. `scopeName(scope)`: scope must be an object with `model` object; returns `model.display_name`
   (string) else `model.id` (string) else nil. Missing/null scope or model → unscoped.

`ClaudeUsageWindow.remainingPercent = clamp(100 - usedPercent)`.

### 1.4 Snapshot model (`ClaudeSnapshot`)

| Property | Rule |
|---|---|
| `fetchedAt` | time of fetch (client returns `now`) |
| `fiveHour`, `sevenDay` | from response |
| `accountEmail` | `auth.accountEmail` at publish |
| `resetsAt` | `fiveHour?.resetsAt ?? sevenDay?.resetsAt` |
| `weeklyLabel` | `"Weekly · <scopeName>"` when `sevenDay.scopeName` set, else `"Weekly"` (separator is U+00B7 with spaces) |
| `headlineUsedPercent` | `(fiveHour ?? sevenDay)?.usedPercent ?? 0` |
| `usagePool` | `UsagePool.combining([fiveHour? .hourly, sevenDay? .weekly])` → one → that name, 0 or 2 → `mixed`. Section title `"Usage Pool <Hourly|Weekly|Mixed>"` |

### 1.5 Poller (`ClaudeUsagePoller`)

State: `snapshot`, `isRefreshing`, `lastError`, `lastRefreshedAt`, `dailyBudgetDays`,
`codeUsage: ClaudeCodeUsageSummary?`, `localDailyTokens: [DayKey: [model: ClaudeModelTokens]]`,
private `weeklyResetsAt` (last seen this run), `budgetReferenceNow`.

Dependencies: `HourlyDeltaActivityStore(storageKey: "claude_hourly_today")`,
`DailyQuotaDeltaStore(storageKey: "claude_daily_usage")`, `ClaudeCodeUsageLedger()` (default store
prefix `claude_code_`).

Subscriptions: `signedOut` → `clearSnapshot()`; `accountReset` → `clearSnapshot()` + `hourly.clear()`
+ `daily.clear()`.

`clearSnapshot()`: snapshot, dailyBudgetDays, weeklyResetsAt, budgetReferenceNow, lastError,
lastRefreshedAt → nil. **`codeUsage` and `localDailyTokens` are kept** (local data survives sign-out).

`performRefresh()`:

1. Gate: provider enabled and not refreshing; set `isRefreshing`.
2. **Always** load Claude Code usage first: `loadCodeUsage(now)` (background, utility priority):
   `scans = ClaudeCodeLocalStats.scanDailyTokens(calendars: [UTC ledger calendar, current local calendar])`;
   if `nil` (no projects dir) or count != 2 → `nil`. Else
   `ClaudeCodeUsageScan(summary: ClaudeCodeUsageSummary.make(days: ledger.merge(scans[0]), now, calendar: UTC), localDailyTokens: scans[1])`.
   If the task was cancelled → `.skipped`. Set `codeUsage = scan?.summary`, `localDailyTokens = scan?.localDailyTokens ?? [:]`.
   If bars already exist, re-attach tokens to them.
   (Test seam: when a fake `fetchUsage` is injected and no `loadCodeUsage`, the loader returns nil.)
3. Cookie header = `auth.cookieHeader()` (pruned to allowlist). Missing/empty → `auth.needsSignIn = true`;
   if `snapshot == nil` set `lastError = "Sign in to Claude to load usage."`; return `.skipped`.
4. `ProviderRefresh.run(label: "Claude")` with fetch = GET usage. On success (still current generation):
   - `auth.recordAuthSuccess()` (inside run), `snapshot = ClaudeSnapshot(...)`, `lastError = nil`,
     `lastRefreshedAt = now`, `auth.needsSignIn = false`.
   - If `fiveHour` present: `hourly.record(usedPercent: fiveHour.usedPercent, at: fetchedAt)`
     (the 5-hour window drives the hourly activity series because it counts every model).
   - If `sevenDay` present: remember `weeklyResetsAt = sevenDay.resetsAt` (only when non-nil);
     `daily.record(windowUsedPercent: sevenDay.usedPercent, at: fetchedAt, window: QuotaWindow(start: nil, resetsAt: sevenDay.resetsAt))`.
   - `budgetReferenceNow = fetchedAt`;
     `dailyBudgetDays = attachTokens(buildDailyBudgetDays(spentByDay: daily.spentByDay, resetsAt: weeklyResetsAt(now: fetchedAt), windowStart: daily.windowStart, interruptedWindowStart: daily.interruptedWindowStart, now: fetchedAt))`.
   - return `.success`.
   On failure: unauthorized/notSignedIn → strike; `lastError = error.localizedDescription`; backoff outcome.

`weeklyResetsAt(now)`: `snapshot.sevenDay.resetsAt` if present; else
`projectWeeklyReset(weeklyResetsAt ?? daily.windowResetsAt, now)`.

`projectWeeklyReset(r, now)`: nil → nil; `r > now` → `r`; else
`weeks = floor((now - r) / 604800) + 1`; return `r + weeks * 604800` (so `r == now` → `r + 1 week`).

`buildDailyBudgetDays(spentByDay, resetsAt, windowStart?, interruptedWindowStart?, weekOffset = 0, now, calendar = current)`:
`resetsAt == nil` → `[]` (never substitute a rolling window). Else
`DailyBudget.buildWeeklyWindowDays(limitUSD: 100, daysInPeriod: 7, …)` (Core). Values are
percent-of-weekly-pool; each bar's budget = `100/7`.

`dailyBudgetDays(weekOffset:)`: rebuilds from the live daily store with
`now = budgetReferenceNow ?? Date()` and attaches tokens (used by week arrows).

`attachTokens(byDay, days, calendar = current)`: if `byDay` empty → unchanged. Else for each bar,
key = `DayKey(bar.date, calendar)`; if `byDay[key]` exists set `bar.tokens = DailyTokenUsage(claudeModels:)`
(below), else leave `tokens = nil`. `spentUSD` untouched.

`DailyTokenUsage(claudeModels:)`: for each `(modelID, counts)` call
`add(model: ClaudeModelName.display(modelID), input, output, cacheRead, cacheWrite: 5m + 1h, costUSD: ClaudeAPIPricing.cost(counts, modelID))`
(Core `DailyTokenUsage.add`: sums tokens; `costUSD` accumulates when non-nil; a nil cost with
tokens > 0 sets `hasUnpriced`; per-model token totals merged by display name and sorted tokens desc,
then name asc).

### 1.6 Claude Code local transcripts (`ClaudeCodeLocalStats`, `ClaudeCodeFileCache`)

#### 1.6.1 Directories

`projectsDirectories(environment, home)`:

- If env `CLAUDE_CONFIG_DIR` is set and non-empty after trimming spaces: split on `,`, trim each,
  drop empties, expand a leading `~`, append `/projects` → list (in order).
- Else: `[<home>/.config/claude/projects, <home>/.claude/projects]` (both scanned).
- `home` = real user home (macOS: `getpwuid_r`, not the sandbox container).

Windows mapping (flag: verify on a real install): home = `%USERPROFILE%` (prefer
`SHGetKnownFolderPath(FOLDERID_Profile)`); candidates `%USERPROFILE%\.config\claude\projects` and
`%USERPROFILE%\.claude\projects`; `CLAUDE_CONFIG_DIR` honoured identically (comma-separated;
expand `~` to `%USERPROFILE%`). Claude Code running inside WSL writes under the Linux home
(`\\wsl$\<distro>\home\<user>\.claude`), which is out of scope unless added deliberately.

`scanDailyTokens(directories, calendars, cache)`: keep only directories that exist **and are
directories**; none → return `nil` (means "Claude Code not installed": the panel hides the Claude
Code section). Otherwise `cache.update(directory:)` for each, then return
`calendars.map { cache.dailyTokens(calendar:) }`.

#### 1.6.2 Walk (`ClaudeCodeFileCache.update(directory:)`)

- Recursive walk under the directory, **skipping hidden files and hidden directories** (name starts
  with `.`). Consider files whose extension is exactly `jsonl` (case-sensitive) and that are regular
  files with a modification date. Nested subfolders (e.g. `session/subagents/agent.jsonl`) are included.
- Per file, metadata: `modified` (mtime), `size`, `fileID` (macOS `fileIdentifier`, i.e. inode;
  may be nil).
- After the walk, prune cached items whose `root` equals this directory and whose path was not seen;
  if anything was pruned, set `needsRebuild = true`.

Windows: fileID ≈ volume serial + file index (`GetFileInformationByHandle`; Rust std's
`MetadataExt::file_index` is unstable — use `windows-sys`/`same-file`), or `None`. Symlinked
directories are not followed on macOS (FileManager enumerator default); match that.

#### 1.6.3 Per-file incremental read (`refresh`)

Cached `Item { root, modified, size, fileID, parsedBytes, entries }` keyed by full path.

1. If cached and `modified`, `size`, `fileID` all equal → nothing to do.
2. **Append path**: if cached, same `fileID`, `cached.parsedBytes > 0`, `size >= cached.parsedBytes`,
   and `readAppended(url, from: parsedBytes)` succeeds → parse `parseCompleteLines(appended)`;
   item becomes `parsedBytes += consumed`, `entries += new`, new metadata; the new entries are
   appended to the shared `added` log. Done.
3. `readAppended(offset)`: open, seek to `offset - 1`, read to EOF; the first byte must be `\n`
   (0x0A), else return nil (file was rewritten); return the bytes after it.
4. **Full path** (new file, shrunk file, replaced file, or previous read didn't end on a newline):
   read whole file (memory-map if safe; unreadable → empty), `parseCompleteLines`; store item with
   `parsedBytes = consumed`. If the file was new (`cached == nil`) append its entries to `added`;
   otherwise set `needsRebuild = true`.

`parseCompleteLines(data)`: `tailStart` = index after the last `\n` (or 0). Tail = bytes after it.
If the tail is empty or **not** a complete JSON value → parse only `data[..tailStart]` and
`consumed = tailStart` (a partly written last line waits for the next read). If the tail parses as
JSON → parse everything and `consumed = len(data)` (note: the next read then fails the newline
check and re-parses the file in full; this is intentional parity).

#### 1.6.4 Line parsing

`usageLineRanges(data)`: split raw bytes on `\n` only (`\r` stays in the line); keep lines that
contain the byte sequence `"usage"` (7 bytes, quotes included). Only those lines are JSON-decoded
(performance: transcripts reach GBs).

`parseLine(line)` — strict typed decode of only these fields (any other field ignored):

| JSON path | Type | Use |
|---|---|---|
| `type` | string? | must equal `"assistant"` |
| `timestamp` | string? | required; ISO8601 flexible; unparseable → skip |
| `requestId` | string? | dedupe |
| `message.id` | string? | dedupe |
| `message.model` | string? | model id; missing → `"unknown"`; `"<synthetic>"` → skip line |
| `message.usage` | object? | required |
| `message.usage.input_tokens` | number? | input |
| `message.usage.output_tokens` | number? | output |
| `message.usage.cache_read_input_tokens` | number? | cache read |
| `message.usage.cache_creation_input_tokens` | number? | total cache write |
| `message.usage.cache_creation.ephemeral_1h_input_tokens` | number? | 1-hour cache write |

Type mismatch on any of these declared fields (e.g. `input_tokens` is a string, `type` is a number,
`message.id` is a number) makes the **whole line** invalid → skipped (Swift `JSONDecoder` semantics).
`null` = absent. `ephemeral_5m_input_tokens` is ignored.

`count(v)`: absent/NaN/Inf/≤0 → 0; else `Int64(min(v, Int64.max))` (truncate toward zero).
`cacheWrite = count(cache_creation_input_tokens)`; `cacheWrite1h = min(cacheWrite, count(ephemeral_1h))`;
`cacheWrite5m = cacheWrite - cacheWrite1h` (without `cache_creation`, all writes are 5-minute).
`dedupeKey = "<message.id>:<requestId>"` only when both are present, else none.

`Entry { date, model, inputTokens, outputTokens, cacheReadTokens, cacheWrite5mTokens, cacheWrite1hTokens, dedupeKey }`.

#### 1.6.5 Tallies (per calendar) and dedupe

`ClaudeModelTokens { inputTokens, outputTokens, cacheReadTokens, cacheWrite5mTokens, cacheWrite1hTokens: i64; responseCount: int }`,
`totalTokens = sum of the five token fields`; `+` adds fieldwise; `fieldwiseMax` takes max per field
(including `responseCount`).

`ClaudeDailyTokenTally(calendar).add(entry)`: if `dedupeKey` non-nil and already in `seen` → ignore
(dedupe is global across files and days within one tally); else
`days[DayKey(entry.date, calendar)][entry.model] += counts(responseCount: 1)`. Entries without a
dedupe key are always counted. (Day lookups cache the last day's interval — pure optimisation.)

`ClaudeCodeFileCache.dailyTokens(calendar)`:

- If `needsRebuild`: drop all tallies, clear flag.
- If a tally exists for this calendar: apply log entries `added[applied - addedBase ..]`, set
  `applied = addedCount`.
- Else: if there are already **4** tallies (`maxTallies`), drop them all; build a new tally by
  iterating cached items **sorted by path**, each item's entries in file order (this ordering decides
  which duplicate is counted); `applied = addedCount`.
- Trim the `added` log to entries not yet applied by the slowest tally.
- Return `tally.days`.

Calendars are keyed by identity (Gregorian + time zone). Two are used per poll: UTC (ledger) and
the current local zone (bars).

#### 1.6.6 Ledger (`ClaudeCodeUsageLedger`)

Purpose: Claude Code deletes transcripts after ~30 days; the ledger keeps per-day totals forever.

- Day calendar: Gregorian, time zone UTC (local days would double count after a TZ change).
- Storage: `FileBackedStringStore(filenamePrefix: "claude_code_")`, key `daily_tokens` → file
  `claude_code_daily_tokens.dat` in the app data dir (macOS `~/Library/Application Support/TokenMon/`;
  Windows: the Core-chosen app data dir, e.g. `%APPDATA%\TokenMon\`).
- Payload JSON (encoded with **sorted keys**):
  `{"days": {"2026-10-01": {"claude-opus-5-5": {"cacheReadTokens":…,"cacheWrite1hTokens":…,"cacheWrite5mTokens":…,"inputTokens":…,"outputTokens":…,"responseCount":…}}}, "version": 2}`.
  All six count fields are required on decode.
- Load (lazy, first merge): missing → `{}`; JSON decode failure (unreadable, missing field) → `{}`
  (log error); `version != 2` → `{}` (earlier local-day format discarded).
- `merge(scanned)`: `merged = saved ∪ scanned`, for a day/model in both → `fieldwiseMax`. Write the
  file only if `merged != saved`. Cache in memory. Returns merged. Thread-safe (lock).

#### 1.6.7 Summary (`ClaudeCodeUsageSummary.make(days, now, calendar=UTC)`)

- `monthPrefix` = first 7 chars of `DayKey(now, UTC)` (`yyyy-MM`) → month turns over at 00:00 UTC.
- `byModel(days)` sums `ClaudeModelTokens` over days per model id.
- `totals(models)`: `tokens = Σ totalTokens`; for each model `cost = ClaudeAPIPricing.cost`; priced →
  add to `costUSD`; unpriced → `hasUnpriced = true`.
- `month = totals(byModel(monthDays))`, `allTime = totals(byModel(all days))`.
- `monthModels = ranked(byModel(monthDays))`: drop models with `totalTokens == 0`; row
  `{modelID, tokens, costUSD: cost ?? 0, isUnpriced: cost == nil}`; `sharePercent = cost/totalCost*100`
  when `totalCost > 0`, else `tokens/totalTokens*100` when `totalTokens > 0`, else 0. Sort: cost desc,
  then totalTokens desc, then modelID asc.

#### 1.6.8 Pricing (`ClaudeAPIPricing`, `ratesVerifiedOn = "2026-10-06"`)

`baseModelID(id)`: lowercase; if the substring after the last `-` is exactly 8 ASCII digits, drop
`-YYYYMMDD`; else the lowercased id. `rates(for:) = table[baseModelID(id)]`.
Cache writes: 5-minute = `1.25 × input`, 1-hour = `2 × input`.

`cost(tokens, modelID)` = `(in×input + out×output + cr×cacheRead + w5×1.25×input + w1h×2×input) / 1e6`;
unknown model → `nil`.

| # | Model id (base) | Input $/1M | Output $/1M | Cache read $/1M | Cache write 5m $/1M (=1.25×in) | Cache write 1h $/1M (=2×in) |
|---|---|---|---|---|---|---|
| 1 | `claude-fable-5-1` | 10 | 50 | 0.25 | 12.5 | 20 |
| 2 | `claude-fable-5` | 10 | 50 | 1 | 12.5 | 20 |
| 3 | `claude-opus-5-5` | 4 | 20 | 0.20 | 5 | 8 |
| 4 | `claude-opus-5` | 5 | 25 | 0.50 | 6.25 | 10 |
| 5 | `claude-opus-4-8` | 5 | 25 | 0.50 | 6.25 | 10 |
| 6 | `claude-opus-4-7` | 5 | 25 | 0.50 | 6.25 | 10 |
| 7 | `claude-opus-4-6` | 5 | 25 | 0.50 | 6.25 | 10 |
| 8 | `claude-sonnet-5-5` | 2 | 10 | 0.20 | 2.5 | 4 |
| 9 | `claude-sonnet-5` | 2 | 10 | 0.20 | 2.5 | 4 |
| 10 | `claude-sonnet-4-6` | 3 | 15 | 0.30 | 3.75 | 6 |
| 11 | `claude-haiku-5-5` | 0.10 | 0.50 | 0.01 | 0.125 | 0.2 |
| 12 | `claude-haiku-4-5` | 1 | 5 | 0.10 | 1.25 | 2 |

Haiku 5.5 uses its ≤100K-prompt rate for all prompt lengths.

`ClaudeModelName.display(id)`: `baseModelID(id)`, strip leading `claude-`, then
`OpenCodeModelName.display` (§3.9). E.g. `claude-opus-5-5` → `Opus 5.5`, `claude-fable-5-1` →
`Fable 5.1`, `claude-haiku-4-5-20251001` → `Haiku 4.5`.

### 1.7 Panel (`ClaudePanelView`)

Branches:

1. `auth.needsSignIn && snapshot == nil` → **signed-out**: header row (Claude logo, title
   `Claude`); body text `Sign in to claude.ai to load your 5-hour and weekly usage.` (secondary);
   button `Sign In to Claude…`; then the Claude Code section (if `codeUsage` non-nil).
2. `snapshot != nil` → **main**, vertical spacing 10:
   - Header row `Claude`.
   - Card: section header `Usage Pool <Hourly|Weekly|Mixed>`;
     track **`5-Hour Window`**, percent = `fiveHour.usedPercent ?? 0`, Claude accent, caption =
     `Format.resetCaption(fiveHour.resetsAt)` when present (format `Resets EEE dd MMMM h:mma`, POSIX
     locale, local zone, `AM/PM` → `am/pm`, e.g. `Resets Thu 27 August 11:00am`);
     track **`snapshot.weeklyLabel`**, percent = `sevenDay.usedPercent ?? 0`, caption from
     `poller.weeklyResetsAt()` (live or projected).
   - `NavigableDailyBudgetCard(style: .weekly, accent: Claude, periodUsedPercent: sevenDay.usedPercent, resetsAt: poller.weeklyResetsAt(), daysForWeek: poller.dailyBudgetDays(weekOffset:))`.
     Hidden when the day list is empty. Week arrows: back unbounded, forward stops at 0. Hover label
     noun `weekly`.
   - Claude Code section (if `codeUsage`).
   - If `lastError`: stale caption (`<message>` + `Showing data from <relative> ago.` when
     `lastRefreshedAt` known), top padding 8.
   - If `auth.needsSignIn`: button `Sign In Again…`.
   - Sign-out button (icon with confirm step; Core UI).
3. Else (no snapshot, not needing sign-in): header; text `Refreshing…` if refreshing, else
   `lastError`, else `No usage data yet.`; sign-in button if `needsSignIn`; Claude Code section.

**Claude Code section** (`ClaudeCodeUsageSection`, shown only when `codeUsage != nil`, i.e. a
projects dir exists, regardless of claude.ai sign-in):

- 2×2 metric grid: `Tokens this month` = `Format.tokens(month.tokens)`; `Spend this month` =
  `spend(month)`; `Total tokens` = `Format.tokens(allTime.tokens)`; `Total spend` = `spend(allTime)`.
- `spend(t) = Format.usd(t.costUSD) + (t.hasUnpriced ? "+" : "")` → e.g. `$12.50`, `$12.50+`.
- If `monthModels` non-empty: card with collapsible header **`Models This Month`** (expanded state
  persisted in settings key `claudeModels`); when expanded, one row per model with dividers
  (opacity 0.35) between: company logo (by model id), `ClaudeModelName.display(modelID)` (semibold,
  1 line), right-aligned value `No price` if unpriced else `Format.usd(costUSD)`; below a 6-pt capsule
  share bar (track = primary at 12% opacity, fill = Claude accent, width = `clamp(sharePercent)/100`).

`Format.usd` = en_US currency (`$1,234.56`). `Format.tokens`: `<1000` → integer; else thousands
rounded half-away → `NK` if `<1000`; else millions with 1 decimal if rounded `<10` else 0 decimals →
`N.NM`/`NM` if `<1000`; else billions 1 decimal `N.NB` (999,500 → `1.0M`).

**Bars hover label (Claude per-day tokens)** — Core `DailyBudgetHoverLabel`, data from
`localDailyTokens` (local-day calendar, transcripts still on disk; days whose transcripts were deleted
show no token block):

- Title: date `EEEE, MMM d` (e.g. `Monday, Nov 2`).
- Usage line (not for future days): prior-window day → `%.1f%% of the weekly pool before the provider reset it early`;
  budget ≤ 0 → `No budget`; spent ≤ 0.001 → `0% of the %.1f%% daily allowance`; else
  `%.1f%% of weekly · %d%% of daily allowance` (allowance % = round(spent/budget×100)).
- When `tokens.totalTokens > 0`: divider; `<Format.tokens(total)> tokens` and, when `costUSD > 0`,
  right side `≈ $X.XX` plus `+` if `hasUnpriced`; grid rows `Input`, `Output`, `Cache read`,
  `Cache write` (5m+1h); divider; top **3** models by tokens (`ClaudeModelName.display` names) with
  token counts.

---

## 2. ChatGPT (Codex usage)

### 2.1 Auth (`ChatGPTAuthSession`, `ChatGPTSignIn`)

| Item | Value |
|---|---|
| Hosts | `chatgpt.com`, `www.chatgpt.com`, `auth.openai.com`, `auth0.openai.com`, `api.openai.com` |
| Start URL | `https://chatgpt.com/` |
| Title / subtitle | `Sign in to ChatGPT` / `Sign in to your ChatGPT account. This window finishes on its own once you return to chatgpt.com.` |
| `isAuthHost` | host contains `auth.openai` / `auth0.openai` / `accounts.google` / `appleid.apple` / `login.microsoft`; or path contains `/login` / `/signin` |
| `isReturnPage` | lowercased host == `chatgpt.com` or ends with `.chatgpt.com`, and lowercased path contains none of `login`, `signin`, `/auth` |
| Session cookie family prefix | `__secure-next-auth.session-token` (lowercased compare) |
| Preferred session cookie | lowercased name == prefix, or starts with prefix + `.` (chunks `.0`, `.1`, …) |
| `looksLikeAuthCookie` | lowercased name contains `csrf` → false; else contains `session-token` or `session_token` |
| Essential allowlist | **prefix** family `__secure-next-auth.session-token` (whole family incl. chunks); CSRF excluded |
| `includeAllDomainCookiesWhenSessionFound` / `maxAttempts` | true / 4 |
| Failure message | `No ChatGPT session cookie found. Finish signing in to chatgpt.com, then click Capture Session.` |
| Store prefix / log | `chatgpt_auth_` / `ChatGPTAuth` |
| Identity | no identity cookie → account identity = captured email (if any) |

Rolling cookie: after each successful refresh the poller calls `auth.applyRefreshedCookies(setCookieHeaders)`
(Core): parse each `Set-Cookie` for `https://chatgpt.com/`, keep only essential names; a live renewed
cookie of a chunk family replaces the whole family in the stored header; expired/empty ones remove
the pair; rewrite the stored `session` only when the header changed.

### 2.2 Endpoints and flow (`ChatGPTUsageClient`)

| Step | Method / URL | Headers | Response |
|---|---|---|---|
| 1 Session exchange | `GET https://chatgpt.com/api/auth/session` | defaults + `Cookie` + `Referer: https://chatgpt.com/` (no bearer) | JSON `{ "accessToken": "<JWT>", "user": {...}, ... }` + `Set-Cookie` (renewed session) |
| 2 Usage | `GET https://chatgpt.com/backend-api/wham/usage` | defaults + `Cookie` + `Authorization: Bearer <accessToken>` + `Referer: https://chatgpt.com/` + `ChatGPT-Account-Id: <id>` (only when extracted) | usage JSON (below) + `Set-Cookie` |

`fetchUsage(now)`:

1. If the token cache has a valid token for `tokenCacheKey` at `now`: call usage with it
   (sessionCookies = `[]`). On `ProviderError` whose usage error is `unauthorized` → `cache.clear()`
   and fall through; any other error propagates.
2. Exchange: `transport.session(cookie)` → `parseSessionToken(body)`; store
   `cache.store(token, key, validUntil(token, now))`.
3. Usage with the new token; `Fetch.setCookieHeaders = sessionSetCookies + usageSetCookies`;
   `fetchedAt = now`.

`parseSessionToken(data)`: not a JSON object → `badResponse(.chatGPT, "Sign-in page returned instead of a session token")`
when body looks like HTML, else `badResponse(.chatGPT, "Malformed session payload")` (both
**transient**, the credential is kept). `accessToken` missing/not string/empty → `unauthorized(.chatGPT)`.

Poller: `tokenCacheKey = String(auth.sessionGeneration)` (stable while cookies roll; changes on
sign-in/out).

### 2.3 Access-token cache (`ChatGPTAccessTokenCache`, in memory only)

| Constant | Value |
|---|---|
| `sessionRenewInterval` | 30 min (1800 s) |
| `expiryMargin` | 5 min (300 s) |
| `unknownExpiryLifetime` | 5 min (300 s) |

- Single entry `(key, token, validUntil)`. `token(forKey:now)` returns it only if key matches and
  `now < validUntil`.
- `validUntil(token, now)`: JWT `exp` readable → `min(now + 1800, exp − 300)`; unreadable →
  `now + 300`. (An already-near-expiry token yields a past deadline → not reused.)
- Cleared by: poller `clearSnapshot()` (sign-out / account reset), `onRejected` (an
  unauthorized/notSignedIn result), and a cached-token 401/403 inside `fetchUsage`.

### 2.4 JWT claims (`ChatGPTAccountID`)

- Payload = `token.split(".")[1]` (need ≥ 2 parts), base64url (`-`→`+`, `_`→`/`, pad `=` to a
  multiple of 4), JSON object. Signature not verified.
- Account id: claim object `"https://api.openai.com/auth"`; return string `chatgpt_account_id`; else
  iterate the claim's keys and return the first string value whose lowercased key contains
  `account_id` (iteration order is unspecified in Swift; Rust may use document order). Else nil.
- Expiry: `JSON.number(payload.exp)` seconds since epoch.

### 2.5 Usage parsing (`ChatGPTUsageResponse.parse`)

Sample:

```json
{"plan_type":"plus","rate_limit":{"allowed":true,"limit_reached":false,
 "primary_window":{"used_percent":12,"limit_window_seconds":18000,"reset_after_seconds":9000,"reset_at":1787300000},
 "secondary_window":{"used_percent":57.5,"limit_window_seconds":604800,"reset_after_seconds":343143,"reset_at":1787800000}}}
```

- Root must be a JSON object, else `badResponse(.chatGPT, "Unexpected usage payload")` (HTML too).
- `rl = root.rate_limit` if it is an object, else root.
- `planName = root.plan_type` (string), `allowed = rl.allowed` (bool, default **true**),
  `limitReached = rl.limit_reached` (bool, default false).
- `primary = window(rl.primary_window ?? rl.five_hour)`; `secondary = window(rl.secondary_window ?? rl.weekly)`.
  Parity quirk: the fallback key is used only when the first key is **absent**; a present-but-`null`
  `primary_window` does not fall back.
- `window(d)`: object with `used = JSON.number(used_percent) ?? JSON.number(utilization)`;
  `resetsAt` from `reset_at`: `JSON.number` → epoch seconds, else string → ISO8601 flexible;
  `windowSeconds = Int(JSON.number(limit_window_seconds))` (truncate); `usedPercent = clamp(used)`.
  `reset_after_seconds` is ignored.

`ChatGPTUsageWindow.label(fallback)`: `windowSeconds` nil or ≤ 0 → fallback; `h = s/3600`:
`h == 168` → `Weekly`; `h == 24` → `Daily`; `h < 24` and integral → `"<h>-Hour Window"`;
`h % 24 == 0` → `"<h/24>-Day Window"`; else fallback. (18000→`5-Hour Window`, 3600→`1-Hour Window`,
2592000→`30-Day Window`, 5400→fallback.)

Snapshot: `{fetchedAt, planName, allowed, limitReached, primary, secondary}`;
`displayPlanName` = `"ChatGPT"` if plan nil/empty, else first char uppercased + rest lowercased
(`plus`→`Plus`); `usagePool` = combining(primary→hourly, secondary→weekly);
`resetsAt` / `headlineUsedPercent` per §0.

### 2.6 Poller (`ChatGPTUsagePoller`)

- Subscriptions: `signedOut` and `accountReset` → `clearSnapshot()` (clears token cache, snapshot,
  lastError, lastRefreshedAt). No persisted history.
- Refresh: gate; cookie missing → `needsSignIn = true`, if no snapshot `lastError = "Sign in to ChatGPT to load usage."`,
  `.skipped`. Else `ProviderRefresh.run(label: "ChatGPT", onRejected: tokenCache.clear)`.
- Publish: `applyRefreshedCookies(fetch.setCookieHeaders)`, set snapshot, `lastError = nil`,
  `lastRefreshedAt = now`, `needsSignIn = false`.

### 2.7 Panel (`ChatGPTPanelView`)

- Signed-out (needsSignIn && no snapshot): header `ChatGPT`; text
  `Sign in to chatgpt.com to load your 5-hour and weekly Codex usage.`; button `Sign In to ChatGPT…`.
- Main: header `ChatGPT` with trailing meta label `displayPlanName`; card: `Usage Pool …`; track
  label `primary.label(fallback: "5-Hour Window")` (or `5-Hour Window` if no primary), percent
  `primary.usedPercent ?? 0`, caption `Limit reached` when `limitReached` else reset caption of
  `primary.resetsAt`; track `secondary.label(fallback: "Weekly")`, caption reset of
  `secondary.resetsAt`; if `!allowed` caption text `Codex access is currently blocked for this account.`.
  Then stale-error caption, `Sign In Again…` if needsSignIn, sign-out button. No bars, no hover.
- Else: header + `Refreshing…` / lastError / `No usage data yet.` + sign-in button if needsSignIn.

---

## 3. OpenCode (Go subscription + Zen)

### 3.1 Auth (`OpenCodeAuthSession`, `OpenCodeSignIn`)

| Item | Value |
|---|---|
| Hosts | `opencode.ai`, `auth.opencode.ai` |
| Start URL | `https://opencode.ai/console/login` |
| Title / subtitle | `Sign in to OpenCode` / `Sign in to the OpenCode console. This window finishes on its own once the console loads.` |
| `isAuthHost(host, path)` | host contains `auth.opencode.ai`, or path contains `/auth/authorize` or `/console/login` |
| `isReturnPage(url)` | lowercased host is `opencode.ai` or ends with `.opencode.ai`, and does **not** contain `auth.`; then (case-sensitive path): starts with `/console` → true unless it starts with `/console/login`; else true if path contains `/workspace`, or (starts with `/auth` and does not contain `authorize`) |
| `onReturned(url)` | if the URL path has a segment starting `wrk_` → `saveWorkspaceID(id)` |
| `afterCapture` | with the captured cookie: `resolveOrgID(preferred: workspaceID)` → save; then `fetchAccountEmail()` → `saveAccountEmail` (also becomes the account identity) |
| Console session cookie | `__host-console_session` (lowercased compare; real name `__Host-console_session`) |
| Preferred session cookie | lowercased name `auth` or `__host-console_session` |
| `looksLikeAuthCookie` | name in {`auth`,`session`,`sid`,`__host-console_session`} or contains any of `auth`,`session`,`token`,`jwt`,`sid`,`account`,`openid`,`oauth` |
| Essential allowlist | `auth`, `provider`, `__host-console_session` |
| maxAttempts / failure msg | 4 / `No OpenCode console session cookie found. Finish signing in until you see the console, then click Finish Sign-In.` |
| Store prefix / log / extra keys | `opencode_auth_` / `OpenCodeAuth` / `["workspace"]` (file-stored, not secret) |

`refreshFromDisk` also loads `workspaceID = store["workspace"]`. `clearBrowserState` (sign-out and
3rd-strike invalidation) removes `workspace` (extra key) and sets in-memory `workspaceID = nil`.

### 3.2 Console endpoints (`OpenCodeConsoleClient`)

Live GET (custom, not `ProviderHTTP`): URL = path resolved against `https://opencode.ai`; timeout
**20 s**; headers `Cookie: <header>`, `Accept: application/json`, `User-Agent: TokenMon/<v>`, and
`x-org-id: <wrk_…>` when org-scoped. Response check (`check(http, data, orgID)`) in order:

1. Bot challenge → `badResponse(.openCode, "Request blocked by a bot-protection check (HTTP <s>). Retrying.")`.
2. Final URL (after redirects) path contains `/console/login`, `/auth/authorize` or `/auth/login` →
   `unauthorized(.openCode)`.
3. 403 with an org id → `OpenCodeConsoleError.orgForbidden` (not a strike; next workspace tried).
4. 401 or 403 → `unauthorized(.openCode)`.
5. ≥ 400 → log first 200 body bytes privately; `badResponse(.openCode, "HTTP <s>")` (no body in message).

| Endpoint | Org header | Parse |
|---|---|---|
| `GET /console/api/go/status` | yes | `parseGoMeters` |
| `GET /console/api/orgs` | no | `parseOrgs`: body must be a JSON array of objects else `badResponse("Could not read OpenCode workspaces.")`; ids = `row.id` strings starting `wrk_`, in order |
| `GET /console/auth/session` | no | `parseSessionEmail`: `root.user.email` string; any failure → nil (best effort, errors swallowed) |

`parseGoMeters(data)`: not a JSON object → `badResponse(.openCode, "Unrecognized Go status response.")`.
`access` object with `meters` object required, else empty meters (no seat). Meters
`fiveHour`, `week`, `month` (each object → Meter, else nil); `monthResetsAt = ISO8601(access.endsAt)`.
Meter: `limitUSD = usd(limitMicroCents)` (optional), `usedUSD = usd(usedMicroCents) ?? 0`,
`resetsAt = ISO8601(resetsAt)` if string. `usd(v)`: string → `Double(string)` (no trim), number →
double; else nil; result `/ 100_000_000` (1 USD = 1e8 micro-cents). `hasAny` = any meter non-nil.

Sample `go/status` (see test fixture §7): `access.startsAt/endsAt`, meters with string micro-cent
values; `month` has no `resetsAt` → falls back to `access.endsAt`.

`fetchGoUsageSnapshot(knownOrgID)`:

1. `preferred = knownOrgID` if it starts with `wrk_`, else nil.
2. `attempt(org)`: `tried += 1`; GET status with org; `seated` (hasAny) → return meters; `noSeat` →
   continue; `orgForbidden` → `forbidden += 1`. Other errors propagate (session expiry etc.).
3. Try `preferred` first (no org list needed if it is seated). Then `listOrgs()` and try each org
   ≠ preferred, in order. First seated → `(snapshot(meters, now), org)`.
4. `tried == 0` → `badResponse("No OpenCode workspace on this account.")`; all forbidden →
   `badResponse("The OpenCode console denied access to this workspace.")`; else
   `badResponse("No Go subscription on this account (or another member holds the Go seat).")`.

`resolveOrgID(preferred)`: preferred with `wrk_` prefix → it; else first listed org; none →
`badResponse("No OpenCode workspace on this account.")`.

`workspaceID(from url)`: first path segment (split on `/`) starting with `wrk_`.

`snapshot(meters, now)`: windows `[rolling5h(fiveHour), weekly(week), monthly(month, fallbackReset: monthResetsAt)]`
dropping missing meters; each `limitUSD = meter.limitUSD ?? kind.defaultLimitUSD`,
`resetsAt = meter.resetsAt ?? fallback`, `sessionCount 0`; `models: []`, `isEstimated: false`,
monthly totals 0.

### 3.3 Local SQLite source (`opencode.db`)

Path: `<real home>/.local/share/opencode/opencode.db` (macOS; `XDG_DATA_HOME` is **not** consulted
by the Swift code). Opened with `sqlite3_open_v2(path, SQLITE_OPEN_READONLY)` using the plain path
(no `file:` URI), `busy_timeout = 2000 ms`. Missing file →
`OpenCodeLocalStatsError.databaseMissing` (`OpenCode usage database not found at <path>. Install and use OpenCode to track usage.`);
open error → `openFailed` (`Could not open the OpenCode usage database: <msg>`); prepare/step error →
`queryFailed` (`Could not read OpenCode usage: <msg>`). A non-`DONE` terminal step (BUSY past
timeout, corruption) throws rather than returning a partial read.

Windows mapping (flag: verify): OpenCode resolves its data dir with `xdg-basedir`, which on Windows
falls back to `%USERPROFILE%\.local\share` when `XDG_DATA_HOME` is unset → expected
`%USERPROFILE%\.local\share\opencode\opencode.db`. Recommended Rust lookup order:
`%XDG_DATA_HOME%\opencode\opencode.db` (if set), `%USERPROFILE%\.local\share\opencode\opencode.db`;
consider `%LOCALAPPDATA%\opencode\opencode.db` as a last-resort probe. Read-only opening a WAL
database needs the `-shm` file to exist or the directory to be writable; on Windows OpenCode holding
the file open is fine for SQLite readers, but test it.

**Query 1 — subscription anchor** (`earliestGoSessionDate`):

```sql
SELECT MIN(time_created) FROM session
WHERE time_archived IS NULL
  AND lower(CASE WHEN json_valid(model) THEN json_extract(model, '$.providerID') END) = 'opencode-go'
```

Result NULL → nil; else `Date(ms / 1000)`.

**Query 2 — assistant messages** (`readAssistantMessageRows`, bind `?1 = startMS` int64):

```sql
SELECT time_created, session_id,
  json_extract(data, '$.providerID'), json_extract(data, '$.modelID'),
  json_extract(data, '$.cost'),
  json_extract(data, '$.tokens.input'), json_extract(data, '$.tokens.output'),
  json_extract(data, '$.tokens.cache.read'), json_extract(data, '$.tokens.cache.write')
FROM message
WHERE time_created >= ?
  AND (CASE WHEN json_valid(data) THEN json_extract(data, '$.role') END) = 'assistant'
```

Row mapping (`SessionRow`): `timeCreatedMS = col0 int64`; `sessionID = col1 text trimmed` (NULL →
`""`); `providerID`, `modelID` = cols 2/3 as text trimmed of whitespace+newlines (NULL → `""`); rows
with empty provider or model are **skipped**; `costUSD = col4 double` (NULL → 0, SQLite coercion);
tokens cols 5–8 `int64` (NULL → 0; SQLite integer coercion). Malformed JSON rows are excluded by the
`json_valid` guard.

Columns relied on: `session(time_created INTEGER ms, time_archived INTEGER|NULL, model TEXT JSON {"id","providerID"})`,
`message(id, session_id, time_created INTEGER ms, time_updated, data TEXT JSON)`. `data` JSON:
`{"role":"assistant","cost":<usd>,"providerID":"opencode-go","modelID":"minimax-m3","tokens":{"input":N,"output":N,"cache":{"read":N,"write":N}}}`.

`SessionRow.date = ms/1000`; `totalTokens = in + out + cacheRead + cacheWrite`;
`billable = OpenCodeZenCostEstimate.billableCostUSD(...)` (§3.5).

### 3.4 Scan + cache

- `scanLookbackDays = 62`; `scanStart(now) = now − 62×86400 s`.
- `loadScan(dbURL, since, cache)`: file must exist (else `databaseMissing`); cache hit when same
  path, same **fingerprint**, and `cached.since <= since`; else open one connection, run Query 1
  and Query 2 (`startMS = Int64(since × 1000)`), close; store. Fingerprint =
  `(db mtime, db size, <db>-wal mtime, <db>-wal size)` (missing file → nil components).
- One scan feeds snapshot, hourly chart, daily spends and day tokens; idle DB read once.

### 3.5 Pricing (`OpenCodeZenCostEstimate`, `ratesVerifiedOn = "2026-10-10"`)

`Rates {input, output, cacheRead, cacheWrite}` per 1M.
`value(r, tokens) = (r.in×in + r.out×out + r.cr×cr + r.cw×cw) / 1e6`.

`modelIDVariants(id)`: `[lower(id)]`, then repeatedly strip one suffix `-contributor` (checked first)
or `-free` and append, until neither applies (e.g. `muse-spark-1.2-contributor-free` →
`…-contributor-free`, `…-contributor`, `muse-spark-1.2`).

`rates(providerID, modelID)`: provider table = `goRates` for `opencode-go`, `zenRates` for
`opencode` (provider lowercased), none otherwise. For each variant in order: provider table hit →
return; else `fallbackRates` hit → return. None → nil.

`catalogRates(modelID)`: variants looked up in `fallbackRates` only (used by OpenRouter).

`isPlanProvider(p)`: lowercased `opencode` or `opencode-go`.

`billableCostUSD(provider, model, recorded, tokens…) -> (cost, isEstimated, isUnpriced)`:
1. `recorded > 0` → `(recorded, false, false)`.
2. not a plan provider → `(0, false, false)`.
3. total tokens ≤ 0 → `(0, false, false)`.
4. no rates → `(0, false, true)`.
5. `v = value(rates)` → `(v, v > 0, false)`.

`estimate(provider, model, …)` = value or 0; `estimate(modelID, …)` (catalog only) = value or 0.

**Go rates** (`opencode-go`, docs/go, off-peak DeepSeek, base context tier):

| # | Model id | Input | Output | Cache read | Cache write |
|---|---|---|---|---|---|
| 1 | `glm-5.3-flash` | 0.15 | 0.50 | 0.03 | 0 |
| 2 | `glm-5.3` | 1.40 | 4.40 | 0.26 | 0 |
| 3 | `glm-5.2` | 1.40 | 4.40 | 0.26 | 0 |
| 4 | `kimi-k3` | 3.00 | 15.00 | 0.30 | 0 |
| 5 | `kimi-k2.7-code` | 0.95 | 4.00 | 0.19 | 0 |
| 6 | `kimi-k2.6` | 0.95 | 4.00 | 0.16 | 0 |
| 7 | `longcat-2.0` | 0.30 | 1.20 | 0.006 | 0 |
| 8 | `mimo-v2.6-flash` | 0.14 | 0.28 | 0.0028 | 0 |
| 9 | `mimo-v2.6-pro` | 0.435 | 0.87 | 0.003625 | 0 |
| 10 | `mimo-v2.5` | 0.14 | 0.28 | 0.0028 | 0 |
| 11 | `mimo-v2.5-pro` | 0.435 | 0.87 | 0.003625 | 0 |
| 12 | `minimax-m3` | 0.30 | 1.20 | 0.06 | 0 |
| 13 | `minimax-m2.7` | 0.30 | 1.20 | 0.06 | 0.375 |
| 14 | `muse-spark-1.3-contributor` | 0.10 | 0.20 | 0.002 | 0 |
| 15 | `muse-spark-1.2-contributor` | 0.10 | 0.20 | 0.002 | 0 |
| 16 | `qwen3.8-max` | 2.00 | 6.00 | 0.25 | 2.50 |
| 17 | `qwen3.8-flash` | 0.15 | 0.47 | 0.016 | 0.20 |
| 18 | `qwen3.7-plus` | 0.40 | 1.60 | 0.04 | 0.50 |
| 19 | `deepseek-v4.1-flash` | 0.15 | 0.60 | 0.003 | 0 |
| 20 | `deepseek-v4-pro` | 0.66 | 1.98 | 0.022 | 0 |
| 21 | `deepseek-v4-flash` | 0.15 | 0.60 | 0.003 | 0 |
| 22 | `deepseek-v4-flash-vision-exp` | 0.15 | 0.60 | 0.003 | 0 |
| 23 | `hy4-preview` | 0.834 | 2.501 | 0.042 | 0 |
| 24 | `hy3` | 0.14 | 0.58 | 0.035 | 0 |
| 25 | `space-bunny` | 0.15 | 0.60 | 0.03 | 0 |
| 26 | `grok-4.7` | 2.00 | 6.00 | 0.50 | 0 |
| 27 | `grok-4.6` | 2.00 | 6.00 | 0.50 | 0 |
| 28 | `gpt-6-luna` | 0.10 | 0.50 | 0.01 | 0.125 |
| 29 | `gpt-5.6-luna` | 0.20 | 1.20 | 0.02 | 0.25 |
| 30 | `claude-haiku-5-5` | 0.10 | 0.50 | 0.01 | 0.125 |

**Zen rates** (`opencode`, docs/zen pay-as-you-go, base tier):

| # | Model id | Input | Output | Cache read | Cache write |
|---|---|---|---|---|---|
| 1 | `minimax-m3` | 0.30 | 1.20 | 0.06 | 0 |
| 2 | `minimax-m2.7` | 0.30 | 1.20 | 0.06 | 0 |
| 3 | `minimax-m2.5` | 0.30 | 1.20 | 0.06 | 0 |
| 4 | `glm-5.3-flash` | 0.15 | 0.50 | 0.03 | 0 |
| 5 | `glm-5.3` | 1.40 | 4.40 | 0.26 | 0 |
| 6 | `glm-5.2` | 1.40 | 4.40 | 0.26 | 0 |
| 7 | `glm-5.1` | 1.40 | 4.40 | 0.26 | 0 |
| 8 | `glm-5` | 1.00 | 3.20 | 0.20 | 0 |
| 9 | `kimi-k2.7-code` | 0.95 | 4.00 | 0.19 | 0 |
| 10 | `kimi-k3` | 3.00 | 15.00 | 0.30 | 0 |
| 11 | `kimi-k2.6` | 0.95 | 4.00 | 0.16 | 0 |
| 12 | `kimi-k2.5` | 0.60 | 3.00 | 0.10 | 0 |
| 13 | `qwen3.8-max` | 2.00 | 6.00 | 0.25 | 2.50 |
| 14 | `qwen3.8-flash` | 0.15 | 0.47 | 0.016 | 0.20 |
| 15 | `qwen3.7-max` | 2.50 | 7.50 | 0.50 | 3.125 |
| 16 | `qwen3.7-plus` | 0.40 | 1.60 | 0.04 | 0.50 |
| 17 | `qwen3.6-plus` | 0.50 | 3.00 | 0.05 | 0.625 |
| 18 | `qwen3.5-plus` | 0.20 | 1.20 | 0.02 | 0.25 |
| 19 | `deepseek-v4.1-flash` | 0.30 | 1.20 | 0.006 | 0 |
| 20 | `deepseek-v4-pro` | 1.74 | 3.48 | 0.145 | 0 |
| 21 | `deepseek-v4-flash` | 0.14 | 0.28 | 0.028 | 0 |
| 22 | `deepseek-v4-flash-vision-exp` | 0.14 | 0.28 | 0.028 | 0 |
| 23 | `claude-fable-5-1` | 10.00 | 50.00 | 0.25 | 12.50 |
| 24 | `claude-fable-5` | 10.00 | 50.00 | 1.00 | 12.50 |
| 25 | `claude-opus-5-5` | 4.00 | 20.00 | 0.20 | 5.00 |
| 26 | `claude-opus-5` | 5.00 | 25.00 | 0.50 | 6.25 |
| 27 | `claude-opus-4-8` | 5.00 | 25.00 | 0.50 | 6.25 |
| 28 | `claude-opus-4-7` | 5.00 | 25.00 | 0.50 | 6.25 |
| 29 | `claude-opus-4-6` | 5.00 | 25.00 | 0.50 | 6.25 |
| 30 | `claude-opus-4-5` | 5.00 | 25.00 | 0.50 | 6.25 |
| 31 | `claude-sonnet-5-5` | 2.00 | 10.00 | 0.20 | 2.50 |
| 32 | `claude-sonnet-5` | 2.00 | 10.00 | 0.20 | 2.50 |
| 33 | `claude-haiku-5-5` | 0.10 | 0.50 | 0.01 | 0.125 |
| 34 | `claude-sonnet-4-6` | 3.00 | 15.00 | 0.30 | 3.75 |
| 35 | `claude-sonnet-4-5` | 3.00 | 15.00 | 0.30 | 3.75 |
| 36 | `claude-haiku-4-5` | 1.00 | 5.00 | 0.10 | 1.25 |
| 37 | `gemini-3.8-flash` | 1.50 | 7.50 | 0.15 | 0 |
| 38 | `gemini-3.7-flash` | 1.50 | 7.50 | 0.15 | 0 |
| 39 | `gemini-3.6-flash` | 1.50 | 7.50 | 0.15 | 0 |
| 40 | `gemini-3.5-flash` | 1.50 | 9.00 | 0.15 | 0 |
| 41 | `gemini-3.5-flash-lite` | 0.30 | 2.50 | 0.03 | 0 |
| 42 | `gemini-3.1-pro` | 2.00 | 12.00 | 0.20 | 0 |
| 43 | `gemini-3-flash` | 0.50 | 3.00 | 0.05 | 0 |
| 44 | `grok-4.7` | 2.00 | 6.00 | 0.50 | 0 |
| 45 | `grok-4.6` | 2.00 | 6.00 | 0.50 | 0 |
| 46 | `grok-4.5` | 2.00 | 6.00 | 0.30 | 0 |
| 47 | `grok-build-0.1` | 1.00 | 2.00 | 0.20 | 0 |
| 48 | `muse-spark-1.3` | 1.25 | 4.25 | 0.15 | 0 |
| 49 | `muse-spark-1.2` | 1.25 | 4.25 | 0.15 | 0 |
| 50 | `gpt-6-astra` | 10.00 | 50.00 | 1.00 | 12.50 |
| 51 | `gpt-6-sol` | 2.00 | 10.00 | 0.20 | 2.50 |
| 52 | `gpt-6.1-sol` | 2.00 | 10.00 | 0.10 | 2.50 |
| 53 | `gpt-6-luna` | 0.10 | 0.50 | 0.01 | 0.125 |
| 54 | `gpt-5.6-sol` | 4.00 | 20.00 | 0.40 | 5.00 |
| 55 | `gpt-5.6-terra` | 2.00 | 12.00 | 0.20 | 2.50 |
| 56 | `gpt-5.6-luna` | 0.20 | 1.20 | 0.02 | 0.25 |
| 57 | `gpt-5.5` | 5.00 | 30.00 | 0.50 | 0 |
| 58 | `gpt-5-codex` | 1.07 | 8.50 | 0.107 | 0 |
| 59 | `gpt-5.5-pro` | 30.00 | 180.00 | 30.00 | 0 |
| 60 | `gpt-5.4` | 2.50 | 15.00 | 0.25 | 0 |
| 61 | `gpt-5.4-pro` | 30.00 | 180.00 | 30.00 | 0 |
| 62 | `gpt-5.4-mini` | 0.75 | 4.50 | 0.075 | 0 |
| 63 | `gpt-5.4-nano` | 0.20 | 1.25 | 0.02 | 0 |
| 64 | `gpt-5.3-codex-spark` | 1.75 | 14.00 | 0.175 | 0 |
| 65 | `gpt-5.3-codex` | 1.75 | 14.00 | 0.175 | 0 |
| 66 | `gpt-5.2` | 1.75 | 14.00 | 0.175 | 0 |
| 67 | `gpt-5.2-codex` | 1.75 | 14.00 | 0.175 | 0 |
| 68 | `gpt-5.1` | 1.07 | 8.50 | 0.107 | 0 |
| 69 | `gpt-5.1-codex` | 1.07 | 8.50 | 0.107 | 0 |
| 70 | `gpt-5.1-codex-max` | 1.25 | 10.00 | 0.125 | 0 |
| 71 | `gpt-5.1-codex-mini` | 0.25 | 2.00 | 0.025 | 0 |
| 72 | `gpt-5` | 1.07 | 8.50 | 0.107 | 0 |
| 73 | `gpt-5-nano` | 0.05 | 0.40 | 0.005 | 0 |

**Shared catalogue / fallback** (OpenRouter-sourced; free-model paid-equivalents and OpenRouter `$0` rows):

| # | Model id | Input | Output | Cache read | Cache write |
|---|---|---|---|---|---|
| 1 | `big-pickle` | 0.30 | 1.20 | 0.06 | 0 |
| 2 | `mimo-v2.5-free` | 0.14 | 0.28 | 0.0028 | 0 |
| 3 | `mimo-v2.6-flash-free` | 0.14 | 0.28 | 0.0028 | 0 |
| 4 | `laguna-s-2.1-free` | 0.09 | 0.18 | 0.009 | 0 |
| 5 | `ling-3.0-flash-free` | 0.021 | 0.063 | 0.0042 | 0 |
| 6 | `north-mini-code-free` | 0.20 | 0.80 | 0.02 | 0 |
| 7 | `nemotron-3-ultra-free` | 0.50 | 2.20 | 0.10 | 0 |
| 8 | `nemotron-3.5-lightning-free` | 0.06 | 0.16 | 0.03 | 0 |
| 9 | `hy3-free` | 0.132 | 0.528 | 0.033 | 0 |
| 10 | `space-bunny` | 0.15 | 0.60 | 0.03 | 0 |
| 11 | `minimax-m2.5` | 0.27 | 1.08 | 0.027 | 0 |
| 12 | `minimax-m2.7` | 0.30 | 1.20 | 0.06 | 0 |
| 13 | `deepseek-v4.1-flash` | 0.15 | 0.60 | 0.003 | 0 |
| 14 | `deepseek-v4-flash` | 0.04872 | 0.09744 | 0.009744 | 0 |
| 15 | `deepseek-v4-pro` | 0.783 | 1.566 | 0.06525 | 0 |
| 16 | `minimax-m3` | 0.30 | 1.20 | 0.06 | 0 |
| 17 | `mimo-v2.5` | 0.14 | 0.28 | 0.0028 | 0 |
| 18 | `mimo-v2.5-pro` | 0.435 | 0.87 | 0.0036 | 0 |
| 19 | `mimo-v2.6-pro` | 0.435 | 0.87 | 0.0036 | 0 |
| 20 | `glm-5` | 0.60 | 1.92 | 0.12 | 0 |
| 21 | `glm-5.1` | 0.9646 | 3.0316 | 0.17914 | 0 |
| 22 | `glm-5.2` | 0.6496 | 2.0416 | 0.12064 | 0 |
| 23 | `glm-5.3-flash` | 0.045 | 0.14 | 0.01 | 0 |
| 24 | `kimi-k2.5` | 0.45 | 2.25 | 0.07 | 0 |
| 25 | `kimi-k2.6` | 0.95 | 4.00 | 0.16 | 0 |
| 26 | `kimi-k2.7-code` | 0.6562 | 3.30 | 0.18 | 0 |
| 27 | `kimi-k3` | 3.00 | 15.00 | 0.30 | 0 |
| 28 | `qwen3.5-plus` | 0.20 | 1.20 | 0.02 | 0.25 |
| 29 | `qwen3.6-plus` | 0.325 | 1.95 | 0 | 0.40625 |
| 30 | `qwen3.7-plus` | 0.32 | 1.28 | 0.064 | 0.40 |
| 31 | `qwen3.7-max` | 1.475 | 4.425 | 0.295 | 1.84375 |
| 32 | `claude-fable-5` | 10.00 | 50.00 | 1.00 | 12.50 |
| 33 | `claude-opus-4-5` | 5.00 | 25.00 | 0.50 | 6.25 |
| 34 | `claude-opus-4-6` | 5.00 | 25.00 | 0.50 | 6.25 |
| 35 | `claude-opus-4-7` | 5.00 | 25.00 | 0.50 | 6.25 |
| 36 | `claude-opus-4-8` | 5.00 | 25.00 | 0.50 | 6.25 |
| 37 | `claude-opus-5` | 5.00 | 25.00 | 0.50 | 6.25 |
| 38 | `claude-sonnet-4-5` | 3.00 | 15.00 | 0.30 | 3.75 |
| 39 | `claude-sonnet-4-6` | 3.00 | 15.00 | 0.30 | 3.75 |
| 40 | `claude-sonnet-5` | 2.00 | 10.00 | 0.20 | 2.50 |
| 41 | `claude-haiku-4-5` | 1.00 | 5.00 | 0.10 | 1.25 |
| 42 | `gemini-3-flash` | 0.50 | 3.00 | 0.05 | 0 |
| 43 | `gemini-3.1-pro` | 2.00 | 12.00 | 0.20 | 0 |
| 44 | `gemini-3.5-flash` | 1.50 | 9.00 | 0.15 | 0.083333 |
| 45 | `gemini-3.5-flash-lite` | 0.30 | 2.50 | 0.03 | 0.083333 |
| 46 | `gemini-3.6-flash` | 0.75 | 3.75 | 0.075 | 0.041667 |
| 47 | `grok-4.5` | 2.00 | 6.00 | 0.30 | 0 |
| 48 | `grok-build-0.1` | 1.00 | 2.00 | 0.20 | 0 |
| 49 | `gpt-5` | 1.25 | 10.00 | 0.125 | 0 |
| 50 | `gpt-5.1` | 1.25 | 10.00 | 0.125 | 0 |
| 51 | `gpt-5.1-codex` | 1.25 | 10.00 | 0.13 | 0 |
| 52 | `gpt-5.1-codex-max` | 1.25 | 10.00 | 0.125 | 0 |
| 53 | `gpt-5.1-codex-mini` | 0.25 | 2.00 | 0.03 | 0 |
| 54 | `gpt-5.2` | 1.75 | 14.00 | 0.175 | 0 |
| 55 | `gpt-5.2-codex` | 1.75 | 14.00 | 0.175 | 0 |
| 56 | `gpt-5.3-codex` | 1.75 | 14.00 | 0.175 | 0 |
| 57 | `gpt-5.3-codex-spark` | 1.75 | 14.00 | 0.175 | 0 |
| 58 | `gpt-5.4` | 2.50 | 15.00 | 0.25 | 0 |
| 59 | `gpt-5.4-mini` | 0.75 | 4.50 | 0.075 | 0 |
| 60 | `gpt-5.4-nano` | 0.20 | 1.25 | 0.02 | 0 |
| 61 | `gpt-5.5` | 5.00 | 30.00 | 0.50 | 0 |
| 62 | `gpt-5.6-luna` | 0.20 | 1.20 | 0.02 | 0.25 |
| 63 | `muse-spark-1.3-contributor-free` | 0.10 | 0.20 | 0.002 | 0 |
| 64 | `muse-spark-1.2-contributor-free` | 0.10 | 0.20 | 0.002 | 0 |
| 65 | `muse-spark-1.2-contributor` | 0.10 | 0.20 | 0.002 | 0 |
| 66 | `muse-spark` | 1.25 | 4.25 | 0.15 | 0 |

### 3.6 Local snapshot (`OpenCodeLocalStats.snapshot(scan, now)`)

Constants: `rolling5hSeconds = 18000`; limits `rolling5h $12`, `weekly $30`, `monthly $60`.

- Rolling rows: `date >= now − 5h` (no upper bound).
- Week: `weeklyBounds(now)` = UTC Monday 00:00 to +7 days, `[start, end)` (Sunday belongs to the
  week that started the previous Monday).
- Month: `monthlyBounds(now, subscribedAt: scan.subscribedAt ?? now)`, `[start, end)`.
  (With no Go session, anchor = now → month starts at `now` → monthly ≈ 0.)
- `monthlyBounds`: UTC Gregorian. Take day/hour/minute/second/nanosecond of `subscribedAt`;
  `anchor(y, m)` = that day clamped to the month length at that time; `start = anchor(now.year, now.month)`;
  if `start > now` shift to previous month; `end = anchor(next month)`. E.g. subscribed
  2026-05-18 14:24:41 & now 2026-08-01 15:00 → `[2026-07-18 14:24:41, 2026-08-18 14:24:41)`;
  subscribed Jan 31, now Feb 15 → `[Jan 31, Feb 28)`.
- Windows (Go rows only, `providerID` lowercased == `opencode-go`):
  `usedUSD = Σ billable.cost`; `limitUSD = default`; `sessionCount` = distinct non-empty session ids
  + count of rows with empty id; resets: rolling = `max(Go row date in rolling) + 5h` (nil if none),
  weekly = week.end, monthly = month.end.
- `models` = `modelUsage(week rows where isPlanProvider)` (Go + Zen; BYOK excluded):
  key `provider/model`; sums tokens and billable cost; `isCostEstimated` if any row estimated;
  `isCostUnpriced` if any row unpriced; `sessionCount`: parity quirk — a row with empty session id
  does `+= 1`, a row with an id sets `sessionCount = |distinct ids for key|` (overwriting anonymous
  increments). Keep entries with `sessionCount > 0` and any of cost/tokens > 0. Sort cost desc then
  outputTokens desc. `percentOfWindow = cost / Σcost × 100` (0 if Σ = 0).
- Monthly totals over month rows with plan providers: `monthlyTokens = in+out+cr+cw`,
  `monthlyInputTokens`, `monthlyOutputTokens`, `monthlyEstimatedUSD = Σ billable.cost`.
- `isEstimated = true`.

`OpenCodeSnapshot`: `windows`, `models`, `isEstimated`, `monthlyTokens`, `monthlyEstimatedUSD`,
`monthlyInputTokens`, `monthlyOutputTokens`;
`monthlyUsedPercent = monthly.usedPercent ?? weekly.usedPercent ?? 0`; `primaryUsedPercent = monthlyUsedPercent`;
`hasLocalStats = !models.isEmpty || monthlyTokens > 0 || monthlyEstimatedUSD > 0`;
`usagePool = combining(windows' kinds → hourly/weekly/monthly)`.
`OpenCodeWindowUsage.usedPercent = limit > 0 ? clamp(used/limit×100) : 0`.
`OpenCodeWindowKind.label`: `5-Hour`, `Weekly`, `Monthly`.

### 3.7 Today's hourly stacks (`buildDayHourlyUsage(rows, now, maxLegendModels = 8)`)

- Local calendar day containing `now`, rows in `[dayStart, dayStart+1d)`, **all providers**.
- Per hour (local hour 0–23), per `provider/model` bucket: `cost += recorded costUSD`,
  `quotaCost += billable.cost`, tokens, `messages += 1`; hour `messageCount`.
- Segments: buckets with `cost > 0 || messages > 0`, sorted cost desc, then messages desc.
- Always 24 `OpenCodeHourUsage {hour, segments, messageCount}`; `totalUSD = Σ seg.costUSD`;
  `hourLabel` = `12a`, `1a`…`11a`, `12p`, `1p`…`11p`.
- Legend: models by total recorded cost desc, first 8, `{id: "provider/model", label: modelID}`.
- `isEmpty` = max hour USD ≤ 0 and all hours have 0 messages.
- Overview helpers: `overviewProviderQuotaHourWeights(monthlyLimitUSD)`: `weekly = monthly / (365.2425/12/7)`;
  if `weekly <= 0` all zeros; per segment `delta = max(0, quotaCost)/weekly×100` added to the Go
  series (`opencode-go`) or Zen series (`opencode`); others ignored.
  `overviewProviderHourTokenCounts()`: same split of `segment.totalTokens`.

### 3.8 Monthly Daily Budget (`OpenCodeDailyBudget.swift`)

- `dailySpendsByDay(rows, calendar = current)`: key = local start-of-day; sum `billable.cost`,
  skipping rows with billable ≤ 0.
- `fetchMonthDailySpends(now)`: Go rows inside `monthlyBounds(now, subscribedAt ?? now)`.
- `fetchDailySpends(from, to, calendar)`: `loadScan(since: from)` (any error → `{}`), Go rows in
  `[from, to)`.
- `dailyTokens(since, calendar)` (hover): Go rows only, `DailyTokenUsageTally` keyed by DayKey,
  model name `OpenCodeModelName.display(modelID)`, costUSD = `billable.cost` or nil when
  `isUnpriced`. Error → `{}`.
- `scaledSpendsPercent(spends, to used)`: if `Σ > 0` and `used > 0` multiply each by `used/Σ`; else
  unchanged.
- `priorWeekHistoryDays = 28`. `spendHistory(periodStart, periodEnd)`: fetch spends from
  `startOfDay(periodStart) − 28 days` to `periodEnd`; `period` = keys `>= startDay && < periodEnd`;
  `history` = keys `< startDay`.

`monthDailyBudgetDays(limitUSD, usedPercent = 0, periodResetsAt?, weekOffset = 0, now, spentByDay?, dbURL, calendar)` → `OpenCodeMonthBudget?`:

1. `consoleBounds = DailyBudget.subscriptionMonth(knownStart: nil, resetsAt: periodResetsAt, now, calendar)`
   (with only `resetsAt`, start = resetsAt − 1 calendar month; stale cycles advance by whole months).
2. USD spends: injected `spentByDay` if given; else if `consoleBounds` → `spendHistory(consoleBounds)`
   (period + history); else `fetchMonthDailySpends`.
3. `spendsPercent = usd / limitUSD × 100` (empty if limit ≤ 0); `anchored = scaledSpendsPercent(spendsPercent, usedPercent)`.
4. If `anchored` empty, `usedPercent > 0` and `consoleBounds`: `elapsed = max(1, daysBetween(startOfDay(start), startOfDay(now)) + 1)`;
   each of the first `elapsed` days from `start` gets `usedPercent / elapsed`.
5. `packaged(knownStart, resetsAt, historyUSD)`: `historyPercent = historyUSD / limit × 100`;
   `DailyBudget.buildSubscriptionMonthLast7Days(limitUSD: 100, spentByDay: anchored, knownStart, resetsAt, historyByDay: historyPercent, weekOffset, now, calendar)`
   (Core: Monday–Sunday week bars; budget/day = 100 / days in period, e.g. `100/31`; nil when neither
   bound is known); returns `{days, periodStart, spentPercentByDay: anchored, historyPercentByDay, knownStart, resetsAt, referenceNow: now}`.
6. If `periodResetsAt` → `packaged(nil, periodResetsAt, history)` (console reset wins).
7. Else `subscribedAt` from `loadScan(scanStart(now))`; none → **nil** (no bars). `month = monthlyBounds(now, subscribedAt)`;
   if no injected spends and history empty, history = `spendHistory(month).history`;
   `packaged(month.start, month.end, history)`.

### 3.9 Model naming and colours

`OpenCodeModelName.display(id)`:

1. Lowercased contains `muse-spark` → `Muse Spark`.
2. Split on `-`, drop empty parts. For each part:
   - previous word exists, part is all digits, previous word is all digits/`.` → merge `prev.part`
     (`4`,`5` → `4.5`);
   - else previous word is `GPT` or `GLM` and part starts with a digit → `GPT-5.4`;
   - else append `brands[lower(part)]` or part with first character uppercased (rest unchanged).
   Brands: `gpt→GPT`, `glm→GLM`, `minimax→MiniMax`, `deepseek→DeepSeek`, `mimo→MiMo`, `hy3→HY3`.
3. No words → original id; else join with spaces.

Examples (tested): `gpt-5.4-mini`→`GPT-5.4 Mini`, `glm-5.1`→`GLM-5.1`, `kimi-k2.6`→`Kimi K2.6`,
`qwen3.6-plus`→`Qwen3.6 Plus`, `minimax-m3`→`MiniMax M3`, `deepseek-v4-flash-free`→`DeepSeek V4 Flash Free`,
`claude-sonnet-4-5`→`Claude Sonnet 4.5`, `muse-spark-1.2-contributor-free`→`Muse Spark`.

`OpenCodeCatalog.providerShortName`: `opencode-go→Go`, `opencode→Zen`, `deepseek→DeepSeek`,
`xai→xAI`, `openrouter→OpenRouter`, `nvidia→NVIDIA`, `alibaba→Alibaba`, `google→Google`,
`anthropic→Anthropic`, `openai→OpenAI`, else id as given. `modelDisplayName(provider, model)` =
`"<short> · Muse Spark"` if lowercased model contains `muse-spark`/`muse_spark`/`musepark`, else
`"<short> · <modelID>"`.

`ModelPalette`: `orange` = OpenCode provider accent; `purple` = sRGB(0.58, 0.44, 0.86). Seeded
palette (8): (0.55,0.78,1.0), (0.11,0.38,0.82), (0.22,0.32,0.48), (0.40,0.55,0.82),
(0.75,0.90,1.0), (0.90,0.45,0.20), (0.20,0.60,0.40), (0.60,0.30,0.70).
`sRGB(seed)`: `h = 5381` (Int64); for each UTF-8 byte `h = (h &* 33) ^ byte` (wrapping);
index = `|h| mod 8` (unsigned magnitude). `sRGB(forProvider:seed:)`: `opencode-go` → orange,
`opencode` → purple, else seeded.

### 3.10 Poller (`OpenCodeUsagePoller`)

State: `snapshot`, `dayHourlyUsage`, `dailyBudgetDays`, `dailyBudgetPeriodStart`, private
`budgetSource`, `isRefreshing`, `lastError`, `lastRefreshedAt`, `dataSourceLabel`.
`signedOut`/`accountReset` → `clearSnapshot()` (all of the above → nil).

Default local fetch (background, user-initiated): `(try? fetchSnapshot(), try? fetchDayHourlyUsage())`
— independent, so hourly data can succeed when the snapshot fails.

`performRefresh()`:

1. Gate. `generation = auth.sessionGeneration`; `cookie = auth.cookieHeader()`;
   `consoleOutcome = .success`.
2. If cookie non-empty — **console path**:
   - `(consoleSnap, orgID) = fetchGoUsageSnapshot(knownOrgID: auth.workspaceID)`; then
     `local = try? fetchLocal()`; `snap = mergeLocalModels(consoleSnap, local.0)`
     (keep console windows; take local `models` if non-empty; take local monthly token/USD fields if
     `monthlyTokens > 0 || monthlyEstimatedUSD > 0`).
   - If cancelled or `!auth.isCurrent(generation)` → `.skipped`.
   - Save workspace id if it changed; publish snapshot; hourly if present; build budget (§3.11);
     `lastError = nil`; `lastRefreshedAt = now`; `dataSourceLabel = "OpenCode console"`;
     `needsSignIn = false`; `recordAuthSuccess()`; return `.success`.
   - Cancellation → `.skipped`. `ProviderError`: if `!isCurrent(generation)` → `.skipped`;
     unauthorized/notSignedIn → `recordAuthFailure` then `generation = auth.sessionGeneration`
     (adopt the post-invalidation generation so this poll still publishes the estimate);
     `consoleOutcome = error`. Other error: if generation changed → `.skipped`; `consoleOutcome = error`.
3. **Local fallback** (always reached unless returned above): `(snap, hourly) = fetchLocal()`.
   If cancelled or generation changed → `.skipped`. Set hourly if present. If `snap == nil` →
   `lastError = "Could not read local OpenCode usage."`, return `consoleOutcome`. Else publish
   snapshot + budget, `dataSourceLabel = "Local estimate"`, and `lastError`:
   - no cookie → `Showing local estimate. Sign in to OpenCode for official Go usage.`
   - `auth.needsSignIn` → `Console session expired — showing local estimate. Sign in again for official numbers.`
   - else → `Console fetch failed — showing local estimate.`
   `lastRefreshedAt = now`; return `consoleOutcome` (a failed console fetch still backs off).
   A thrown local error: generation changed → `.skipped`; else `lastError = error.localizedDescription`,
   return `consoleOutcome`.

### 3.11 Budget build and week browsing

`buildDailyBudgetDays(for snap)`: `monthly = window(kind .monthly)`; `limit = monthly.limitUSD ?? 60`;
`limit <= 0` → nil. Background: `monthDailyBudgetDays(limitUSD: limit, usedPercent: snap.monthlyUsedPercent, periodResetsAt: monthly.resetsAt)`
(default DB, local calendar) → if non-nil set `tokensByDay = dailyTokens(since: scanStart(referenceNow))`
and attach tokens to `days`. Poller stores `budgetSource`, `dailyBudgetDays = days`,
`dailyBudgetPeriodStart = periodStart`.

`dailyBudgetDays(weekOffset)`: no source → nil; 0 → `source.days`; else
`buildSubscriptionMonthLast7Days(limitUSD: 100, spentByDay: source.spentPercentByDay, knownStart, resetsAt, historyByDay: source.historyPercentByDay, weekOffset, now: source.referenceNow)`
with tokens attached (no new SQLite read).

### 3.12 Panel (`OpenCodePanelView`, `OpenCodeLimitBar`)

- Signed-out (needsSignIn && no snapshot): header `OpenCode Go`; text
  `Sign in to the OpenCode console to load official rolling, weekly, and monthly usage.`;
  button `Sign In to OpenCode…`.
- Main:
  - Header `OpenCode Go`.
  - Card: `Usage Pool …`; one limit bar per window: label `5-Hour`/`Weekly`/`Monthly`, percent
    `usedPercent`, colour orange for 5-hour, purple for weekly/monthly; caption:
    5-hour with reset → `Resets <relative named>` (e.g. `Resets in 2 hours`); weekly →
    `Format.resetCaption(date, "EEE h:mma")` (e.g. `Resets Mon 12:00am`); monthly →
    default reset caption; 5-hour with no reset and `usedPercent <= 0` → `No usage in last 5h`;
    otherwise none. If `snapshot.isEstimated`: orange caption
    `Estimated from local sessions — sign in for console accuracy.`
  - `NavigableDailyBudgetCard(style: .monthly, accent: purple, periodUsedPercent: monthlyUsedPercent, periodStart: dailyBudgetPeriodStart, resetsAt: monthly window resetsAt, daysForWeek: dailyBudgetDays(weekOffset:) ?? [])`;
    hover noun `monthly`; hover token block from Go-only `tokensByDay`.
  - If models non-empty: card with collapsible header **`Top Models by Spend`** (persisted key
    `openCodeModels`); shows first 3 unless "View all" toggled (`View all` / `Show less` when
    count > 3). Row: logo, `OpenCodeModelName.display(modelID)`, cost label: `No price` if
    `isCostUnpriced && cost == 0`; else currency string, prefixed `~` when estimated or unpriced;
    share bar width `clamp(percentOfWindow)`, colour `ModelPalette.sRGB(forProvider:seed: "provider/model")`.
  - If `hasLocalStats`: 2×2 grid `Monthly spend` (usd), `Total tokens`, `Input tokens`,
    `Output tokens` (Format.tokens).
  - `lastError` as plain caption (secondary, not the stale caption).
  - If `needsSignIn`: button `Sign In to OpenCode…`. Sign-out button.
- Else: header + `Refreshing…`/lastError/`No usage data yet.` + sign-in button if needsSignIn.

---

## 4. OpenRouter

### 4.1 Auth (`OpenRouterAuthSession`) — API key, no browser

| Item | Rule |
|---|---|
| Storage | `SecretRoutingCredentialStore.live(filenamePrefix: "openrouter_auth_", secretKeys: ["key"])` → Keychain vault key `openrouter_auth_key` (Windows: Core secret store) |
| `apiKey()` | stored `key`, nil if missing/empty |
| init / `refreshFromDisk()` | `isSignedIn = apiKey != nil`, `needsSignIn = !isSignedIn`, failures = 0, `generation += 1` |
| `looksLikeAPIKey(k)` | starts with `sk-or-v1-` or `sk-or-` |
| `saveAPIKey(raw)` | trim whitespace+newlines; empty → `lastAuthError = "Enter an OpenRouter API key."`, false; not key-shaped → `"That doesn't look like an OpenRouter key (expected sk-or-…)."`, false; else store, clear error, failures 0, signedIn, `needsSignIn = false`, `generation += 1`, true |
| `recordAuthFailure(reason)` | `failures += 1`; at ≥ 3 → `markSessionInvalid(reason)` and return true |
| `markSessionInvalid(reason)` | `needsSignIn = true`, `lastAuthError = reason` (if given), `isSignedIn = false`, failures 0, `generation += 1`; **key stays stored** |
| `signOut()` | remove `key`, signed out, `needsSignIn`, error nil, failures 0, `generation += 1`, fire `signedOut` |
| `isCurrent(g)` | `g == generation && isSignedIn && !needsSignIn` |
| Account identity / email | none (`alertAccount = nil`) |

### 4.2 Endpoints (`OpenRouterUsageClient`)

Base `https://openrouter.ai/api/v1`; paths resolved as directory-relative (`key` →
`https://openrouter.ai/api/v1/key`). All `GET`, headers defaults + `Authorization: Bearer <key>`
(no cookie/referer). Context `.openRouter`.

| Endpoint | Required | Body (`data` wrapper required) |
|---|---|---|
| `/key` | yes | `{"data":{"label"?:str,"usage":num (required),"usage_daily"?,"usage_weekly"?,"usage_monthly"?,"limit"?:num|null,"limit_remaining"?,"limit_reset"?:str,"is_free_tier"?:bool,"is_management_key"?:bool}}` |
| `/credits` | only if `is_management_key != false` (true or absent) | `{"data":{"total_credits":num,"total_usage":num}}` (both required) |
| `/activity` | same | `{"data":[{"date":str,"model":str,"provider_name"?:str,"usage":num,"requests":int,"prompt_tokens":int,"completion_tokens":int,"reasoning_tokens":int}]}` (all but provider_name required) |

`fetchSnapshot(now)`: decode `/key` (failure → `badResponse(.openRouter, "Unexpected JSON: <desc>")`;
HTTP errors propagate → 401/403 count as strikes). If `isManagementKey == false` → build without
credits/activity. Else fetch `/credits` and `/activity` **concurrently**; any fetch or decode failure
of either → treated as nil (degrades quietly).

### 4.3 Snapshot (`OpenRouterSnapshot.build(key, credits?, activity?, fetchedAt)`)

Base fields: `keyLabel = label`, `isManagementKey = flag ?? false`, `isFreeTier = flag ?? false`,
`accountCreditsUSD = credits.total_credits`, `accountUsedUSD = credits.total_usage`,
`keyUsageUSD = usage`, `keyUsageDaily/Weekly/MonthlyUSD = field ?? 0`, `keyLimitUSD = limit`,
`keyLimitRemainingUSD = limit_remaining`, `usedUSD = usage`, budget nil.

1. If `credits` and `total_credits > 0`: `budgetSource = accountCredits`, `budgetUSD = total_credits`,
   `usedUSD = total_usage`, `remainingUSD = max(0, total_credits − total_usage)`.
2. Else if `limit > 0`: `budgetSource = keyLimit`, `budgetUSD = limit`;
   `windowUsage` = (only when `limit_reset` is set) `limit_remaining.map { max(0, limit − it) }` ??
   usage field matching lowercased `limit_reset` (`daily`→usage_daily, `weekly`→usage_weekly,
   `monthly`→usage_monthly, other → nil);
   `usedUSD = windowUsage ?? usage`;
   `remainingUSD = limit_remaining ?? windowUsage.map { max(0, limit − it) } ?? max(0, limit − usage)`.
3. Else: no budget (`usedPercent` nil, spend only).
4. `models = OpenRouterModelUsage.models(activity ?? [])`.
5. `usedUSD = max(0, usedUSD)` (overdraft clamps).

Derived: `usedPercent = budgetUSD > 0 ? clamp(used/budget×100) : nil`;
`budgetStatTitle` = `Credits purchased` / `Key spending limit` / `Credit limit` (no source);
`budgetStatValue = Format.usd(budgetUSD)` or `None`; `budgetSource.label` = `Account credits` /
`Key limit`.

### 4.4 Activity → models

- `canonicalSlug(slug)`: lookup lowercased slug in revealed map `{"stealth/ox-alpha": "z-ai/glm-5.3-flash"}`;
  hit → mapped value, else original slug (case preserved).
- Group rows by canonical slug: `activitySlug` = first row's `model`; sums of
  `max(0, requests)`, `max(0, prompt_tokens)`, `max(0, completion_tokens)`, `max(0, usage)` as cost.
  (`reasoning_tokens`, `date`, `provider_name` unused.)
- For groups with cost ≤ 0: `estimatedValueUSD(slug: modelID, prompt, completion)` = bare id = last
  `/` segment of canonical slug → `OpenCodeZenCostEstimate.estimate(modelID: bare, input: prompt, output: completion, cache 0/0)`
  (catalogue table, `-contributor`/`-free` suffix stripping only; `:free` is **not** stripped). If
  `> 0` set cost and `isCostEstimated = true`.
- Sort cost desc then `totalTokens` (prompt+completion) desc; `percentOfWindow = cost/Σ×100`.
- `isRevealed = modelID != activitySlug`; `id = activitySlug`.

### 4.5 Poller (`OpenRouterUsagePoller`)

- `signedOut` → `clearSnapshot()` (snapshot, lastError, lastRefreshedAt).
- Refresh: gate; no key → `needsSignIn = true`, if no snapshot
  `lastError = "Add an OpenRouter API key to load usage."`, `.skipped`. If `!isSignedIn || needsSignIn`
  (rejected key) → `.skipped` without fetching (polls stop until a new key is saved).
  Else `ProviderRefresh.run(label: "OpenRouter")`; publish sets snapshot, clears error, sets
  `lastRefreshedAt`, `needsSignIn = false`.

### 4.6 Panel (`OpenRouterPanelView`)

- Signed-out (needsSignIn && no snapshot): header `OpenRouter`; text
  `Paste an OpenRouter API key to track spending against your purchased credits.`; key entry.
- Key entry: secure field placeholder `sk-or-v1-…`; button `Add Key` (or `Save Key` while replacing),
  disabled while the trimmed draft is empty; `auth.lastAuthError` in red below. Save →
  `saveAPIKey`; on success clear draft, exit replace mode, trigger `refreshNow()`.
- Main: header `OpenRouter` with meta label `budgetSource.label` or `No credit limit`.
  Card: header row `Credit Budget` with trailing `"<round(usedPercent)>% Used"` when a percent
  exists; then either a track (label hidden, `Credits used`) with caption
  `"$used of $budget · $remaining left"` or caption
  `This key has no credit limit, so usage shows as spend only.`.
  Stats card (header `Stats`): `budgetStatTitle`=`budgetStatValue`; `Total spent` =
  `usd(accountUsedUSD ?? keyUsageUSD)`; `Remaining` = `usd(remainingUSD ?? 0)`; `Spent today` =
  `usd(keyUsageDailyUSD)`.
  If models: card `Top Models by Spend` (not collapsible), first 3 + `View all`/`Show less`; row:
  `modelID` (canonical slug), meta tag `revealed` if revealed, cost `Format.usd` with `~` prefix when
  estimated, share bar (OpenRouter accent).
  Stale-error caption; replace-key row: key entry when replacing or `needsSignIn`, else link button
  `Replace API Key…` (clears draft, enters replace mode); sign-out (also clears draft/replace mode).
- Else: header + `Refreshing…`/lastError/`No usage data yet.` + key entry.

---

## 5. On-disk files touched by this slice

| File | Owner | Access | macOS path | Windows mapping (proposed) |
|---|---|---|---|---|
| Claude Code transcripts `**/*.jsonl` | Claude Code | read-only, incremental | `~/.config/claude/projects`, `~/.claude/projects`, or `$CLAUDE_CONFIG_DIR/*/projects` | `%USERPROFILE%\.config\claude\projects`, `%USERPROFILE%\.claude\projects`, `CLAUDE_CONFIG_DIR` (verify) |
| `claude_code_daily_tokens.dat` (ledger JSON v2) | TokenMon | read/write | `~/Library/Application Support/TokenMon/` | Core app-data dir (e.g. `%APPDATA%\TokenMon\`) |
| `claude_hourly_today`, `claude_daily_usage` stores | TokenMon (Core stores) | read/write | app support | app data |
| `opencode.db` (+ `-wal` fingerprint only) | OpenCode | read-only SQLite | `~/.local/share/opencode/opencode.db` | `%XDG_DATA_HOME%\opencode` or `%USERPROFILE%\.local\share\opencode\opencode.db` (verify) |
| `opencode_auth_workspace.dat`, `*_auth_email.dat`, `*_auth_account.dat` | TokenMon | read/write | app support (0600) | app data (user-only ACL) |
| Secrets `claude_auth_session`, `chatgpt_auth_session`, `opencode_auth_session`, `openrouter_auth_key` | TokenMon | vault | Keychain vault | Core secret store (Credential Manager / DPAPI) |

---

## 6. Behaviour flags / parity quirks worth preserving (or consciously fixing)

1. ChatGPT `primary_window: null` blocks the `five_hour` fallback (Swift `??` on a present NSNull).
2. ChatGPT account-id fallback scan order is unspecified in Swift.
3. OpenCode `modelUsage.sessionCount` overwrite quirk with anonymous rows (§3.6).
4. OpenCode monthly window with no Go session anchors at `now` (monthly ≈ $0).
5. OpenCode rolling window has no upper time bound.
6. Claude transcript decode is type-strict per declared field (a string token count drops the line).
7. Claude `parseCompleteLines` consuming an unterminated-but-valid JSON tail forces a full re-parse
   next time (correct but slower).
8. Claude org-id cookie name match is case-sensitive (`lastActiveOrg`); the allowlist is
   case-insensitive.
9. OpenRouter `:free` slugs are not price-normalised (stay $0).
10. Test `testAccountIDFromAccessToken` deliberately uses the misspelt claim `chatggpt_account_id`
    to exercise the `account_id` substring fallback.

---

## 7. Test parity checklist

Fixtures are inline in the Swift tests. "UTC cal" = Gregorian/UTC.

### 7.1 `Claude/ClaudeCodeUsageLedgerTests.swift` (temp dir store, prefix `claude_code_`)

| Test | Asserts |
|---|---|
| `testCostUsesEachRateIncludingBothCacheLifetimes` | 1M of each of 5 token kinds on `claude-opus-5-5` → `$37.20` (4+20+0.20+5+8) |
| `testPricingIgnoresSnapshotDateSuffix` | `baseModelID("claude-haiku-4-5-20251001") == "claude-haiku-4-5"`; unchanged for `claude-opus-5-5`; rates found for dated id |
| `testUnknownModelHasNoPrice` | `claude-unknown-9` → no rates, cost nil |
| `testModelDisplayNames` | `Opus 5.5`, `Fable 5.1`, `Haiku 4.5` |
| `testMergeKeepsTheLargerCountsAndDaysNoLongerScanned` | saved {09-01 opus in50; 10-01 opus in10/out90} ⊕ scanned {10-01 opus in30/out40, sonnet in5} → 09-01 in 50 kept; 10-01 opus in30 out90; sonnet in5 |
| `testLedgerKeepsDaysAfterTheirTranscriptsAreDeleted` | ledger A merges 09-01; new ledger B on same store merges 10-01 → both days present |
| `testLedgerDiscardsAnEarlierFormat` | stored `{"version":1,…10-01…}` → merged lacks 10-01, has new 10-02 |
| `testLedgerDaysAreUTC` | ledger calendar offset 0 |
| `testSpendMarksLeftOutUnpricedUsage` | `$12.50`, `$12.50+` |
| `testLedgerStartsEmptyWhenTheSavedFileIsUnreadable` | stored `not json` → merged has only the 1 scanned day |
| `testSummarySplitsMonthFromAllTimeAndRanksModelsByValue` | days 09-30 opus out1M; 10-01 opus+sonnet out1M; 10-22 sonnet out1M; now 2026-10-22T12Z → month tokens 3M, $40; all-time 4M, $60; ranking [sonnet, opus]; shares [50,50]; first cost $20 |
| `testSummaryFlagsUnpricedModels` | opus out1M + unknown out500 → month.hasUnpriced; unknown isUnpriced, share 0; first share 100 |
| `testSummaryIsEmptyAtTheStartOfAMonth` | data only 09-30, now 10-01T00:30Z → month 0, no models, all-time 10 |

### 7.2 `Claude/ClaudeUsageClientTests.swift`

| Test | Asserts |
|---|---|
| `testParseWindows` | fixture five_hour 42.5 (`resets_at 2026-08-21T14:00:00.127279+00:00`), seven_day 87.0 (`2026-08-27T11:00:00+00:00`) |
| `testParseResetDates` | 5h reset 14:00 UTC; weekly hour 11 (no-fraction form) |
| `testParseMissingWindowsYieldsNil` | `{}` → both nil |
| `testParseClampsOutOfRangeUtilization` | 130 → 100 |
| `testParseHTMLBodyIsUnauthorized` | `<!doctype html>…Sign in to Claude…` → `unauthorized(.claude)` |
| `testParseMalformedBodyIsBadResponse` | `not json` → `badResponse` |
| `testOrganizationIDFromCookieHeader` | `__cf_bm=abc; lastActiveOrg=9d7a…678; sessionKey=…` → uuid; `sessionKey=abc` → nil; `""` → nil |
| `testParseFallsBackToWeeklyLimitWhenSevenDayIsNull` | limits fixture (session 14 active unscoped; weekly_scoped 3 scope Fable inactive, reset 2026-10-11T08:00Z) → weekly 3, scope `Fable`, reset matches; 5h 14; label `Weekly · Fable` |
| `testParsePrefersAccountWideWeeklyLimit` | weekly scoped 80 active vs unscoped 20 inactive → 20, no scope, no reset |
| `testParseSevenDayObjectWinsOverLimits` | seven_day 40 beats limits weekly 5 |

### 7.3 `Claude/ClaudeCodeLocalStatsTests.swift` (helper line: model `claude-opus-4-1`, in10/out20/cr300/cw40, optional 1h split)

| Test | Asserts |
|---|---|
| `testParsesAssistantUsageAndSkipsOtherLines` | user line, valid, `<synthetic>`, `not json` → 1 entry; tokens 10/20/300; 5m 40, 1h 0; key `msg_1:req_1` |
| `testParsesASliceOfALargerBuffer` | parsing a sub-slice with non-zero start index works; keys `msg_1:req_1`,`msg_2:req_2` |
| `testParsesTimestampsWithAndWithoutFractionalSeconds` | `…01.500Z` vs `…01Z` differ by 0.5 s |
| `testSplitsCacheWritesByLifetime` | total 100, 1h 70 → 5m 30, 1h 70 |
| `testOneHourCacheWritesNeverExceedTheTotal` | total 10, 1h 50 → 5m 0, 1h 10 |
| `testDailyTokensDedupeAndGroupByDayAndModel` | duplicate msg_1/req_1 counted once; 10-10 opus responses 2, in 11, out 22, total 380; sonnet 1; 10-11 opus total 370 |
| `testDailyTokensUseTheCalendarsLocalDay` | 2026-10-10T20:00Z in Asia/Tokyo → key `2026-10-11` |
| `testScanReadsNestedTranscripts` | `projects/-Users-me-app/session.jsonl` + `…/session/subagents/agent.jsonl` → 2 responses |
| `testRepeatScansFollowAppendsRewritesAndRemovals` | 2 files → 2; partial append (20 bytes, no newline) → still 2; finish line → 3; rewrite shorter → 2; delete other → 1 |
| `testScanKeepsSeparateRunningTotalsPerCalendar` | UTC vs Tokyo keys `2026-10-10` vs `2026-10-11`; after append both counts 2; a newly asked calendar counts 2 total |
| `testScanIsNilWithoutClaudeCode` | nonexistent dir → nil |
| `testProjectsDirectoriesHonorsConfigDir` | no env → `[/Users/me/.config/claude/projects, /Users/me/.claude/projects]`; `CLAUDE_CONFIG_DIR="/a, /b"` → `[/a/projects, /b/projects]` |

### 7.4 `Claude/ClaudeUsagePollerTests.swift` (UTC calendar unless noted; stores in temp dirs)

| Test | Asserts |
|---|---|
| `testBuildDailyBudgetDaysAnchorsFirstBarToPeriodStart` | reset 2026-08-27 11:00, now 08-25 15:00 → 7 bars, Aug 20 … Aug 26, budget 100/7 |
| `testBuildDailyBudgetDaysEmptyWithoutResetTime` | resetsAt nil → [] |
| `testEarlyResetKeepsPriorDaysAndRestartsTheWeeklyWindow` | (local cal) polls 20%@−2d, 45%@−1d with reset +5d; then 2%@today with reset +7d → 7 live bars starting today, first spent 2; last prior-window bar 25; weekly pace start = today |
| `testSameResetInstantDropLeavesBarsUnchanged` | 60% then 10% same reset → 7 bars, no prior-window |
| `testBuildDailyBudgetDaysWeekOffsetShowsPriorPeriod` | spent {Aug 14: 6}, weekOffset −1 → first bar Aug 13, bar[1] = 6, no prior-window |
| `testProjectWeeklyResetCarriesPastInstantForwardByWholeWeeks` | 09-13 08:00 with now 10-10 09:00 → 10-11 08:00; future unchanged; nil → nil |
| `testRefreshBuildsBarsFromWeeklyLimitWhenSevenDayIsNull` | payload with limits weekly 3 reset 2099 → 7 bars, store lastUsedPercent 3, windowResetsAt set |
| `testRefreshWithoutResetTimeReusesPersistedReset` | store has reset 9 days ago; payload weekly 9% w/o reset → 7 bars; projected reset in (now, now+7d] |
| `testRefreshLoadsClaudeCodeUsage` | injected loader → codeUsage == summary; injected fetch without loader → codeUsage nil |
| `testClaudeCodeUsageLoadsWithoutSessionAndOnFetchFailure` | fetch throws → snapshot nil, codeUsage set; signed out → fetch never called, codeUsage set |
| `testSignOutKeepsClaudeCodeUsage` | `clearSnapshot()` keeps codeUsage |
| `testAttachTokensAddsEachLocalDaysTokens` | America/Denver; 2026-11-02 opus(10,20,1M,30,40)+sonnet(5,5)+mystery(out1) → in 15, out 26, cr 1M, cw 70, total 1,000,111; first model `Opus 5.5`; 3 models; cost > 0; hasUnpriced; next day tokens nil; spent kept 5 |

### 7.5 `ChatGPT/ChatGPTAccessTokenCacheTests.swift` (now = 1_790_000_000; JWT `h.<b64url {"exp":…}>.s`; session returns `["session=renewed"]`)

| Test | Asserts |
|---|---|
| `testTokenIsReusedUntilRenewDeadline` | exp +1h: fetch at t, t+60 → 1 exchange; at t+1801 → 2 exchanges; 3 usage calls |
| `testSessionCookiesAreReturnedOnlyWithAnExchange` | first fetch setCookies `["session=renewed"]`, cached second `[]` |
| `testRejectedCachedTokenIsReexchangedOnce` | 2nd usage throws unauthorized once → retried, primary 12; exchanges 2 |
| `testValidUntilHonoursTokenExpiryAndRenewInterval` | exp+10min → now+5min; exp+1d → now+30min; `opaque` → now+5min |
| `testCacheIsScopedToKey` | key "1" hit; key "2" miss; t+61 miss |
| `testTokenIsReusedAfterCookieRenewal` | new client with renewed cookie, same key → 1 exchange |
| `testWindowLabelsDeriveFromWindowSeconds` | 18000/3600/86400/604800/2592000/5400/nil → `5-Hour Window`,`1-Hour Window`,`Daily`,`Weekly`,`30-Day Window`,`Fallback`,`Fallback` |

### 7.6 `ChatGPT/ChatGPTUsageClientTests.swift`

| Test | Asserts |
|---|---|
| `testParseWindows` | plan `plus`, allowed, not reached, 12, 57.5 |
| `testParseWindowMetadata` | windowSeconds 18000 / 604800; reset epoch 1787300000 |
| `testParseAlternateFieldNames` | `{"five_hour":{"used_percent":30},"weekly":{"utilization":40}}` → 30 / 40 |
| `testParseMissingWindowsYieldsNil` | `{"plan_type":"free"}` → nil windows, plan `free` |
| `testParseClampsOutOfRangePercent` | 140 → 100 |
| `testParseInvalidPayloadThrows` | `nope` throws |
| `testSessionTokenParsing` | `accessToken tok-123` ok; `{}` throws |
| `testSessionTokenHTMLIsTransientNotUnauthorized` | `<!DOCTYPE html>…Just a moment...` → badResponse |
| `testAccountIDFromAccessToken` | `chatggpt_account_id` (fallback) → `acct_42`; `chatgpt_account_id` → `acct_alt`; `garbage` → nil |

### 7.7 OpenCode tests

`OpenCode/OpenCodePollerFallbackTests.swift`

| Test | Asserts |
|---|---|
| `testExpiredConsoleSessionFallsBackToLocalEstimate` | console always unauthorized, local estimate snapshot: poll 1 → not needsSignIn, estimated, `Console fetch failed — showing local estimate.`; after polls 2–3 → needsSignIn, estimated, source `Local estimate`, error `Console session expired — showing local estimate. Sign in again for official numbers.` |

`OpenCode/OpenCodeMonthBudgetWeekTests.swift` (schema: `session(time_created, cost, tokens_*, time_archived, model, id)`, `message(id, session_id, time_created, time_updated, data)`)

| Test | Asserts |
|---|---|
| `testMonthDailyBudgetWeekOffsetKeepsHistoryOutOfHeadlineScale` | Go msgs Jul 20 $6, Aug 18 $4; now Aug 20 (UTC), reset Sep 5, limit 60, used 20 → current week sum 20, Aug 18 = 20; weekOffset −4 → first bar Jul 20 = 10 (6/60×100), not prior-window |

`OpenCode/OpenCodeConsoleClientTests.swift`

| Test | Asserts |
|---|---|
| `testParseGoMeters` | fixture: 5h limit 12, used 0.16726299, reset set; week 30 / 3.59502844; month 60; monthResetsAt set |
| `testSnapshotPercentagesAndMonthlyResetFallback` | kinds [5h, weekly, monthly]; % 1.393858 / 11.983428 / 5.991714; monthly reset = endsAt; monthlyUsedPercent = monthly |
| `testParseGoMetersWithoutSubscriptionIsEmpty` | `{"access":null}` and `{}` → no meters |
| `testParseOrgs` | `[{"id":"wrk_01KTEST123",…},{"name":"no id"}]` → one id |
| `testParseSessionEmail` | `user.email` → `a@b.com`; `{}` → nil |
| `testMicroCentsToUSD` | `"3000000000"` → 30; 600000000 → 6; nil → nil |
| `testWorkspaceIDFromURL` | `/console/wrk_…`, `/workspace/wrk_…/go` → id; `/auth` → nil |
| `testLoginRedirectRecognizesConsoleLoginPages` | `/console/login`, `/auth/authorize`, `/auth/login` true; `/console/wrk_…` false |
| `OpenCodeConsoleWorkspaceTests.testPreferredWorkspaceWithSeatIsUsedWithoutListing` | seated preferred → org `wrk_redirect`; `/console/api/orgs` never requested |
| `…testOtherWorkspacesAreTriedForAGoSeat` | orgs [first, redirect, go]; only go seated → `wrk_go`, monthly 10% |
| `…testForbiddenEverywhereReportsDeniedWorkspace` | all orgForbidden → `badResponse("The OpenCode console denied access to this workspace.")` |
| `…testResolveOrgIDKeepsPreferredWorkspace` | preferred kept; nil → first listed |
| `…testServerErrorMessageCarriesNoBodyBytes` | 500 with body `secret-token` → `badResponse("HTTP 500")` |

`OpenCode/OpenCodePriceTableTests.swift`

| Test | Asserts |
|---|---|
| `testPricedModelsAreNotFlaggedUnpriced` | Zen `claude-haiku-5-5` 1M in at $0 → 0.10, estimated, not unpriced; recorded 1.5 on unknown model → not unpriced |
| `testRatesCarryAVerificationDate` | `ratesVerifiedOn` parses as a DayKey |

`OpenCode/OpenCodeLocalScanTests.swift` (now = 1_785_592_600; session model `{"id":"minimax-m3","providerID":"opencode-go"}`)

| Test | Asserts |
|---|---|
| `testSessionStartedBeforeRollingWindowCountsItsRecentMessages` | msgs −6h $1, −1h $3 → rolling $3, 1 session |
| `testLongSessionSplitsAcrossWeeksByMessageTime` | msg before week start $5, after $2 → weekly $2 |
| `testMonthlyTotalsComeFromTheMonthOnly` | anchor so month starts week.start+1d; msg in week before month $40; in month $2 (10/5/7/3) → monthly $2, est $2, in 10, out 5, total 25 |
| `testHeadlineAndDailyBarsValueZeroCostGoRowsAlike` | $0 Go minimax-m3 1M in → weekly > 0 and equals daily spends sum |
| `testMalformedMessageRowsAreSkipped` | `data='not json'` row ignored; rolling $1 |
| `testScanCacheReusesUnchangedDatabase` | same/later since → cached (1 load); earlier since → reload; DB changed → reload (3) |
| `testFetchesShareOneScanUntilTheDatabaseChanges` | 1 row + subscribedAt; after insert → 2 rows |

`OpenCode/OpenCodeModelNameTests.swift`: `testModelDisplayNames` — 8 examples in §3.9.

`OpenCode/OpenCodeStatsTests.swift` (now = 1_785_592_600 unless noted; `insert` writes a session row and mirrors an assistant message unless archived)

| Test | Asserts |
|---|---|
| `testAggregationWindowsAndModels` | Go minimax $4 + $2 in rolling, anthropic $90 −2d, Zen $3, Go kimi $500 −40d, archived kimi $99 → rolling $6/2 sessions/50%; weekly $6/2; monthly $6/2 (anchor −40d); Go model sessions 2, in 15M, out 1M, cr 3M, $6; no anthropic; top = minimax; primary 10%; Zen `deepseek-v4-flash-free` listed |
| `testZenDoesNotCountTowardGoLimits` | Zen $8 + Go $1 → rolling $1, 1 session, 8.33% |
| `testRollingResetTracksLastEligibleSession` | reset = last Go (−1h) + 5h; Zen −10m ignored |
| `testModelsUseWeeklyWindow` | Go $2 −3d → 1 model; primary 2/60×100 |
| `testDatabaseMissingThrows` | missing DB throws |
| `testClampedPercent` | 6/12→50; 15/12→100; 0→0; limit 0→0 |
| `testHasLocalStatsReflectsLocalContribution` | console-only false; with a model true |
| `testWindowLabelsAndLimits` | labels + 12/30/60 |
| `testWeeklyBoundsUTCMonday` | Wed 2026-07-15 13:00 → start 1_783_900_800 (Mon 07-13), 7 days; Sunday 07-19 → same start |
| `testMonthlyBoundsSubscriptionAnchored` | subscribed 1_779_114_281, now 1_785_596_400 → [1_784_384_681, 1_787_063_081] ±2 |
| `testMonthlyAggregationUsesSubscriptionWindow` | anchor + before-month $20 + in-month $7 + $3 → monthly $10, 2 sessions |
| `testScaledSpendsPercentAnchorsTotalToUsedPercent` | {5,15} to 30 → {7.5, 22.5} |
| `testScaledSpendsPercentUnchangedWhenNothingToAnchor` | used 0 → unchanged; empty stays empty |
| `testMonthDailyBudgetDaysAnchorsBarsToUsedPercent` | anchor Aug 1, spends {Aug4 3, Aug5 7}, used 30 → 7 bars, periodStart Aug 1, sum 30, 9 and 21 |
| `testMonthDailyBudgetDaysPrefersConsoleResetsAtOverLocalAnniversary` | reset Aug 20 → periodStart Jul 20 |
| `testMonthDailyBudgetDaysNilWithoutSignals` | no reset, no Go session → nil |
| `testMonthDailySpendsTrackGoOnly` | Go $3, Zen $5, deepseek $4 → only {3} |
| `testMonthlyBoundsClampsShortMonths` | Jan 31 → start Jan 31, end Feb 28 |
| `testGoEligibleProvider` | `opencode-go`, `OpenCode-Go` true; others false |
| `testPlanEligibleProvider` | `opencode-go`, `opencode`, `OpenCode` true; deepseek/xai/openai/anthropic false |
| `testBYOKProvidersExcludedFromModels` | models providers {opencode-go, opencode}; rolling $2 |
| `testOverviewCountsOnlyPlanUsage` | hour 10 has 2 segments; xai/deepseek in 11/12; quota weights with monthly 10: Go[10] = 30×4.348…, Zen[10] = 10×4.348…; hours 11/12 zero; tokens Go/Zen 1/1; Overview build shares (Core) |
| `testProviderHourlyUsageUsesQuotaDeltasWithoutDailyNormalization` | (Overview/Core) activity 3 and 0.5; grok share 66.67% |
| `testProviderHourlyUsageIncludesGrokbotShare` | (Overview/Core) activity 5, grokbot 80% |
| `testCatalogNames` | Go/Zen/DeepSeek; `DeepSeek · deepseek-v4-pro` |
| `testOverviewHourlyBarHeightIsProportional` | (Overview UI) heightFraction 0.5/1/0/0/1 |
| `testOverviewHourlyRelativeStackKeepsSmallProvidersVisible` | (Overview UI) 0.5 vs 1.0 |
| `testModelPaletteDeterministic` | same seed same colour, components in [0,1] |
| `testModelPaletteProviderColors` | Go → orange, Zen → purple |
| `testUnusedModelsExcludedFromList` | $0/0-token model dropped |
| `testZenZeroCostUsesPublishedTokenValue` | Zen `deepseek-v4-flash-free` 1M in + 0.5M out at $0 → $0.28 estimated; Go $2 not estimated |
| `testZenPublishedCostHelpers` | 1M in → $0.14 estimated; recorded 1.5 → 1.5 not estimated |
| `testMuseSparkFreeAliasEstimatesPaidEquivalentValue` | `muse-spark-1.3-contributor-free` 1M+1M → $0.30 |
| `testFreeAliasResolvesToBaseRates` | catalog `kimi-k3-free` 1M in → $3.00 |
| `testUnknownModelIsFlaggedUnpriced` | `totally-new-model-free` → (0, false, true) |
| `testMonthlyTokensAndEstimatedValue` | monthly tokens ≥ 2M; est $2.14 (±0.02) |
| `testDayHourlyUsageStacksModelsByLocalHour` | 24 hours, total $7, hour 9 $3 with 2 segs, hour 14 $4, hour 0 empty, legend excludes yesterday's `old`, first `minimax-m3` |
| `testHourlyAndModelsTrackMidSessionModelSwitch` | message model `gpt-5.6-luna` (Go) appears in hour 15 with cost > 0 and in models |
| `testPreviewSnapshot` | preview 3 windows, first rolling, primary 16.9/60×100 |
| `testMonthlyDailyBudgetUsesSubscriptionMonthDenominator` | reset Sep 5, now Aug 20 → first bar budget 100/31 |
| `testFetchDailySpendsFiltersToWindow` | Sep 10 $5, 16 $3, 17 $9, window [16,18) → {16:3, 17:9}, sum 12 (local-cal keys) |
| `testMonthlyDailyBudgetSpreadsWhenNoLocalSpends` | no rows, used 12, window Aug 5–Sep 5, now Aug 7 → bars sum 12, today > 0 |

### 7.8 OpenRouter tests

`OpenRouter/OpenRouterAuthSessionTests.swift`

| Test | Asserts |
|---|---|
| `testStartsSignedOut` | not signed in, needsSignIn, key nil |
| `testSaveRejectsNonOpenRouterKey` | `not-a-key` rejected with error; `"  sk-or-v1-abc123  \n"` saved trimmed |
| `testSavePersistsAcrossInstances` | new instance reads key |
| `testSignOutClearsKey` | key removed |
| `testMarkSessionInvalidKeepsKeyButPromptsReauth` | key kept, needsSignIn, error set |
| `testRejectedKeyIsNotCurrentUntilReplaced` | generation not current after invalidation; new key current |

`OpenRouter/OpenRouterModelsTests.swift`

| Test | Asserts |
|---|---|
| `testParseKeyPayload` | label, usage 25.5, daily 2.5, limit 100, management true |
| `testParseCreditsPayload` | 100.5 / 25.75 |
| `testBudgetPrefersAccountCredits` | credits 120/30 → accountCredits, 120, used 30, remaining 90, 25%, `Credits purchased` |
| `testBudgetFallsBackToKeyLimit` | non-mgmt usage 25 limit 100 → keyLimit, used 25, remaining 75, 25%, `Key spending limit`, value `$100.00` |
| `testNoBudgetYieldsNilPercent` | no limit → nil source/budget/remaining/percent; used 12.5; `None` |
| `testKeyLimitWindowUsesLimitRemainingWhenResetSet` | remaining 90, reset monthly → used 10, remaining 90 |
| `testKeyLimitWindowFallsBackToMatchingUsageField` | no remaining, weekly 14, reset weekly → used 14, remaining 86 |
| `testKeyLimitUnknownResetKeepsAllTimeUsage` | reset `hourly`, no fields → used 25, remaining 75 |
| `testZeroCreditBalanceHasNoDenominator` | credits 0, no limit → no source |
| `testNegativeUsageClampsToZero` | credits 8 / usage −0.5 → used 0, 0% |
| `testKeyParsingDefaults` | `{"data":{"usage":3}}` → defaults false/0 |
| `testParseActivityPayload` | 2 rows parsed incl. stealth tokens 3,489,882 / 110,726 |
| `testStealthModelRevealedAndValued` | `stealth/ox-alpha` → `z-ai/glm-5.3-flash`, revealed, estimated $0.185 |
| `testReportedCostIsNotEstimatedAndAggregates` | 2.1292 + 0.5 → 2.6292, requests 500, 100% |
| `testUnpricedFreeSlugStaysZero` | `moonshotai/kimi-k2.6:free` → $0, not estimated |

`OpenRouter/OpenRouterUsageClientTests.swift`

| Test | Asserts |
|---|---|
| `testEndpointURLsResolveUnderAPIVersionPath` | `/api/v1/key`, `/credits`, `/activity` |
| `testResolveTreatsBaseWithoutTrailingSlashAsDirectory` | (Core resolve) both forms → `/api/v1/key` |
| `testResolveKeepsRootRelativeAndHostOnlyPaths` | (Core resolve) root-relative replaces path |
| `testNonManagementKeySkipsAccountEndpoints` | only `.key` requested; keyLimit |
| `testManagementKeyFetchesCreditsAndActivity` | all 3 requested; accountCredits 40 |
| `testAccountEndpointFailureDegradesToKeyLimit` | credits/activity throw → keyLimit, no models |

---

## 8. Fuzz targets (untrusted input parsers)

| Target | Input | Invariants to check |
|---|---|---|
| Claude `parse_line` / `parse_jsonl` / `usage_line_ranges` / `parse_complete_lines` | arbitrary bytes (transcripts are user-writable, GB-sized) | no panic; `consumed <= len`; consumed ends at a newline or end; token counts ≥ 0; 5m + 1h == total write |
| `ClaudeCodeFileCache` refresh sequence | random append/truncate/rewrite ops on temp files | totals equal a from-scratch parse of current files (modulo dedupe ordering) |
| Ledger payload decode | arbitrary `claude_code_daily_tokens.dat` | no panic; bad → empty |
| `ClaudeUsageResponse::parse` | arbitrary JSON/HTML | percents in [0,100]; no panic on deep/odd `limits` |
| `organization_id_from_cookie_header` | arbitrary header strings | no panic, never returns empty |
| ChatGPT `ChatGPTUsageResponse::parse`, `parse_session_token` | arbitrary bytes | percents clamped; window seconds overflow safe (`Int(f64)` saturate) |
| ChatGPT JWT `account_id` / `expiry` | arbitrary token strings (bad base64, huge exp, NaN) | no panic; `valid_until` arithmetic doesn't overflow |
| `Set-Cookie` folding (`apply_refreshed_cookies`, Core) | arbitrary Set-Cookie lists | allowlist-only output |
| OpenCode `parse_go_meters`, `parse_orgs`, `parse_session_email`, `usd()` | arbitrary JSON | no panic; numeric strings like `"1e400"`, `"nan"` handled (Swift `Double("nan")` = NaN → must not produce NaN percent; clamp/guard) |
| OpenCode SQLite rows | crafted `opencode.db` (bad JSON, huge ints, text in numeric fields, NULLs) | no panic; skipped rows; query errors surface as errors |
| `monthly_bounds` / `weekly_bounds` | random instants incl. far past/future | start ≤ now < end; end − start within 28–31 days |
| `OpenCodeModelName::display`, `ClaudeModelName::display`, `base_model_id`, `model_id_variants` | arbitrary Unicode | no panic; variants terminate |
| `ModelPalette` hash | arbitrary strings | index in 0..8 (watch `i64::MIN.abs()` — use `unsigned_abs`) |
| OpenRouter `/key`, `/credits`, `/activity` decode + `OpenRouterSnapshot::build` | arbitrary JSON | used ≥ 0; percent in [0,100] or None; negative/huge numbers |
| `canonical_slug` / `estimated_value_usd` | arbitrary slugs | no panic |
