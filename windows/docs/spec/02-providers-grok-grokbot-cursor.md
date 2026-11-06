# Spec 02: Grok (SuperGrok), Grokbot, and Cursor providers

Source of truth (Swift, read in full for this spec):

- `TokenMon/Providers/Grok/*` (GRPCWebParser, GrokUsageClient, GrokUsageResponseParser, GrokModels, GrokAuthSession, GrokSignIn, GrokUsagePoller, GrokDailyUsageBuilder, GrokDailyUsagePreview, GrokHistoryStore, GrokHistoryImport, GrokExportService, GrokPanelView)
- `TokenMon/Providers/Grokbot/*` (GrokbotModels, GrokbotUsageClient, GrokbotUsagePoller, GrokbotPanelView)
- `TokenMon/Providers/Cursor/*` (CursorAuthSession, CursorSignIn, CursorModels, CursorUsageClient, CursorUsageEvents, CursorUsagePoller, CursorPanelView)
- Tests: `TokenMonTests/Providers/{Grok,Grokbot,Cursor}/*`

Shared Core types (`ProviderAuthSession`, `WebKitCookieCapture`, `ProviderHTTP`, `AuthenticatedRequest`, `ProviderRefresh`, `PollingLoop`, `DailyBudget`, `DailyQuotaDeltaStore`, `HourlyDeltaActivityStore`, `QuotaWindow`, `DailyTokenUsage`, `Format`, `JSON`, `Percent`, `Domain`, shared panel components) are specified elsewhere. This document records only how these three providers use them, plus the exact constants needed for parity.

Conventions in this doc:

- "Day" means a local calendar day (`Calendar.current`, the system time zone). `startOfDay(x)` is local midnight.
- `Percent.clamp(x)` = `max(0, min(100, x))`.
- `JSON.number(v)`: accepts JSON numbers (not booleans) and numeric strings (trimmed of spaces), descends into `{"val":…}` / `{"value":…}` wrappers, and rejects NaN or infinite values.
- `JSON.firstDouble(dict, keys)`: the first key, in order, whose value passes `JSON.number`.
- `JSON.firstString`: the first key whose value is a JSON string. There is no coercion.
- `JSON.firstBool(dict, keys, fallback=false)`: the first key holding a JSON bool, or a number (`!= 0`). Otherwise `fallback`.
- `ISO8601.parseFlexible(s)`: ISO-8601 with or without fractional seconds (e.g. `2026-07-13T00:00:00.000Z` and `2026-07-16T20:25:00Z`).
- `Domain.matches(host, hosts)`: case-insensitive. Strips one leading `.`. Matches when the host equals an entry or ends with `"." + entry`. A substring never matches (`evilgrok.com` does not match `grok.com`).
- `AppIdentity.userAgent` = `TokenMon/<version>`.

---

## 0. Provider identity table

| Field | Grok | Grokbot | Cursor |
|---|---|---|---|
| `MonitorProvider` id | `grok` | `grokbot` | `cursor` |
| Display name | `Grok` (panel title `SuperGrok`) | `Grokbot` | `Cursor` |
| Accent sRGB | (0.11, 0.38, 0.82) | (0.38, 0.42, 0.50) | (0.18, 0.53, 0.38) |
| Auth session | `GrokAuthSession` (own) | **shares** `CursorAuthSession` | `CursorAuthSession` |
| Error context displayName | `Grok` | `Grokbot` | `Cursor` |
| notSignedIn message | `Sign in to grok.com to load usage.` | `Sign in to Cursor to load your Grokbot allowance.` | `Sign in to Cursor to load usage.` |
| unauthorized message | `Session expired. Please sign in again.` | `Cursor session expired. Sign in again.` | `Cursor session expired. Sign in again.` |
| forbidden (non-challenge 403) message | n/a | `Grok Bot is not available on this Cursor account.` (a transient `.custom`, not unauthorized) | n/a |
| Pool cadence (`usagePool`) | weekly | from span: ≥25 days monthly, ≤10 weekly, else mixed; nil span → 7 → weekly | monthly |
| Section title | `Usage Pool Weekly` | `Usage Pool <Weekly/Monthly/Mixed>` | `Usage Pool Monthly` |
| `polls()` | polled when the selected tab is `grok` or `overview` | `grokbot` or `overview` (**not** `cursor`) | `cursor` or `overview` |

Error descriptions (`ProviderError.errorDescription`):

- `.badResponse(ctx, m)` gives `"<ctx.displayName> response error: <m>"`.
- `.network(ctx, m)` gives `"<ctx.displayName> network error: <m>"`. For example, the test expects `Grok network error: offline`.
- 429 (via `ProviderHTTP`) gives `.custom("<displayName> is rate limiting requests. Retrying later.")`.
- Bot challenge gives `Request blocked by a bot-protection check (HTTP <status>). Retrying.`. This counts as a `.badResponse`.

---

## 1. Grok (SuperGrok weekly pool)

### 1.1 Auth

| Item | Value |
|---|---|
| Sign-in sheet title | `Sign in to Grok` |
| Subtitle | `Sign in with your Grok / xAI account. This window finishes on its own once you're back on grok.com.` |
| Start URL (exact, percent-encoded) | `https://accounts.x.ai/sign-in?redirect=https%3A%2F%2Fgrok.com%2F%3F_s%3Dusage` |
| Auth (identity) hosts, allowed to load during sign-in, cookies never captured | `accounts.x.ai`, `auth.x.ai`, `api.x.com`, `twitter.com`, `x.com` (`Domain.matches`) |
| Return page (sign-in complete) | any URL whose host `Domain.matches(["grok.com"])`. `grok.com.evil.example` is rejected. |
| Return delay before capture | 1.5 s (`returnDelayNanoseconds = 1_500_000_000`). The default for other providers is 0.8 s. |
| Session/cookie hosts (`grokHosts`) | `grok.com`, `x.ai`. These are also the **sign-out hosts**. |
| `isGrokDomain(d)` | `Domain.matches(d, ["grok.com","x.ai"])`. Tests: `.grok.com` and `accounts.x.ai` pass; `evilgrok.com` and `x.ai.evil.example` fail. |
| Essential cookie allowlist | `sso`, `sso-rw` (names compared lowercased) |
| Preferred session cookie | name (lowercased) ∈ {`sso`,`sso-rw`} |
| `looksLikeAuthCookie` (unused once an allowlist exists) | name contains any of `sso, session, auth, token, jwt, sid, user, account, x-session, xai, oidc, refresh, access` **and** (secure or httpOnly) |
| `includeAllDomainCookiesWhenSessionFound` | true. This only matters without an allowlist, so it has no effect here. |
| Capture attempts | 4, with a 0.4 s delay between attempts (`retryDelayNanoseconds` default 400 ms) |
| Capture failure message | `No session cookies found. Finish signing in, then click Capture Session.` |
| Credential store prefix | `auth_` (keys `auth_session` in the vault, `auth_email.dat` etc. for files; see the Core credential spec) |
| Log category | `Auth` |
| Extra store keys | none |

Capture algorithm (with an allowlist), per `WebKitCookieCapture.select`:

1. Keep the cookies whose domain is a Grok domain.
2. Keep only the essential names.
3. Succeed only when one of them is preferred (`sso` or `sso-rw`).
4. Build the header as `name=value` pairs joined by `"; "`.
5. On every read, the stored header is narrowed again to essential cookies (`pruneCookieHeader`). If no essential cookie is present, the header is left unchanged.

Identity/email:

- Grok has no identity endpoint.
- At capture, the email is the value of a cookie named `email` / `user_email`, or any cookie whose percent-decoded value is a plausible `local@domain.tld`. Only `sso`/`sso-rw` are captured, so in practice this is almost always nil.
- A REST fallback payload may carry a top-level `email` string. It is used only when the client was given no account email.
- The poller fills `snapshot.accountEmail` from `auth.accountEmail` when the fetched snapshot has none.

Sign-in completion: the embedded browser navigates back to a grok.com page. After 1.5 s, cookie capture runs (up to 4 attempts × 0.4 s). Post-capture there are no provider-specific steps beyond the shared ones (store the credential, reset the strike count, poll).

### 1.2 HTTP endpoints

#### 1.2.1 Primary: gRPC-web `GetGrokCreditsConfig`

```
POST https://grok.com/grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig
Timeout: 15 s
Headers (exactly these):
  Cookie: <pruned cookie header>        (omitted if empty)
  Content-Type: application/grpc-web+proto
  x-grpc-web: 1
  x-user-agent: connect-es/2.1.1
  Accept: */*
  Origin: https://grok.com
  Referer: https://grok.com/?_s=usage
  User-Agent: TokenMon/<version>
Body: 5 bytes  00 00 00 00 00   (gRPC-web data frame: flag 0x00, big-endian length 0, empty message)
```

It is sent on the shared no-cookie-jar, no-cache session. A cancelled request surfaces as `CancellationError`, and the poller treats that as "skipped" (not an error). Other transport errors are thrown unmapped (the raw URL error). The client then tries the REST probes (below) and rethrows the original error if none succeed.

Response handling, **in this exact order**:

1. If the response is not HTTP, throw `.network(grok, "Invalid response")`.
2. Bot challenge (any status with a `cf-mitigated` header, or a 403 whose body starts, after trimming whitespace, with `<`): throw `.custom(botChallengeMessage(status))` with usage `.badResponse`. This is transient.
3. 401 or 403: throw `.unauthorized(grok)`.
4. Status ≠ 200: throw `.custom("Usage request failed (HTTP <code>).")` (usage `.badResponse` with the same message). The first 400 bytes of the body are logged privately and never shown.
5. **Header trailers-only check**: read HTTP header `grpc-status` and `grpc-message` (the message is percent-decoded; if decoding fails the raw text is used). Then apply `validateStatus`.
6. **Body trailer-frame check**: parse trailer frames (below), then apply `validateStatus(fields["grpc-status"], fields["grpc-message"])`.
7. `parseUsage(body, now)`.
8. If `parsed.products` is empty, then `products = synthesizeProducts(usedPercent)`.
9. Return `WeeklyUsageSnapshot(usedPercent, remainingPercent: max(0, 100-used), resetsAt, products, accountEmail)`.

`validateStatus(raw, message)`:

- If `raw` is nil, not an integer after trimming spaces, or `0`, do nothing.
- If status is `16` or `message.lowercased()` contains `unauthenticated`, throw `.unauthorized(grok)`.
- Otherwise throw `.custom("Usage request failed (HTTP <status>).")`. The gRPC status number is used as the "HTTP" code in the text.

#### 1.2.2 gRPC-web framing (parser)

Frame = `flags:u8, length:u32 big-endian, payload[length]`.

`dataFrames(bytes)`:

- Walk while `index + 5 <= len`.
- If any frame's `start + length > len`, return **an empty list** (all frames are discarded, including earlier good ones).
- Keep payloads whose `flags & 0x80 == 0` (data frames). The compression flag `0x01` is not handled; such a payload is treated as a plain message.
- Trailing bytes shorter than 5 are ignored.

`trailerFields(bytes)`:

- Same walk, but on overrun it `break`s and keeps the fields collected so far.
- For frames with `flags & 0x80 != 0` whose payload is valid UTF-8, split the text on any newline character (`\r` and `\n` each separate lines; empty lines are skipped).
- For each line with a `:`, the key is the text before the first `:`, trimmed of whitespace and lowercased. The value is the text after it, trimmed and then percent-decoded (`""` if percent-decoding fails).
- Later keys overwrite earlier ones.

Payload fallback: if `dataFrames` is empty **and** `looksLikeProtobuf(raw)`, the whole body is treated as one payload. `looksLikeProtobuf` means the first byte has `byte>>3 > 0` and `byte & 7` ∈ {0,1,2,5}. If there are still no payloads, throw `grokEmptyResponse` (`.custom("Empty usage response from Grok.")`, `.badResponse`).

#### 1.2.3 Protobuf scan (`scanProtobuf`)

This is a generic, schema-less walk. Output:

- `fixed32` entries: `(path, f32, order)`
- `varint` entries: `(path, u64)`

Each data payload is scanned at `depth=0`, `path=[]`. The `order` counter carries across payloads and increments once per fixed32 found.

For each field in the payload:

1. Read the key varint. If the varint is invalid or the key is `0`, return what has been collected so far.
2. `field = key >> 3`, `wire = key & 7`, `fieldPath = path + [field]`.
3. Handle by wire type:
   - wire 0: read a varint and record `(fieldPath, value)`. If the varint is invalid, return.
   - wire 1: skip 8 bytes. If fewer than 8 remain, return.
   - wire 2: read a length varint. If it is invalid or larger than the remaining bytes, return. If `depth < 4`, recursively scan the sub-bytes as a message with `depth+1` and `path=fieldPath`. This is done for **every** length-delimited field, so strings may produce garbage entries. Skip the sub-bytes either way.
   - wire 5: read 4 bytes little-endian as an IEEE-754 f32 and record `(fieldPath, f32, order++)`. If fewer than 4 remain, return.
   - Any other wire type: return.

`readVarint`: standard LEB128 over at most 64 bits of shift (`shift < 64`). Bytes are consumed until one has the high bit clear. It returns nil if input ends first or the shift reaches 64.

The maximum recorded path length is 5: depths 0 through 4 record paths, and depth 4 does not recurse.

#### 1.2.4 Observed `CreditsConfig` schema (from the live captures used as fixtures)

| Path | Wire | Meaning |
|---|---|---|
| `[1]` | 2 | root message |
| `[1,1]` | 5 (float) | **total used %** of the weekly pool |
| `[1,2]`, `[1,3]` | 2 | empty in captures |
| `[1,4]` | 2 | period start `Timestamp{1: seconds varint, 2: nanos varint}` |
| `[1,5]` | 2 | **reset** `Timestamp{1: seconds, 2: nanos}`. `[1,5,1]` is the canonical reset. |
| `[1,7]` (repeated) | 2 | product slice sub-message `{1: enum varint, 2: percent float}` |
| `[1,8]` | 2 | `{1: varint 1 or 2, 2: Timestamp, 3: Timestamp}` (period info) |
| `[1,11]`, `[1,13]` | 0 | `1` in captures (unused) |
| `[1,12]` | 2 | empty |

Product enum map (field 1 inside each field-7 message):

| Enum | id | displayName | Evidence |
|---:|---|---|---|
| 1 | `api` | `API` | inferred |
| 2 | `build` | `Grok Build` | live (30%) |
| 3 | `other` | `Other` | live (1%) |
| 4 | `chat` | `Chat` | live (21%) |
| 5 | `imagine` | `Imagine` | live (12%) |
| 6 | `voice` | `Voice` | inferred |
| other N | `product-N` | `Product N` | fallback, colour `other` |

#### 1.2.5 `parseUsage` derivations

**Used %:**

1. Take the first fixed32, in scan order, with path exactly `[1,1]` whose value is finite and in `0...100`.
2. Otherwise, among fixed32 entries with `path.last == 1`, `path.count <= 2`, finite, and in `0...100`, pick the minimum by (`path.count` ascending, then `order` ascending). So a top-level `[1]` float beats `[x,1]`.
3. Convert to Double (from f32).

**Products** (`parseProductsFromPayloads`). This uses a separate recursive walk over every payload:

- `scanForField7Products(bytes, start, end)`: walk the fields of `bytes[start..<end]`.
  - A key of 0 or an invalid key stops the walk.
  - wire 0: skip the varint.
  - wire 1: skip 8 bytes (if they don't fit, return what has been found).
  - wire 5: skip 4 bytes (same rule).
  - wire 2: read the length. If it is invalid or does not fit, return. If `field == 7`, parse the bytes as a product sub-message. Otherwise **recurse** into the bytes. There is no depth limit.
  - Any other wire type: return.
- `parseProductSubMessage`: walk the sub-message.
  - wire 0: field 1 sets `enum`.
  - wire 5: field 2 sets `percent` (f32 → f64).
  - wire 1 and wire 2 are skipped.
  - A key of 0, a truncated field, or an unknown wire type means **no product** (nil).
  - Valid only if `enum` and `percent` are both present and `0.05 < percent <= 100`. An enum with no percent, or with an explicit `0.0`, produces no product. This pairing **inside each sub-message** is required: zipping enums and percents flat misassigns Imagine's 12% to Voice.
- Merge across all payloads by lowercased id. Duplicate ids **sum** their percents, and `ProductUsage.init` clamps the result to 0...100.
- Sort with `ProductCatalog.sortForDisplay`, using display-order index (unknown = 99).
  - Swift sort with dictionary-ordered ties is effectively nondeterministic for multiple unknown ids. **The Rust port should use a stable sort keyed by (index, id).**

**Reset instant:**

- Candidates are all varints with value in `[1_700_000_000, 2_100_000_000]`, read as Unix seconds. Nanos are ignored.
- `canonical` = the minimum date among candidates at path `[1,5,1]`. It is kept even if it is in the past, so a lagging payload keeps its anchor and the week window rolls forward instead.
- Otherwise `reset` = the minimum candidate date strictly greater than `now`.
- Otherwise `nil`.

**Missing used %** (proto3 omits a 0.0 float):

- `hasPeriod` is true if any varint path starts with `[1,5]`, or path == `[1,8,1]` with value 1 or 2.
- `used = percent ?? (reset != nil && hasPeriod && fixed32.isEmpty ? 0 : nil)`.
- If `used` is still nil, throw `.custom("Could not parse usage response: gRPC usage percent missing")` (`.badResponse`).

`synthesizeProducts(used)`: if `used > 0.05`, return `[ProductUsage(id:"other", displayName:"Other", percentOfPool: used, colorToken: .other)]`. Otherwise `[]`.

`debugFieldDump` (debug builds only) prints `frame[i] bytes=N`, then `  f32 a.b.c = v` and `  vi  a.b = v`. A plausible timestamp gets a `// <date>` suffix. This is not needed for parity.

#### 1.2.6 REST fallback probes

These are only tried when the billing call threw something other than `.unauthorized` or cancellation. They run in order, stopping at the first success, and are skipped if the task is cancelled:

| # | URL |
|---|---|
| 1 | `GET https://grok.com/rest/subscriptions` |
| 2 | `GET https://grok.com/rest/user` |
| 3 | `GET https://grok.com/rest/billing/usage` |
| 4 | `GET https://grok.com/rest/usage` |

- Headers: `Cookie`, `Accept: application/json`, `Origin: https://grok.com`, `Referer: https://grok.com/?_s=usage`, `User-Agent`.
- Timeout 15 s.
- A probe counts only if: the transport succeeded, the status is **exactly 200**, the body is non-empty, and `GrokUsageResponseParser.parseJSON` returns a snapshot.
- Every other outcome, **including 401/403 and bot challenges**, moves on to the next probe. REST probes never mark the session unauthorized.
- If no probe succeeds, the original billing error is rethrown.

If `cookieHeader == nil`, the client throws `.notSignedIn(grok)` before any request.

#### 1.2.7 REST JSON parser (`GrokUsageResponseParser.parseJSON`)

1. Parse JSON. The root must be an object. Otherwise return nil.
2. Wrapper recursion: for each key in `["usage","data","subscription","billing","credits","result"]` in that order, if `dict[key]` exists and `parseAny(dict[key])` succeeds, return that result. Recursion depth is unbounded.
3. Read the percentages:
   - `used = firstDouble(["usedPercent","usagePercent","credit_usage_percent","percentUsed","used_percent","usage_percent"])`
   - `remaining = firstDouble(["remainingPercent","remaining_percent","percentRemaining"])`
   - `isPercent(v)` = v is nil, or finite with `0 <= v <= 100.5`. If either fails, return nil. Bare `percent`/`value` keys are deliberately ignored.
4. Products, from the first source present:
   - **Array form**: `products`, else `breakdown`, else `productBreakdown`. It must be an array of objects. For each item:
     - `id` = string `id` ?? `key` ?? `name` ?? `"other"`
     - `name` = `displayName` ?? `name` ?? `label` ?? `ProductCatalog.displayName(id)`
     - `pct` = `firstDouble(["percentOfPool","usagePercent"])`. If absent, drop the item.
     - The product id is `id.lowercased()`.
   - **Map form** (only if no array): `byProduct`, else `productUsage`, an object of `key → value`. Read `pct` as follows:
     - If the value is a number, use it. Swift `as? Double` also accepts JSON booleans as 1/0, which is a quirk; the Rust port may treat booleans as non-numbers.
     - If the value is an object, use `firstDouble(["percentOfPool","usagePercent"])`.
     - Otherwise drop the entry.
     - The product id is `key.lowercased()`, and displayName is `ProductCatalog.displayName(key)`.
5. `resetsAt = parseFlexible(firstString(["resetsAt","resetAt","reset_at","billingPeriodEnd","periodEnd","nextReset"]))`.
6. `credits = firstDecimal(["extraCredits","extraCreditsBalance","onDemandBalance","creditsBalance"])`. This is a Decimal; keep the written digits.
7. Product range check: `products.allSatisfy(isPercent)`. This is effectively a no-op, because `ProductUsage.init` already clamps to 0...100.
8. `inferredUsed = used ?? (sum(products.percentOfPool) > 0 ? sum : nil)`. If nil or not `isPercent`, return nil.
9. `remainingPercent = remaining ?? max(0, 100 - inferredUsed)`.
10. If products are empty, use `synthesizeProducts(inferredUsed)`.
11. `accountEmail = argument ?? dict["email"]` (string).
12. The snapshot init clamps used to 0...100, and remaining to 0...100.

### 1.3 Models and derived numbers

`ProductUsage { id, displayName, percentOfPool (clamped 0...100 at init), colorToken }`. When no colour token is given, it is `ProductColor.from(id)`:

| `ProductColor` | ids matched (lowercased) | sRGB |
|---|---|---|
| `build` | `build`, `grok_build`, `grok-build`, `grok build` | (0.55, 0.78, 1.0) sky blue |
| `api` | `api` | (0.22, 0.32, 0.48) dark navy |
| `chat` | `chat`, `grok`, `text` | Grok accent (0.11, 0.38, 0.82) |
| `imagine` | `imagine`, `image`, `video`, `media` | (0.75, 0.90, 1.0) pale blue |
| `voice` | `voice` | (0.40, 0.55, 0.82) mid blue |
| `other` | anything else | (0.45, 0.45, 0.45) |

`ProductCatalog`:

- `knownIDs` (preference toggle order): `build, api, chat, imagine, voice, other`. This is also the default visible set.
- `displayOrder` (bars, chips, legend): `chat, build, imagine, voice, api, other`.
- `categoryIDs` (Categories card): `chat, build, imagine, voice`. API and Other never appear there.
- `displayName(id)`: build→`Grok Build`, api→`API`, chat→`Chat`, imagine→`Imagine`, voice→`Voice`, other→`Other`. Any other id uses Foundation `.capitalized`, which capitalizes the first letter of each word and lowercases the rest.
- `shortName(id)`: build→`Build`, all others the same as `displayName`.
- `filtered(products, visible, threshold)`: walk `displayOrder`. Keep the id if it is in `visible`, a product with that id exists (later duplicates win), and `percentOfPool > threshold`. **Ids not in displayOrder are dropped.**
- `panelProducts(products, visible)` = `filtered(…, threshold: 0)`.
- `categoryProducts(products, visible)`: for each of `categoryIDs` that is in `visible`, use the existing product, or else a placeholder with `percentOfPool 0` and the canonical displayName.
- Settings `visibleProductIDs`:
  - The default is all `knownIDs`.
  - A persisted set is lowercased and intersected with `knownIDs`.
  - If the result is empty but the stored set was non-empty, fall back to all known ids.
  - An explicitly empty stored list stays empty.

`WeeklyUsageSnapshot` (Codable). Fields:

| Field | Type | Notes |
|---|---|---|
| `id` | UUID | new each poll |
| `fetchedAt` | Date | |
| `usedPercent` | Double | clamped 0...100 at init |
| `remainingPercent` | Double | at init: `clamp(remaining ?? 100 - clampedUsed)` |
| `resetsAt` | Date? | |
| `products` | `[ProductUsage]` | |
| `extraCreditsBalance` | Decimal? | |
| `accountEmail` | String? | |

`usagePool` is always weekly.

`DailyUsageWeek` (Grok's own chart model) has these fields:

- `weekStart`
- `weekEnd`
- `days: [DailyUsageDay]`
- `legendProducts`
- `hasDailyData`: any day has segments
- `isEstimated`

`rangeLabel` is `"<MMM d> – <MMM d>"`, using the locale's abbreviated month and day, e.g. `Jul 9 – Jul 15` (an en dash with spaces).

`DailyUsageDay` has these fields:

- `dayStart`
- `weekdaySymbol`: the first 3 characters of `EEE` in the current locale
- `dayOfMonth`: `d`
- `segments: [DailyUsageSegment{productID, displayName, percentOfWeekly, colorToken}]`
- `isToday`
- `isAfterReset`
- `totalPercent`: the sum of the segments

Constants:

- `dailyCapPercent = 100/7` (≈14.2857). The chart header shows it as `14.3%`.
- `fillFraction(p)`: `p <= 0.05` → 0, else `min(p / (100/7), 1)`.

#### 1.3.1 Billing-period week bounds (`billingPeriodWeekBounds(resetsAt, weekOffset, cal, now)`)

```
if resetsAt == nil -> nil
resetDay = startOfDay(resetsAt)
if now >= resetsAt:
    baseStart = resetDay
    today = startOfDay(now)
    repeat up to 520 times:
        if today <= baseStart + 6 days: break
        baseStart += 7 days
else:
    baseStart = resetDay - 7 days
shiftedStart = baseStart + weekOffset*7 days
shiftedEnd   = shiftedStart + 6 days
if weekOffset == 0 && now < resetsAt && sameDay(now, resetDay):
    return (shiftedStart, resetDay)       # 8 bars: the reset day is owned by the running period
return (shiftedStart, shiftedEnd)
```

All date math is calendar-day based (`date(byAdding: .day)`), so it is DST-safe.

#### 1.3.2 Daily bars (`GrokDailyUsageBuilder.week(history, current, weekOffset, resetsAt, calendar, now)`)

The poller calls it with `history = recent.reversed()` (oldest first, at most 200 rows for the active account), `current = snapshot`, and `resetsAt = snapshot.resetsAt`.

1. `effectiveResetsAt = resetsAt ?? current?.resetsAt`. If it is nil, or the bounds are nil, return **nil**. There is no chart without a provider reset, and a calendar week is never substituted.
2. `dayCount = max(1, daysBetween(weekStart, weekEnd) + 1)`. This is 7, or 8 on the reset morning.
3. `mergedSamples`: start with `history`. Append `current` if history is empty or `last.fetchedAt < current.fetchedAt`, **unless** it is a near-duplicate of the last row: `|Δused| < 0.05`, `|Δt| < 60 s`, and the same day. Then sort by `fetchedAt`.
4. `endOfDay`: build a map from day to snapshot, processing samples in time order:
   - If the day has no entry yet, store the sample.
   - Else, if `periodAdvanced(existing.resetsAt → sample.resetsAt)` and `sample.fetchedAt >= existing.fetchedAt`, replace it.
   - Else, if `periodAdvanced(sample.resetsAt → existing.resetsAt)`, skip the sample. It belongs to an older period after a newer one already won the day.
   - Else, if `sample.fetchedAt >= existing.fetchedAt`, replace it.
   - `periodAdvanced(a→b)` = both non-nil and `b - a > 12 h`.
   - `sameBillingPeriod(a,b)` = `!advanced(a→b) && !advanced(b→a)`. Two nils count as the same period.
5. `sortedDays` = the endOfDay keys, ascending.
   - `samplesThroughWeekEnd` = the sorted days `<= weekEnd`.
   - `samplesInWeek` = those `>= weekStart`.
6. `anchorResets` (the period that owns the displayed window):
   - `weekOffset == 0`: `current?.resetsAt ?? effectiveResetsAt`, falling back to the last in-week sample's `resetsAt`.
   - Past weeks: the last in-week sample's `resetsAt`. If there is none, use `expected = weekEnd + 1 day`. If the live reset falls on the same day as `expected`, use the live reset. Otherwise use `expected` at 12:00:00 local.
7. `samePeriodThroughWeekEnd` = the days in `samplesThroughWeekEnd` with `sameBillingPeriod(endOfDay[d].resetsAt, anchorResets)`.
8. `trackingStartDay` (mid-period server rebase): iterate consecutive pairs in `samePeriodThroughWeekEnd`. A pair qualifies when `dayUsed + 5 < prevUsed` and the period did **not** advance. The latest qualifying day wins, or nil if there are fewer than 2 days.
9. `validSampleDays` = `samePeriodThroughWeekEnd` filtered to `>= trackingStartDay`, when there is one.
10. For each `offset` in `0..<dayCount`, `dayStart = weekStart + offset days`, call `dayBar`:
    1. Set up:
       - `dayResets = endOfDay[dayStart]?.resetsAt`
       - `prevDayKey` = the last valid sample day `< dayStart` with `sameBillingPeriod(endOfDay[prev].resetsAt, dayResets)`
       - `prevCumulative = endOfDay[prevDayKey].usedPercent`
       - `isTrackingStart = sameDay(dayStart, trackingStartDay)`. `isAfterReset` starts with this value.
       - `isPeriodStart = sameDay(dayStart, weekStart)`
    2. If there is no sample for the day, or the day is not in `validSampleDays`, the bar is empty.
    3. `products = endOfDay[day].products ?? current.products ?? []` and `previousProducts = endOfDay[prevDayKey].products ?? []`.
    4. **If `prevCumulative` exists:**
       - If `dayCum + 5 < prev`: `isAfterReset = true`. If `dayCum > 0.05`, the segments are `segmentsFromProducts(products, fallbackTotal: dayCum)` (the absolute week-to-date).
       - Otherwise the segments are `growthSegments(prev, dayCum, previousProducts, products)`.
    5. **Else, if `dayCum > 0.05` and (`isTrackingStart` or `isPeriodStart`):**
       - If `isTrackingStart`, set `isAfterReset = true`.
       - If the last sorted day before `dayStart` exists and `periodAdvanced(itsResets → dayResets)`, set `isAfterReset = true`.
       - The segments are `segmentsFromProducts(products, dayCum)`.
    6. **Else, if `dayCum > 0.05`** and the last sorted day `< dayStart` and `>= weekStart` exists and `periodAdvanced(itsResets → dayResets)`: `isAfterReset = true`, and the segments are `segmentsFromProducts(products, dayCum)`.
    7. Otherwise the bar stays empty. The first mid-period sample is a baseline only.
11. `isEstimated` = `validSampleDays ∩ [weekStart, weekEnd]` has fewer than 2 days, **and** `weekOffset == 0`.
12. `finalize`:
    - Legend = the unique segment productIDs in first-seen order, then sorted by `displayOrder.firstIndex(id)` (99 when unknown). The id is not lowercased here.
    - `hasDailyData` = any day has segments.

The segment helpers referenced above:

- `segmentsFromProducts(products, fallbackTotal)`:
  - Keep products with `> 0.05` and sort them for display. Each becomes a segment with its absolute percent.
  - If none remain and `fallbackTotal > 0.05`, return one `other`/`Other` segment for the total.
- `growthSegments(prevUsed, dayUsed, prevProducts, products)`:
  - Compute `productDeltaSegments`: for each current product, `delta = max(0, pct - prev[id.lowercased()] ?? 0)`. Keep deltas above 0.05, sorted for display.
  - If that list is non-empty and its sum is above 0.05, return it.
  - Else, if `max(0, dayUsed - prevUsed) > 0.05`, return `distribute(total, products)`. Otherwise return `[]`.
- `distribute(total, products)`:
  - Keep products with `> 0.05`; call their sum `sum`.
  - If none remain or `sum < 0.05`, return one `other` segment with `total`.
  - Otherwise, for each visible product sorted for display, the share is `total * pct/sum`. Keep shares above 0.05.

Preview (`GrokDailyUsageBuilder.preview`) is UI preview data only:

- A synthetic reset of `2026-07-16T20:25:00Z`, with `now` = reset − 2 days, giving a Thursday-to-Wednesday window.
- The pattern (build, api, chat) per day is `(5,1,0)`, `(7,2,1)`, `(2,1,0)`, `(0,0,0)`, `(18,5,1)`, `(4,1,0)`, `(3,0,0)`.
- `isEstimated = false`.

### 1.4 History store (`GrokHistoryStore`)

- File: `<AppSupport>/TokenMon/grok_history.dat`. This is `FileBackedStringStore` with prefix `grok_` and key `history`, giving `<prefix><key>.dat`. Writes are atomic (a staging temp file, then rename). File mode is 0600 and the folder is 0700 (Core).
- Content is a UTF-8 JSON string of `{"version":1,"rows":[WeeklyUsageSnapshot…]}` using Swift's **default** `JSONEncoder`:
  - Dates are encoded as Double **seconds since 2001-01-01T00:00:00Z** (Apple reference date). This is not Unix time and not ISO.
  - UUIDs are uppercase hyphenated strings.
  - Decimal is a JSON number.
  - Nil optionals are omitted.
  - `colorToken` is the raw string (`build|api|chat|imagine|voice|other`).
  - Product keys are `id, displayName, percentOfPool, colorToken`.
  - Snapshot keys are `id, fetchedAt, usedPercent, remainingPercent, resetsAt?, products, extraCreditsBalance?, accountEmail?`.
  - **For the Windows port**: there is no macOS file to migrate, so the Rust format may differ. If cross-platform import is wanted, decode reference-date doubles.
- Load: decode the file and sort rows by `fetchedAt` ascending.
  - If the file exists but cannot be decoded, set `storeFailed = true`. The store then runs in memory and **never overwrites** the bad file.
  - If the file is missing, a legacy SwiftData `history.store` is imported once. This is macOS-only; Windows has no equivalent, so treat it as N/A.
- `recent` = the rows for `activeAccount` (exact `accountEmail` match; nil matches nil), newest first, capped at **200**. Disk is not capped.
- `setActiveAccount(a)`: if it changed, reload `recent`. The poller calls it at init with `auth.accountEmail`, and `append` calls it with `snapshot.accountEmail`.
- `append(s)`:
  1. Make `s.accountEmail` the active account.
  2. Let `last = recent.first`. If it is on the same day, `|Δused| < 0.05`, and `|Δt| < 60 s`, ignore `s` (noise).
  3. If `last` is on the same day and has the same usage (`|Δused|<0.05`, `|Δremaining|<0.05`, equal `resetsAt`, equal products (full equality, including colour), equal credits, equal account), ignore `s` (idle poll, no write).
  4. Otherwise remove all rows with the same account on the same day, and insert `s` in chronological position. This gives one end-of-day row per account per day.
  5. Reload `recent` and schedule a flush.
- Flush: debounce 400 ms (each append cancels and reschedules). `flush()` is synchronous and runs on app terminate. A write only happens when dirty, a store exists, and `!storeFailed`. If the write fails, the store stays dirty and the next append retries.
- `clear()`: empty rows and `recent`, mark dirty, flush immediately.
- `allSnapshots()`: all rows, all accounts, oldest first. Used by export.
- Settings UI:
  - Section header `SuperGrok History`.
  - Buttons `CSV…` and `JSON…`, and `Clear Local History` (destructive).
  - Footer `Clearing history does not reset your SuperGrok weekly pool.`
  - When `storeFailed`, show in red: `History storage is unavailable — usage is kept for this session only and will not survive a relaunch.`
  - Export errors show in red, using the error's localized description.

### 1.5 Export (`GrokExportService`)

The save dialog's default filename is `grok-usage.csv` or `grok-usage.json`. Input is `allSnapshots()`.

**CSV**:

- Header: `fetchedAt,usedPercent,remainingPercent,resetsAt,products,extraCredits,accountEmail`
- Lines are joined with `\n`, with **no trailing newline**.
- Per row:
  - `fetchedAt`: ISO-8601 UTC `yyyy-MM-dd'T'HH:mm:ss'Z'`. Not escaped.
  - `usedPercent`, `remainingPercent`: `%.2f`.
  - `resetsAt`: ISO-8601, or empty. Escaped.
  - `products`: `"<id>:<percent>"` joined by `|`. The percent uses Swift `Double.description` (e.g. `25.0`, `0.1`, `33.333333333333336`, shortest round-trip repr with at least one decimal). Escaped.
  - `extraCredits`: Decimal description (e.g. `4.1`), or empty. Escaped.
  - `accountEmail`: escaped, or empty.
- `csvEscape(v)`:
  1. If the first Unicode scalar is one of `=`, `+`, `-`, `@`, TAB, or CR, prefix `'`.
  2. Then, if the value contains `,`, `"`, LF, or CR, wrap it in `"`, doubling inner `"`.

**JSON**:

- Pretty-printed with **sorted keys**, dates ISO-8601 (`yyyy-MM-ddTHH:mm:ssZ`, no fractional seconds).
- The top level is an array of rows with keys `accountEmail?, extraCreditsBalance?, fetchedAt, products[], remainingPercent, resetsAt?, usedPercent`. There is no `id`.
- Product keys: `colorToken, displayName, id, percentOfPool`.
- Nil values are omitted.
- Apple's encoder escapes `/` as `\/` and uses `"key" : value` with 2-space indentation. Byte-identical output is not required by tests; the test only requires a JSON array.

### 1.6 Poller (`GrokUsagePoller`)

- Polling loop interval: `settings.pollInterval(.grok, menuIsOpen)`:
  - `nil` (parked) unless `needsPolling(.grok)`. That requires: Grok enabled **and** (the selected tab is `grok` or `overview`, **or** the menu-bar graph shows Grok, **or** the threshold alert watches Grok).
  - Otherwise `activePollSeconds` (default 60) while the menu is open, else `idlePollSeconds` (default 300). Toggling `menuIsOpen` wakes the loop.
- `refreshNow()` triggers an immediate fetch even when the tab isn't shown, as long as the provider is **enabled**. A disabled provider never fetches.
- `performRefresh` gating, in order:
  1. If Grok is disabled or a refresh is already running, skip.
  2. Set `isRefreshing`.
  3. If not (`auth.isSignedIn && !auth.needsSignIn`), set `lastError = "Sign in to grok.com to load usage."` and skip.
  4. Otherwise fetch with `(cookieHeader, accountEmail)` via `ProviderRefresh.run`.
- `ProviderRefresh.run` (shared):
  - Capture `sessionGeneration`.
  - On success, discard the result if the task was cancelled or the generation changed. Otherwise call `recordAuthSuccess()` and then publish.
  - On cancellation, return `.skipped`. No error is set.
  - On any other error, return `.skipped` if the generation changed. If the error is `.unauthorized` or `.notSignedIn`, call `recordAuthFailure`; three strikes invalidate the session. `lastError = error.localizedDescription`.
- Publish:
  - If `snap.accountEmail == nil`, fill it from `auth.accountEmail`.
  - Set `snapshot`, `lastError = nil`, and `lastRefreshedAt = now`.
  - Call `history.append(snap)` and `grokHourly.record(usedPercent, at: fetchedAt)`. The hourly store key is `grok_hourly_today`, which is file `activity_grok_hourly_today.dat` (Core store).
- A failed poll after a good one keeps the snapshot (stale data stays visible) and sets `lastError`.
- `signedOut` clears `snapshot`, `lastError`, and `lastRefreshedAt`. History and hourly data are kept.
- `accountReset` does the same and also calls `grokHourly.clear()`.
- Headline: `headlineUsedPercent = snapshot.usedPercent`, `headlineResetsAt = snapshot.resetsAt`, `alertAccount = auth.accountEmail`.
- `currentWeek` is cached on the key (`snapshot.id`, `[recent row ids]`, `startOfDay(now)`). It is nil without a snapshot or a reset. `week(weekOffset)` is computed directly when `weekOffset != 0`.

### 1.7 Panel (`GrokPanelView`)

The header row always shows the provider icon and the title **`SuperGrok`**.

States, in priority order:

1. **Session expired**, when `auth.needsSignIn && auth.lastAuthError != nil`:
   - Title row.
   - Text: `poller.lastError ?? "Session expired. Sign in again to load usage."` (secondary).
   - Button `Sign In Again…`.
2. **Signed in with a snapshot**: the usage view (below).
3. **Signed in without a snapshot**:
   - Title row.
   - Text: `Refreshing…` while refreshing, else `lastError ?? "Signed in — waiting for usage data."`.
   - If `needsSignIn`, the button `Sign In Again…`.
4. **Signed out**:
   - Title row.
   - Text: `Sign in to load your usage.`
   - Button `Sign In…`, with keyboard shortcut Cmd+S (use Ctrl+S on Windows).

Usage view, as a vertical stack with spacing 10:

1. **Pool card**
   - Header `Usage Pool Weekly`, trailing `"<Int(round(used))>% Used"`. Rounding is half away from zero.
   - `SegmentedUsageBar(panelProducts(snapshot.products, visible), height 8)`:
     - Draw the visible products with `> 0.05` in order, then a grey remainder slot if `100 - min(100, sum) > 0.5`. The remainder colour is the primary colour at 12% opacity.
     - Gaps are 2 px, each segment has min width 3 px, and the corner radius is 2. Width per slot = `max(3, usable * pct/100)`; if the total exceeds `usable`, scale all slots down proportionally. `usable = width - gap*(slots-1)`.
   - If `resetsAt` is set, the caption `Format.resetCaption(resetsAt)` = `"Resets " + format("EEE dd MMMM h:mma")` in the POSIX locale with `AM`/`PM` lowercased, e.g. `Resets Thu 16 July 8:25pm`.
2. **Categories card**, shown if `categoryProducts` is non-empty. It is empty only when all four are hidden.
   - Header `Categories`.
   - One row of up to 4 chips, in order Chat, Build, Imagine, Voice: a 7 px circle in the product colour, `shortName` (semibold), and `"<Int(round(pct))>%"`.
   - Absent products show `0%`.
   - The chip input is `categoryProducts(panelProducts(...), visible)`.
3. **Extra credits card**, only if `extraCreditsBalance > 0`: `Extra credits` with a USD amount on the right, formatted en_US `$#,##0.00` (e.g. `$4.10`, `$410.00`).
4. **Daily chart card**, only when `poller.week(weekOffset)` is non-nil (`DailyUsageChartView`). It renders above the following views, because the hover label can overflow.
   - **Header row**:
     - `Daily Budget` section header, then `14.3%`.
     - Spacer, then `week.rangeLabel` on the right.
   - **Navigation**: chevron left/right. `previous = offset-1` (unbounded); `next = min(0, offset+1)`; next is enabled only when `offset < 0`. The panel holds `weekOffset` state starting at 0.
   - **Bars**: one column per day in `week.days` (7, or 8 on the reset morning).
     - Track height 68, stem width 12, corner radius 5.
     - The background stem is the primary colour at 12% opacity.
     - The fill is **Grok accent only**; segments are not drawn per product. Its height is `max(6, 68 * fillFraction(totalPercent))`, capped at 68, drawn only if `fraction > 0` and the day is not in the future (`dayStart > startOfDay(now)`).
     - Under each stem: `"<max(0, Int(round(total)))>%"` and the first 2 characters of the weekday symbol. Future days are drawn at opacity 0.5.
     - A hovered column gets an outline.
     - The legend (`legendProducts`) is computed but **not rendered**.
   - **Hover label** (shared `DailyBudgetHoverLabel`), for each day as `DailyBudgetDay(date: dayStart, spent: totalPercent, budget: 100/7)` with allowance noun `weekly`:
     - Line 1: `EEEE, MMM d`.
     - Line 2 (not for future days):
       - prior-window day: `"%.1f%% of the weekly pool before the provider reset it early"`
       - budget ≤ 0: `No budget`
       - spent ≤ 0.001: `"0% of the %.1f%% daily allowance"`
       - otherwise: `"%.1f%% of weekly · %d%% of daily allowance"`
     - Grok days carry no tokens.
   - **Footer caption**:
     - When `weekOffset == 0`, the pace is `DailyBudget.paceHeadroom(days, periodConsumed: snapshot.usedPercent, elapsedDaysInPeriod: elapsedDaysThroughToday(from: weekStart), resetsAt: snapshot.resetsAt)`, and the caption is `paceCaption(pace)` (Core). For example: `On pace for today`, `4.0% usage left today`, `25.0% usage left today from unused prior days`, `No usage yet · 10.0% today`, `Usage 5.0% over today's allowance`.
     - If there is no pace caption, and the week `isEstimated` or has no daily data, show `Bars update between polls · week totals above`.
     - Otherwise, no footer.
   - Past weeks pass `periodUsedPercent` and `resetsAt` as nil.
5. If `lastError` is set, show `StaleDataCaption`: the error message and `Showing data from <relative time> ago.`
6. Sign-out button, a person-minus icon with tooltip `Sign out`. It opens an inline confirm `Sign out of Grok?` with a confirm (check icon) and a cancel (x icon). Confirming calls `auth.signOut()`.

The menu bar (shared renderer) uses `ProductCatalog.filtered(products, visible, threshold: 0.05)`.

---

## 2. Grokbot (Grok Bot, Cursor-backed)

### 2.1 Auth

Grokbot has no auth of its own. It reuses `CursorAuthSession` (§3.1), meaning the same `WorkosCursorSessionToken` cookie and the same three-strike counter. Grokbot rejections count against the Cursor session.

- Sign-in buttons on the Grokbot panel open the Cursor sign-in flow.
- Sign-out on the Grokbot panel calls `cursorAuth.signOut()`. Its confirm prompt says `Sign out of Cursor?` because it ends the Cursor session too.
- The account email comes from the Cursor session (`auth.accountEmail`).

### 2.2 Endpoint

```
POST https://cursor.com/api/dashboard/get-sand-usage-status
Headers: Cookie: <cursor cookie header>
         Accept: application/json
         User-Agent: TokenMon/<version>
         Referer: https://cursor.com/bot
         Origin: https://cursor.com
         Content-Type: application/json
Body: {}
Timeout: platform default (60 s)
```

This is the REST wrapper over `aiserver.v1.GetSandUsageStatusResponse` ("Sand" is the wire name; the desktop bundle is `com.anysphere.sand`). One endpoint serves both the Cursor-funded and the SuperGrok-funded channels.

HTTP mapping (shared `ProviderHTTP`):

| Response | Result |
|---|---|
| Transport error | `.network(grokbot, msg)` |
| Bot challenge (`cf-mitigated` header, or 403 with HTML) | `.badResponse` with the challenge message (transient) |
| Non-challenge 403 | `.custom("Grok Bot is not available on this Cursor account.")`, `.badResponse`. Does **not** count as a strike. |
| 401 | `.unauthorized(grokbot)` |
| 429 | `.custom("Grokbot is rate limiting requests. Retrying later.")` |
| other non-2xx | `.badResponse(grokbot, "HTTP <code>")` |
| 2xx | parse the body |

The expired-session case returns 307 to `api.workos.com/user_management/authorize`. The redirect is followed and lands on HTML.

### 2.3 Parsing (`parseUsageStatus(data, accountEmail, fetchedAt)`)

1. `ProviderHTTP.jsonObject` (Core):
   - If the body is not a JSON object:
     - If it looks like HTML (trimmed text starts with `<`): a login page gives `.unauthorized(grokbot)`. A login page contains a login marker (`type="password"`, `type='password'`, `sign in`, `sign-in`, `signin`, `log in`, `log-in`, `login`, `authkit`, `workos`) and no challenge marker (`cf-chl`, `challenge-platform`, `cf_chl_opt`, `just a moment...`, `attention required! | cloudflare`, `cf-browser-verification`), checked in the lowercased first 64 KiB. Any other HTML gives `.badResponse("Unexpected HTML response")`.
     - Otherwise `.badResponse("Malformed response body")`.
2. If `root.error` is a string and (lowercased) contains `not_authenticated` or `unauthor`, throw `.unauthorized(grokbot)`.
3. **Percent**: `JSON.firstDouble(["usagePercent","usage_percent"])`.
   - If that is absent and any allowance field is present, the percent is `0`. The allowance fields are: `currentPeriodStart`, `current_period_start`, `nextResetTimestampUtc`, `next_reset_timestamp_utc`, `includedLimitZero`, `included_limit_zero`, `hasNonZeroIncludedLimit`, `has_non_zero_included_limit`, `includedUsageSuperGrokPlan`, `included_usage_super_grok_plan`, `grokPlanLabel`, `grok_plan_label`. "Present" means the key exists, even with a null value.
   - Otherwise throw `.custom("No Grok Bot allowance on this account. Grokbot needs a Cursor or SuperGrok plan that includes it.")` (`.badResponse`).
4. `hasIncludedAllowance = !firstBool(["includedLimitZero","included_limit_zero"]) && firstBool(["hasNonZeroIncludedLimit","has_non_zero_included_limit"], fallback: true)`.
5. Dates (`periodStart` from `currentPeriodStart`/`current_period_start`, `resetsAt` from `nextResetTimestampUtc`/`next_reset_timestamp_utc`). For each key in order:
   - If the value is a string that `parseFlexible` accepts, use it.
   - Else, if the value is an object with `JSON.number(seconds) > 0`, the date is `seconds + (nanos ?? 0)/1e9`. `{seconds: 0}` counts as missing.
   - Otherwise nil. A missing reset is **never substituted**.
6. **Entitlement**:
   - `plan = firstString(["includedUsageSuperGrokPlan","included_usage_super_grok_plan"])`, trimmed. If it is nil or empty, the entitlement is `.cursor`.
   - Otherwise `label = firstString(["grokPlanLabel","grok_plan_label"])`, trimmed. The entitlement is `.superGrok(planLabel: label non-empty ? label : plan)`.
7. The snapshot's `usedPercent = clamp(percent)`, and `accountEmail` is the one passed in.

`captionText`:

- `.cursor` gives `via Cursor`.
- `.superGrok(l)` gives `via <l>`, or `via SuperGrok` when `l` is empty.

### 2.4 Models

`GrokbotSnapshot` has these fields:

- `fetchedAt`
- `usedPercent`: clamped
- `periodStart?`
- `resetsAt?`
- `entitlement`
- `accountEmail?`
- `hasIncludedAllowance`: default true
- `remainingPercent`: `clamp(100 - used)`

Derived:

- `reportedSpanDays()`: nil if either end is missing or `resetsAt <= periodStart`. Otherwise `DailyBudget.daysInBillingCycle(start, end)` = `max(1, calendarDays(startOfDay(start), startOfDay(end)))`.
- `daysInPeriod()`: if the reported span is in `6...8`, use it. Otherwise use **7**. This covers a missing end, the same-day collapse (span 1), a 5-day span, and a fortnight (14).
- `usagePool`: `days = reportedSpanDays() ?? 7`. `days >= 25` is monthly; `days <= 10` is weekly; anything else is mixed. This uses the raw, unsanitized span.

### 2.5 Poller (`GrokbotUsagePoller`)

- Interval and gating are the same as Grok, but use `.grokbot`. It needs the `grokbot` or `overview` tab, the menu-bar graph, or an alert watch.
- `performRefresh`:
  1. If disabled or already refreshing, skip.
  2. If the Cursor cookie header is empty, set `lastError = "Sign in to Cursor to load your Grokbot allowance."` and skip. `needsSignIn` is not touched.
  3. Fetch via `ProviderRefresh.run` with the Cursor session.
- Publish:
  1. Set `snapshot`, clear the error, set `lastRefreshedAt`.
  2. If `resetsAt` is non-nil, remember it as `weeklyResetsAt`.
  3. `transition = daily.record(windowUsedPercent: used, at: fetchedAt, window: QuotaWindow(start: periodStart, resetsAt: resetsAt), periodDays: daysInPeriod())`. Daily store key `grokbot_daily_usage`, file `activity_grokbot_daily_usage.dat` (Core `DailyQuotaDeltaStore`).
  4. Handle the transition: `.rollover` calls `hourly.beginNewWindow()`; `.earlyReset` calls `hourly.beginNewWindow(keepingHours: true)`; `.none` does nothing.
  5. Call `hourly.record(used, at: fetchedAt)`. Hourly key `grokbot_hourly_today`.
  6. `budgetReferenceNow = fetchedAt`.
  7. `dailyBudgetDays = buildDailyBudgetDays(spentByDay: daily.spentByDay, resetsAt: resetsAt ?? weeklyResetsAt, daysInPeriod, windowStart: daily.windowStart, interruptedWindowStart: daily.interruptedWindowStart, now: fetchedAt)`.
- `buildDailyBudgetDays(...)`: if `resetsAt` is nil, return `[]`. Otherwise `DailyBudget.buildWeeklyWindowDays(limitUSD: 100, daysInPeriod, resetsAt, spentByDay, windowStart, interruptedWindowStart, weekOffset, now)`.
  - Units are pool-% (limit 100), so `budget/day = 100/daysInPeriod`.
  - Bars run from period start to the day before reset. Before the reset instant on reset morning, the first bar stays the old period start.
  - After an early reset, the preserved prior-window days lead the bars, flagged `isPriorWindow`. These behaviours live in Core.
- `dailyBudgetDays(weekOffset:)` rebuilds from the live store with `resetsAt = snapshot?.resetsAt ?? weeklyResetsAt`, `daysInPeriod = snapshot?.daysInPeriod() ?? 7`, and `now = budgetReferenceNow ?? Date()`.
- `signedOut` (from the shared Cursor session) clears `snapshot`, `dailyBudgetDays`, `weeklyResetsAt`, `budgetReferenceNow`, `lastError`, and `lastRefreshedAt`. `accountReset` also clears the hourly and daily stores.
- Headline: `snapshot.usedPercent`, `snapshot.resetsAt`, `alertAccount = cursorAuth.accountEmail`.

### 2.6 Panel (`GrokbotPanelView`)

The title row is `Grokbot`.

1. **Signed out** (`auth.needsSignIn && snapshot == nil`):
   - Text: `Sign in to Cursor to load your weekly Grokbot allowance. Grokbot is billed through Cursor even when a SuperGrok plan pays for it.`
   - Button: `Sign In to Grokbot…` (the default title).
2. **Snapshot**:
   - **Pool card**:
     - Header `Usage Pool <pool>`, trailing `"<Int(round(used))>% Used"`.
     - Caption `entitlement.captionText` (secondary).
     - If `hasIncludedAllowance`: a `SlimUsageTrack`. Its own label row is hidden. It is an 8 px capsule track in the Grokbot accent at width `clamp(used)%`, with a reset caption `Resets …` when `resetsAt` is set.
     - Otherwise the text `This plan has no included Bot allowance — usage bills on demand.`
   - If `hasIncludedAllowance`, a `NavigableDailyBudgetCard(style: .weekly, accent: grokbot accent, periodUsedPercent: used, resetsAt: snapshot.resetsAt, daysForWeek: poller.dailyBudgetDays(weekOffset:))`:
     - The card is hidden when the days are empty (no reset ever observed).
     - Weekly pace uses the first non-prior bar as the period start.
     - Allowance noun `weekly`. Title `Daily Budget`.
     - The live footer only appears at offset 0.
     - Past weeks pass the used % and reset as nil.
   - If `lastError` is set, `StaleDataCaption`.
   - If `auth.needsSignIn`, the button `Sign In Again…`.
   - The sign-out button names **Cursor**.
3. **No snapshot**:
   - Text `Refreshing…` / `lastError ?? "No usage data yet."`.
   - A sign-in button if `needsSignIn`. This is unreachable, because branch 1 catches that case first.

---

## 3. Cursor

### 3.1 Auth (`CursorAuthSession`)

| Item | Value |
|---|---|
| Sign-in title | `Sign in to Cursor` |
| Subtitle | `Sign in to your Cursor account. This window finishes on its own once you reach the dashboard.` |
| Start URL | `https://cursor.com/dashboard/usage` |
| Auth hosts (`isAuthPage(host, path)`) | `Domain.matches(host, ["authenticator.cursor.sh","authenticator.cursor.com","accounts.google.com","github.com"])` **or** the path contains `/login` or `/signin` |
| Return page | host (lowercased) is `cursor.com` or ends with `.cursor.com`, **and** the path contains `/dashboard` or `/settings` |
| Return delay | 0.8 s (default) |
| Session/cookie hosts = sign-out hosts | `cursor.com`, `cursor.sh`, `authenticator.cursor.sh`, `api2.cursor.sh` |
| `isCursorDomain(d)` | `Domain.matches(d, those hosts)` |
| Essential allowlist | `workoscursorsessiontoken` (compared lowercased) |
| Preferred session cookie | name == `WorkosCursorSessionToken`, or lowercased == `workoscursorsessiontoken` |
| `looksLikeAuthCookie` (moot with an allowlist) | name `workoscursorsessiontoken`, or contains `session`/`token`/`auth`/`workos`/`cursor` |
| Capture attempts | 4 × 0.4 s |
| Failure message | `No Cursor session cookie found. Finish signing in until you see the usage dashboard, then click Capture Session.` |
| Store prefix | `cursor_auth_` |
| Log category | `CursorAuth` |

`isAuthPage` tests:

- True: `github.com/session`, `authenticator.cursor.sh/`, `accounts.google.com/o/oauth2`, `cursor.com/login`.
- False: `github.com.evil.example`, `notgithub.com`, `cursor.com/dashboard`.

`isCursorDomain` tests:

- True: `cursor.com`, `.cursor.com`, `www.cursor.com`, `authenticator.cursor.sh`.
- False: `opencode.ai`, `grok.com`.

Identity/email: from `GET /api/auth/me` on every poll (below). When the email differs from `auth.accountEmail`, the poller calls `auth.saveAccountEmail(email)`, which may fire `accountReset` (Core identity rules).

### 3.2 Endpoints

All three endpoints use base URL `https://cursor.com` via `ProviderHTTP` and the same HTTP mapping as Grokbot, except that **a 403 is `.unauthorized`** (no forbidden override). Shared headers are `Cookie`, `Accept: application/json`, `User-Agent`, and `Referer: https://cursor.com`.

| # | Method | URL | Extra headers | Body |
|---|---|---|---|---|
| 1 | GET | `/api/usage-summary` | — | — |
| 2 | GET | `/api/auth/me` | — | — |
| 3 | POST | `/api/dashboard/get-filtered-usage-events` | `Origin: https://cursor.com`, `Content-Type: application/json` | `{"startDate":"<startMs>","endDate":"<endMs>","page":<int>,"pageSize":500}`. startDate and endDate are **strings** of epoch ms; page and pageSize are numbers. |

Every response body (summary, me, and each events page) goes through `rejectUnauthorizedBody`:

- `ProviderHTTP.jsonObject` (HTML login page gives `.unauthorized(cursor)`; other HTML gives `.badResponse("Unexpected HTML response")`; malformed gives `.badResponse("Malformed response body")`).
- Then, if `error` is a string containing `not_authenticated` or `unauthor`, `.unauthorized(cursor)`.

`fetchSnapshot(now)`:

1. Start `usage-summary` and `auth/me` concurrently. A failure of `me` (any error) is ignored and yields nil.
2. `snap = parseSummary(summary, fetchedAt: now)`. A summary failure throws.
3. If `parseEmail(me)` returns a value, set `snap.accountEmail`.
4. `windowStart = eventsWindowStart(snap.billingCycleStart, now)`.
5. `aggregates = eventAggregates(...)`. If it is nil, or `aggregates.windowStart != windowStart`, return `(snap, .empty(dayStart: startOfDay(now)), [:])`; `costStats` stays nil.
6. Otherwise set `snap.costStats` and `snap.dailyTokens = aggregates.tokensByDay`. Use `hourly = aggregates.hourly` if it is for today, else empty for today. Return `(snap, hourly, aggregates.estimatedWeightByDay)`.

`parseEmail(data)`: parse JSON (an object, else nil). Return `email` if it is a string containing `@`, else nil.

#### 3.2.1 `parseSummary`

- If the root is not a JSON object, throw `.badResponse(cursor, "Expected JSON object")`. Invalid JSON is already rejected earlier as `.badResponse("Malformed response body")`.
- `billingCycleStart`, `billingCycleEnd`: `parseFlexible`, only from **string** values (non-empty).
- `membershipType`: string or nil.
- Sub-objects:
  - `plan = individualUsage.plan`
  - `overall = individualUsage.overall`
  - `onDemand = individualUsage.onDemand`
  - `pooled = teamUsage.pooled`
- Numbers come from `JSON.number`:
  - `planUsedCents = plan.used ?? 0`, `planLimitCents = plan.limit ?? 0`
  - `overall.used/limit`, `pooled.used/limit`
- `autoPercent = clamp(plan.autoPercentUsed)?` and `apiPercent = clamp(plan.apiPercentUsed)?`. Cursor percent fields are already in % units: `0.36` means 0.36%.
- **Total %** precedence:
  1. `clamp(plan.totalPercentUsed)` if present
  2. if auto and api are both present: `clamp((auto+api)/2)`
  3. `api`
  4. `auto`
  5. `planLimitCents > 0`: `clamp(planUsedCents/planLimitCents*100)`
  6. overall used/limit (limit > 0)
  7. pooled used/limit (limit > 0)
  8. `0`
- `planUsedUSD`/`planLimitUSD`:
  - If `planLimitCents > 0 || planUsedCents > 0`, use the plan cents / 100.
  - Else, if overall has both used and limit, use those / 100.
  - Else, if pooled has both, use those / 100.
  - Else nil.
- `onDemandEnabled = onDemand.enabled as Bool ?? false`. `onDemandUsedUSD = onDemand.used/100?` and `onDemandLimitUSD = onDemand.limit/100?`. These are parsed but not rendered in the Cursor panel.
- `pools`: always `[total(totalPercent, resetsAt: cycleEnd)]`, then `auto` if `autoPercent` is present, then `api` if `apiPercent` is present.
- `usedPercent = totalPercent`, `resetsAt = billingCycleEnd`.
- `displayPlanName`:
  - nil or empty `membershipType` gives `Cursor`.
  - Otherwise trim it. If it starts with `cursor` (case-insensitive), the result is the first character uppercased and the rest lowercased (e.g. `cursor pro` → `Cursor pro`).
  - Otherwise `Cursor ` + first character uppercased + rest lowercased (`ultra` → `Cursor Ultra`, `PRO` → `Cursor Pro`).

Pool track labels:

| Pool | Track label |
|---|---|
| total | `Total` |
| auto | `Auto + Composer` |
| api | `API (Other Models)` |

Short labels are `Total` / `Auto` / `API`.

#### 3.2.2 Usage events: paging, cache, overlap

Constants:

| Name | Value |
|---|---|
| `eventsRefreshInterval` | 300 s (5 min) |
| `eventsOverlap` | 900 s (**15 min**) |
| `eventsFullRefreshInterval` | 3600 s (**1 h**) |
| page size | 500 |
| page cap | 40 pages (20,000 events). Hitting the cap logs a warning and stops. |
| events window cap | 31 days |

`eventsWindowStart(cycleStart, now)`:

- `cap = startOfDay(now) − 31 calendar days`.
- Return `max(cycleStart, cap)`, or `cap` when the start is unknown.

`fetchAllEvents(from, to)`:

- `startMs = Int64(from.unix*1000)` and `endMs = Int64(to.unix*1000)` (truncation).
- Loop `page = 1...40`:
  1. POST the page, then `rejectUnauthorizedBody`, then `parseUsageEventsPage`.
  2. Append the events and add the raw page count to `pagedCount`.
  3. Stop if `pageCount < 500`.
  4. Stop if `total > 0 && pagedCount >= total`. A total of 0 means unknown.
  5. Stop at page 40.
- Any error propagates.

`parseUsageEventsPage(data)`:

- If the root is not an object, throw `.badResponse(cursor, "Expected events JSON object")`.
- `total = safeInt(JSON.number(totalUsageEventsCount)) ?? 0`. Numeric strings are accepted (`"1234"` → 1234).
- `raw = usageEventsDisplay` as an array of objects, or `[]`.
- Return `(raw.compactMap(CursorUsageEvent.init), raw.count, total)`. `pageCount` includes skipped events.

`CursorUsageEvent.init(json)` returns nil when:

- `model` (string, default `""`) has the prefix `grok-bot` (Bot usage belongs to Grokbot), or
- there is no valid timestamp.

Otherwise its fields are:

| Field | Source |
|---|---|
| `date` | `eventTimestamp` |
| `chargedCents` | `chargedCents > 0`, else `tokenUsage.totalCents > 0`, else 0 |
| `requestsCosts` | `max(0, JSON.number(requestsCosts) ?? 0)` |
| `inputTokens` / `outputTokens` / `cacheReadTokens` / `cacheWriteTokens` | `safeInt64(JSON.number(tokenUsage.<key>) ?? 0)` |
| `totalTokens` | `safeInt64(input + output + cacheWrite + cacheRead)`, summed as Double before conversion |
| `model` | string |

`weight` is `requestsCosts` if > 0, else `totalTokens` if > 0, else `chargedCents` if > 0, else 1.

`eventTimestamp(event)`, reading `timestamp`:

- A string parsed as Double, a JSON double, or a JSON int.
- `date(fromEpochNumber v)`: v must be finite and > 0. If `v > 1e12`, it is milliseconds (`/1000`); otherwise seconds.
- `"0"` and `"-5"` give nil.

`safeInt64(d)` / `safeInt(d)`: non-finite gives 0; values ≥ max give max; values ≤ min give min; otherwise truncation.

Cache (`CursorEventCache`) is in-memory and holds a single entry. It is keyed by **the cookie header string**, so each session or account sees only its own data. `clear()` runs on sign-out and on account reset (via the poller's `clearSnapshot`).

`CursorEventAggregates` fields: `fetchedAt`, `windowStart`, `costStats`, `hourly`, `estimatedWeightByDay`, `tokensByDay`.

`isFresh(windowStart, now)` requires all of:

- `self.windowStart == windowStart`
- `fetchedAt` is on the same day as `now`
- `now - fetchedAt < 300`
- `now >= fetchedAt`

`CursorEventLedger` fields: `windowStart`, `events`, `fetchedThrough`, `fullFetchAt`.

`canExtend(windowStart, now)` requires all of:

- the same `windowStart`
- `now >= fetchedThrough`
- `now - fullFetchAt < 3600`

`eventAggregates`:

1. If the cached aggregates are fresh, return them. No network.
2. Otherwise compute `refreshedLedger`:
   - **Incremental**, if the cached ledger `canExtend`:
     - `from = max(windowStart, fetchedThrough − 900 s)`.
     - Page `[from, now]`.
     - `events = cached.events.filter { date < from } + fresh`. Overlap events are replaced, not duplicated.
     - `fetchedThrough = now`; keep `fullFetchAt`.
   - **Full**, otherwise: page `[windowStart, now]`, with `fetchedThrough = now` and `fullFetchAt = now`.
3. `aggregate(events, snap, windowStart, now)`. Store the aggregates and ledger, and return them.
4. If paging fails, log it and return the **cached aggregates**, which may be stale or nil. The caller then discards them if their `windowStart` differs.

`aggregate(...)`:

- `costStats = aggregateCostStats(events, cycleStart: snap.billingCycleStart ?? windowStart)`. Sum over events with `date >= cycleStart`:
  - `meteredCycleUSD = Σ chargedCents / 100`
  - `cycleTokens = Σ totalTokens`
  - `cycleInputTokens`, `cycleOutputTokens`
- `hourly` (`CursorDayHourlyUsage` for `startOfDay(now)`). Only events in `[dayStart, dayStart+1day)` count, bucketed by local hour 0–23:
  - `hourWeights[h] += event.weight`
  - `quotaHourWeights[h] += chargedCents / (planLimitUSD*100 / averageWeeksPerMonth) * 100`, with `averageWeeksPerMonth = 365.2425/12/7 ≈ 4.348`. Only for events with `chargedCents > 0`, and all zeros if `planLimitUSD` is nil or ≤ 0. Example: 400 cents with a $400 limit gives ≈4.348.
  - `hourTokenWeights[h] += totalTokens`
  - `isEmpty` = all quota weights ≤ 0 **and** all hour weights ≤ 0.
- `estimatedWeightByDay`:
  - `bounds = DailyBudget.subscriptionMonth(knownStart: cycleStart, resetsAt: cycleEnd, now)`.
  - For events with `totalTokens > 0`, skip those with `date < (bounds.start ?? cycleStart)` or `date >= (bounds.end ?? cycleEnd)`.
  - Add `totalTokens` to `byDay[startOfDay(date)]`.
- `tokensByDay = dailyTokens(events)`. This is the Core `DailyTokenUsageTally` keyed by `DayKey` (`yyyy-MM-dd`, local). Each event adds model, input, output, cacheRead, cacheWrite, and `costUSD = chargedCents/100`. It feeds the hover label's token lines.

### 3.3 Models

`CursorSnapshot` has these fields:

- `fetchedAt`
- `usedPercent`: the total
- `pools`
- `billingCycleStart?`
- `billingCycleEnd?`
- `membershipType?`
- `planUsedUSD?`
- `planLimitUSD?`
- `onDemandEnabled`
- `onDemandUsedUSD?`
- `onDemandLimitUSD?`
- `costStats?`
- `accountEmail?`
- `dailyTokens: [DayKey: DailyTokenUsage]`

Derived:

- `resetsAt = billingCycleEnd`
- `usagePool = .monthly`
- `CursorPoolUsage{kind, usedPercent, resetsAt}`, with `remainingPercent = clamp(100 - used)`
- `CursorCostStats{meteredCycleUSD, cycleTokens, cycleInputTokens, cycleOutputTokens}`

### 3.4 Poller (`CursorUsagePoller`)

- Gating and interval follow the same pattern with `.cursor`.
- `performRefresh`:
  1. If disabled or already refreshing, skip.
  2. If the cookie header is empty:
     - Set `auth.needsSignIn = true`.
     - If `snapshot == nil`, set `lastError = "Sign in to Cursor to load usage."`.
     - Skip.
  3. `ProviderRefresh.run`.
- Publish:
  1. If `snap.accountEmail` is non-nil and differs from `auth.accountEmail`, call `auth.saveAccountEmail(email)`. This may fire `accountReset`, which clears the previous account's figures before this publish.
  2. **Carry-over**: if `snap.costStats == nil` (events unavailable) and the previous snapshot has the **same** `billingCycleStart`:
     - Keep the previous `costStats` and `dailyTokens`.
     - `hourly = carriedHourly(previousHourly, fallback: hourly)`: the previous one if it is for the same day, else the new one.
     - `estimatedWeightByDay = budgetContext?.estimatedWeightByDay ?? new`.
  3. Set `snapshot` and `dayHourlyUsage`.
  4. Remember `billingCycleEnd` if it is non-nil. `cycleEnd = snap.billingCycleEnd ?? remembered`.
  5. Call `daily.record(windowUsedPercent: used, at: fetchedAt, window: QuotaWindow(start: cycleStart, resetsAt: cycleEnd), periodDays: cycleLengthDays)`. `cycleLengthDays` = `daysInBillingCycle(start, end)` when both are known and end > start, else **30**. Store key `cursor_daily_usage`.
  6. Set `budgetContext = {estimatedWeightByDay, used, cycleStart, cycleEnd, referenceNow: fetchedAt}`.
  7. `dailyBudgetDays = buildDailyBudgetDays(observedByDay: daily.spentByDay, interruptedWindowStart: daily.interruptedWindowStart, estimatedWeightByDay, used, cycleStart, cycleEnd, now: fetchedAt)?.attachingTokens(snap.dailyTokens)`.
  8. Set `lastError = nil` and `lastRefreshedAt`, `dataSourceLabel = "Cursor dashboard"`, `auth.needsSignIn = false`.
- `buildDailyBudgetDays(...)`:
  1. `bounds = DailyBudget.subscriptionMonth(knownStart: cycleStart, resetsAt: cycleEnd, now)`. If it is nil, return **nil**; a calendar month is never invented. When only one end is known, the other is inferred by one calendar month anchored to the known day. A stale cycle advances to the running one.
  2. `cycleStartDay = startOfDay(bounds.start)`. `observed` and `estimated` are filtered to `[cycleStartDay, bounds.end)`.
  3. Remove the **first (earliest) observed day**, because it was only partially tracked. Call the rest `effectiveObserved`.
  4. `tracked = Σ effectiveObserved`. `unobserved` = estimated days not in `effectiveObserved`.
  5. Fill the days:
     - If `tracked > used + 0.001` and `tracked > 0`, then `blended[d] = effectiveObserved[d] * used/tracked` (rescale down; no back-fill).
     - Otherwise:
       - `untracked = max(0, used - tracked)` and `weightSum = Σ unobserved`.
       - If `untracked > 0.001` and `weightSum > 0`, add `w/weightSum*untracked` to each unobserved day.
       - Else, if `untracked > 0.001` and `unobserved` is non-empty, add `untracked/count` to each.
  6. `prior`: if `interruptedWindowStart` is set, the observed days in `[startOfDay(interruptedWindowStart), cycleStartDay)`.
  7. Return `DailyBudget.buildMondayWeekDays(periodStart: bounds.start, periodEnd: bounds.end, limitUSD: 100, spentByDay: blended, priorWindowSpentByDay: prior, historyByDay: observedByDay, weekOffset, now)`. This gives Monday–Sunday bars of the week containing `now`, a daily share of `100/cycleDays`, prior-window days dimmed, and past weeks reading `historyByDay`. That function is Core.
- `dailyBudgetDays(weekOffset:)`: nil before the first refresh. Otherwise it rebuilds from `budgetContext` and the live store, then attaches `snapshot.dailyTokens`.
- `clearSnapshot` (sign-out and account reset) clears `snapshot`, `dayHourlyUsage`, `dailyBudgetDays`, `billingCycleEnd`, `budgetContext`, `lastError`, `dataSourceLabel`, `lastRefreshedAt`, and `eventCache.clear()`. `accountReset` also calls `daily.clear()`.
- Headline: `snapshot.usedPercent` (the total) and `snapshot.billingCycleEnd`. The Overview uses `dayHourlyUsage` for the hourly chart (another spec).

### 3.5 Panel (`CursorPanelView`)

1. **Signed out** (`auth.needsSignIn && snapshot == nil`):
   - Title row `Cursor`.
   - Text: `Sign in to cursor.com to load Total, Auto, and API usage.`
   - Button `Sign In to Cursor…`.
2. **Snapshot**:
   - Header row `Cursor`, with the trailing meta label `displayPlanName` (e.g. `Cursor Ultra`).
   - **Pool card**:
     - Header `Usage Pool Monthly`.
     - For each pool in order (Total, then Auto if present, then API if present), a `SlimUsageTrack`:
       - The label row shows the `trackLabel` on the left and `"<Int(round(clamp(used)))>% Used"` on the right.
       - An 8 px capsule in the Cursor accent.
       - The caption `Resets …` from `pool.resetsAt ?? snapshot.resetsAt`.
   - `NavigableDailyBudgetCard(style: .monthly, accent: cursor, periodUsedPercent: used, periodStart: billingCycleStart, resetsAt: billingCycleEnd, daysForWeek: dailyBudgetDays(weekOffset:) ?? [])`:
     - Hidden when the days are empty.
     - Title `Daily Budget`, allowance noun `monthly`.
     - Pace earns across the subscription month (`monthlyPacePeriodStart`).
     - The range label is `"MMMM d – d"` within one month, else `"MMMM d – MMM d"`.
     - The hover label lists tokens per day, from `dailyTokens`, with at most 3 models named (Core hover).
   - If `costStats` is set, a 2×2 `MetricStatGrid`:
     - `Monthly spend`: `Format.usd(meteredCycleUSD)`, e.g. `$62.02`
     - `Total tokens`: `Format.tokens(cycleTokens)`
     - `Input tokens`: `Format.tokens(cycleInputTokens)`
     - `Output tokens`: `Format.tokens(cycleOutputTokens)`
     - `Format.tokens` is compact: below 1000 the raw number; else K with 0 decimals; M with 1 decimal under 10M, else 0; B with 1 decimal. Rounding is half away from zero, and a value that rounds to 1000 of a unit promotes to the next unit (999,500 → `1.0M`).
   - If `lastError` is set, `StaleDataCaption`.
   - If `auth.needsSignIn`, the button `Sign In Again…`.
   - Sign-out button (`Sign out of Cursor?`).
3. **No snapshot**:
   - Text `Refreshing…` / `lastError ?? "No usage data yet."`.
   - A `Sign In to Cursor…` button if `needsSignIn`. This is unreachable, because branch 1 catches that case first.

---

## 4. Cross-provider notes for the Rust port

- Persisted files touched by this slice (`<data dir>/TokenMon/`). On Windows, use `%APPDATA%\TokenMon\` or the Core spec's chosen directory.

  | File | Contents |
  |---|---|
  | `grok_history.dat` | Grok daily history JSON |
  | `activity_grok_hourly_today.dat` | Grok hourly deltas (Core format) |
  | `activity_grokbot_hourly_today.dat`, `activity_grokbot_daily_usage.dat` | Grokbot hourly and daily deltas (Core format) |
  | `activity_cursor_daily_usage.dat` | Cursor daily deltas (Core format) |
  | Credentials | vault keys `auth_session` (Grok) and `cursor_auth_session` (Cursor/Grokbot), plus email `.dat` files (Core) |
- The Cursor event cache is in-memory only. It is never written to disk.
- Authenticated JSON is never written to disk, and response bodies are never shown in the UI.
- Swift edge cases to **fix rather than copy** (record each as a deliberate divergence):
  1. `scanForField7Products` and `parseProductSubMessage` read varints past the sub-message `end`. The length check then computes `UInt64(end - index)`, which traps in Swift when `index > end`. In Rust, bound every read to `end` and use checked arithmetic; on violation, stop the walk (field-7 scan) or reject the product (sub-message).
  2. `scanForField7Products` has no recursion-depth limit. Cap it (e.g. at 64) or use an explicit stack, to avoid stack overflow on adversarial nesting.
  3. The tie order of unknown products is nondeterministic. Sort by (display index, id).
  4. In the REST map form, booleans coerce to 1/0. Ignoring booleans is acceptable.

---

## 5. Test parity checklist

All dates in these tests are UTC Gregorian unless noted. Tests that call `Calendar.current` must run under a deterministic local time zone in Rust, or be rewritten with an injected calendar.

### 5.1 `TokenMonTests/Providers/Grok/GrokUsageParsingTests.swift`

| Test | Asserts |
|---|---|
| `testParseFixtureJSON` | REST JSON `{usedPercent:35, remainingPercent:65, resetsAt:"2026-07-16T20:25:00Z", products:[build 25, api 9, chat 1]}` gives used 35, remaining 65, 3 products, `products[0].id=="build"` with colour `build`. Order is preserved; the REST array is **not** re-sorted. |
| `testRemainingDefaultsFromUsed` | `WeeklyUsageSnapshot(used:40)` has remaining 60. |
| `testUsageValuesAreClampedAtModelBoundary` | used 140 → 100; remaining −20 → 0; product 140 → 100. |
| `testGrokDomainMatchingDoesNotAcceptLookalikes` | `.grok.com` ✓, `accounts.x.ai` ✓, `evilgrok.com` ✗, `x.ai.evil.example` ✗. |
| `testProductColorMapping` | `build`→build, `API`→api (case-insensitive), `imagine`→imagine. |
| `testExportCSVContainsHeader` | CSV of the `.preview` snapshot contains `fetchedAt,usedPercent` and `build:25`. |
| `testExportCSVNeutralizesFormulaInjection` | email `=cmd|'/C calc'!A1` appears as `'=cmd|`, and never as `,=cmd|`. |
| `testCSVEscapeGuardsLeadingTabAndCarriageReturn` | `"\t=1+1"`→`'\t=1+1`; `"\r=1+1"`→`"'\r=1+1"` (quoted); `"\r\n=1+1"`→`"'\r\n=1+1"`; `-5`→`'-5`; `plain`→`plain`. |
| `testCSVEscapeQuotesCarriageReturnAndLineBreaks` | `a\rb`, `a\r\nb`, `a\nb`, `a,b` are each wrapped in quotes; `say "hi"`→`"say ""hi"""`. |
| `testExportJSONRoundTrip` | JSON export parses as an array. |
| `testByProductMapParse` | `{usedPercent:35, byProduct:{build:25,api:9,chat:1}}` gives 3 products. |
| `testGRPCProductBreakdown` | Hex fixture A (§5.8) gives used 36, 2 products: chat 23, build 13. The enum-6 entry with no percent is dropped. |
| `testOversizedLengthVarintDoesNotCrash` | Bytes `3A FF×9 7F` throw (no crash). |
| `testGRPCLiveCreditsConfigEnumMap` | Hex fixture B gives used 64; build 30, chat 21, imagine 12, other 1; no voice; 4 products; order `[chat, build, imagine, other]`. |
| `testGRPCProductBreakdownWithVoiceGap` | Built frame: used 64, products (2:30),(4:21),(6:nil),(5:12),(3:1) give 4 products with correct pairing and no voice. |
| `testGRPCProductBreakdownWithExplicitZeroVoicePercent` | Same, with (6:0.0): imagine 12, other 1, no voice, count 4. |
| `testDailyUsageBuilderDeltas` | UTC, firstWeekday Monday. now 2026-07-15T12Z, reset 07-16T18:57Z. Yesterday 12:00 used 10 (build 7, chat 3); today 10:00 used 30 (build 20, chat 10). Result: 7 days, hasDailyData, not estimated; yesterday 0; today 20 with 2 segments. |
| `testDailyUsageOnlyShowsProductsThatGrew` | now 07-12T12Z, reset 07-13T09Z. Samples without resetsAt: 39 (chat 23, build 13, api 3) then 42 (chat 26, …). Today is 3, a single chat segment. |
| `testDailyUsageShowsYesterdayAfterDayRollover` | Flat 39 → 39 gives both days 0, not estimated, no daily data. |
| `testDailyUsageEmptyUntilSecondSampleDay` | A single sample gives no daily data, estimated, all segments empty. |
| `testDailyUsageExcludesFlatBuildFromToday` | 46 → 51 → 58 with build flat at 16: today is 7 with no build segment, and chat, api, and voice present. |
| `testDailyUsageMidPeriodRecalibrationRebasesOntoThatDay` | 51 → 71 → 28 with the same reset: today `isAfterReset`, earlier days 0, today ≈28 including build, estimated = true. |
| `testDailyUsageRealResetShowsPostResetUsage` | Yesterday 90 (old reset 07-16) and today 12 (new reset 07-23), window from the old reset, now 07-15T20Z: today `isAfterReset`, total 12, includes chat. |
| `testDailyUsageIgnoresPriorBillingPeriodSample` | A prior-period sample (reset 07-09) plus one in-week sample: all bars empty, estimated. |
| `testDailyCapFillFraction` | `fill(10) = 10/(100/7)`; `fill(21) = 1`; `fill(0) = 0`. |
| `testGRPCParseKeepsPastCanonicalReset` | Frame with reset varint 1_750_000_000 (in the past) gives `resetsAt` == that instant. |
| `testDailyUsagePreviewHasSevenDays` | Preview: 7 days, hasDailyData, start weekday Thursday (5), end Wednesday (4). |
| `SharedHelpersTests.testPercentClampBounds` | clamp 0, 100, −5→0, 140→100, 37.6. (Core) |
| `SharedHelpersTests.testDomainMatchesAcceptedHosts` | Domain match cases, including `www.grok.com` ✓, `grok.com.evil.example` ✗, `notopencode.ai` ✗, `x.com` ✓. (Core) |
| `SharedHelpersTests.testFileBackedStringStoreRoundTrip` | set, get, overwrite, remove. (Core) |
| `SharedHelpersTests.testUsdCurrencyFormatter` | 410 → `$410.00`; 4.1 → `$4.10`. (Core) |

### 5.2 `GrokUsageClientTests.swift` (stubbed HTTP by path; unknown paths give 404)

| Test | Asserts |
|---|---|
| `testCancelledFetchThrowsCancellationError` | Cancelling a never-responding request surfaces cancellation. |
| `testBillingSuccessSkipsRESTProbes` | Billing 200 with fixture A gives used 36; only the billing path is requested. |
| `testBillingUnauthorizedRejectsWithoutProbing` | Billing 401 gives `.unauthorized`; only billing is requested. |
| `testTrailersOnlyUnauthenticatedIsUnauthorized` | 200, headers `grpc-status: 16`, `grpc-message: unauthenticated`, empty body: `.unauthorized`. |
| `testBillingChallengeIsTransient` | 403 `text/html` "Just a moment..." is not `.unauthorized`. |
| `testRESTProbeRejectionNeverInvalidates` | Billing 500 and all REST 401: the error is not unauthorized; requested paths are billing followed by all 4 REST paths in order. |
| `testRESTProbeSuppliesUsageWhenBillingFails` | Billing 503 and REST #3 (`/rest/billing/usage`) returning `{"usage":{"usedPercent":42}}` give used 42. |
| `testUnrelatedPercentKeyIsNotUsage` | `{"data":{"percent":50,"name":"profile completion"}}` gives nil. |
| `testOutOfRangeUsageIsRejected` | usedPercent 1500 → nil; −3 → nil. |
| `testProductsWithoutUsageKeysAreIgnored` | Products with only `value`/`percent` keys give nil. |

### 5.3 `ProductCatalogTests.swift`

| Test | Asserts |
|---|---|
| `testFilteredKeepsVisibleAboveThreshold` | threshold 5 over chat 50, build 30, api 2 gives `[chat, build]`. |
| `testFilteredExcludesHiddenProducts` | visible `{chat}` gives `[chat]`. |
| `testFilteredOrdersByDisplayOrder` | Input other, chat, build gives `[chat, build, other]`. |
| `testFilteredEmptyWhenNoneAboveThreshold` | Empty. |
| `testPanelProductsKeepsSynthesizedOtherSlice` | `synthesizeProducts(64)` gives `[other]` at 64. |
| `testPanelProductsDropsZeroValueProducts` | chat 0, build 40 gives `[build]`. |
| `testPanelProductsRespectsVisibility` | `[chat]`. |
| `testCategoryProductsAlwaysIncludesTheFourCategories` | chat 40, api 8, other 7 gives ids `[chat, build, imagine, voice]` with values `[40, 0, 0, 0]`. |
| `testCategoryProductsRespectsVisibility` | visible `{chat, voice}` gives `[chat, voice]`. |

### 5.4 `GrokPollerGatingTests.swift`

| Test | Asserts |
|---|---|
| `testDisabledGrokNeverFetchesEvenWhenSignedIn` | Only Cursor enabled: `refreshNow` performs 0 fetches, and `delayUntilDue` is nil (parked). |
| `testLoopParksWhileGrokIsNotNeeded` | Selected tab Cursor, no menu-bar graph, threshold off: parked. Turning the Grok menu-bar graph on gives `delayUntilDue == 0`. |
| `testCancellationIsNotReportedAsAFailure` | A fetch that throws cancellation leaves `lastError` nil and `needsSignIn` false. |
| `testFailureAfterASnapshotStillReportsTheError` | The snapshot is kept, and `lastError == "Grok network error: offline"`. |
| `testManualRefreshFetchesWhenEnabledButNotShown` | 1 fetch, snapshot set. |
| `testPollerBuildsTheCurrentWeek` | `currentWeek` is nil before a refresh. After a refresh with reset = now+3 days, the week is non-nil, `week(0) == currentWeek`, and `week(-1)` is non-nil. After `signOut`, `currentWeek` is nil. |

### 5.5 `GrokHistoryStoreTests.swift`

| Test | Asserts |
|---|---|
| `testAppendInsertsAtFrontOfRecent` | Yesterday 20 then today 10: `recent.count` 2 with the newest first (10). |
| `testSecondSameDaySampleReplacesInsteadOfPrepending` | 10, then 42, then 55 on the same day: 1 entry, 55, disk 1 row. |
| `testTinyChangeWithinSixtySecondsIsSkipped` | +0.01 at +30 s is ignored; the value stays 50. |
| `testSignificantChangeWithinSixtySecondsIsKept` | +5 at +30 s replaces (55); count 1. |
| `testDifferentDaysProduceDistinctEntries` | 2 entries. |
| `testRecentCapEvictsOldest` | 250 days appended: `recent ≤ 200`; disk keeps 250. |
| `testClearEmptiesRecentAndDisk` | Both empty. |
| `testRowsAreKeyedByAccount` | Same day, accounts a (40) and b (5): 2 rows; active b; recent `[5]`. Switching to a gives `[40]`. |
| `testUnchangedUsageLaterTheSameDayLeavesTheRowAlone` | An identical later poll keeps the original `fetchedAt`; 31 then replaces it. |
| `testHistoryIsSavedAndReloaded` | After flush, a reload has `[42]`, recent 1, and `storeFailed` false. |
| `testUnreadableHistoryIsKept` | The file holds `not json`: `storeFailed`. After append and flush the file is still `not json`, and recent is 1 (in memory). |
| `testSwiftDataHistoryIsImportedOnce` | macOS-only legacy import. **N/A on Windows**; drop or replace with a no-op test. |

### 5.6 `BillingPeriodWindowTests.swift`

| Test | Asserts |
|---|---|
| `testBillingPeriodWeekStartsOnPeriodStartDay` | Reset Thu 07-16T18:57Z. Expected bounds by `now`: <br>• 07-15T12Z: Thu 9 – Wed 15 (span 6). <br>• 07-16T12Z (reset morning): 9 – 16 (span 7). <br>• offset −1 from the reset morning: 2 – 8. <br>• 07-16T20Z (after the reset): 16 – 22, with exactly one Thursday. <br>• Next reset 07-23 at 07-16T20Z: 16 – 22. <br>• offset −1 at 07-15: 2 – 8. |
| `testStaleResetRollsForwardToContainToday` | Reset 07-16, now 07-27: 23 – 29, contains today. |
| `testWeeklyChartRefusedWithoutResetsAt` | No reset gives a nil week. |
| `testPeriodStartThursdayShowsUsage` | now 07-16T20Z, reset 07-23, single sample 12 (chat 8, build 4): week starts on Thursday, and Thursday's bar is 12 with segments. |
| `testCrossResetDeltasDoNotInventDropBar` | Wed 90 (old reset) and Fri 8 (new reset), window from the new reset: Wed is outside the window; Fri is 0 (no drop bar, no segment > 50); estimated. |
| `testPreviousWeekKeepsPriorPeriodDailyUsageAfterRollover` | Mon 40, Tue 55, Wed 70 (old), Fri 8 (new); offset −1: window 9 – 15, Tue 15, Wed 15, hasDailyData. |
| `testResetDayUsageBeforeResetInstantIsCharted` | Wed 60 and Thu morning 72, now Thu 12Z before the 18:57 reset: 8 days, the last is today with 12. |
| `testEarlyResetKeepsOldWindowHistoryOnThePreviousPage` | Old reset 10-07T18:05, new reset 10-09T18:05. Samples 09-30 10, 10-01 30 (old), 10-02T20 3, 10-03 8 (new); now 10-03T15. Current week starts Oct 2, and Oct 2's bar is 3. Previous page: Oct 1 is 20. |

### 5.7 `DailyBudgetTests.swift` and `DailyBudgetWeekOffsetTests.swift` (Core `DailyBudget`; port them with the Core module)

These live in the Grok test folder but exercise Core. Listed for completeness:

**`DailyBudgetTests`:**

- budgetPerDay: 100/7, and zero guards.
- daysInBillingCycle: 7, and a minimum of 1.
- buildDays: covers the period at 10/day; over-budget flag; percentOfBudget clamps at 100, and is 0 for a zero budget.
- buildWeeklyWindowDays:
  - mid-period anchoring (Aug 20–26 for a reset of Thu Aug 27 11:00);
  - before the reset instant;
  - rolls at the reset instant (Aug 27 – Sep 2, stale spend zeroed);
  - a stale reset advances whole weeks (to today … Aug 31).
- paceHeadroom:
  - banked prior days 9;
  - partial 4;
  - over pace −3;
  - excludes future days (30);
  - nil when the budget is zero or there are no days;
  - elapsedDays across the full period (`daily*25`);
  - credits today on the final day;
  - the period start credits today;
  - caps elapsed at the period length (31).
- elapsedDaysThroughToday is inclusive (25, 1).
- monthlyPacePeriodStart:
  - prefers the known start;
  - derives from resetsAt (Aug 16);
  - advances a stale cycle;
  - nil without signals.
- weeklyPacePeriodStart uses the first bar.
- subscriptionMonth:
  - from resetsAt alone;
  - nil without signals;
  - advances a stale cycle;
  - keeps the anchor day after a short month (Jan 31 → Feb 28/Mar 31; reset Jan 31 → Mar 31–Apr 30).
- buildSubscriptionMonthLast7Days:
  - requires a cycle;
  - Monday-start week with budget 100/31;
  - includes today on the reset morning;
  - a stale end does not paint the closed suffix.
- buildDays keeps the daily share on the final morning (31 vs 32 bars).
- paceCaption strings:
  - `20.0% usage left today from unused prior days`
  - `No usage yet · 10.0% today`
  - nil when the budget is 0
  - `Usage 5.0% over today's allowance`
  - `25.0% usage left today from unused prior days`
  - `4.0% usage left today`
  - `On pace for today`
- Over-pace caption omits the used amount (`Usage 7.1% over today's allowance`).
- First-day remaining is a whole day's share (`7.3% usage left today`).
- The final window day accounts for the time until reset (over).
- Without a reset instant: `9.0% usage left today`.
- Monthly mid-cycle shows no false over-pace; burned early flags over.
- Early reset:
  - weekly window leads with prior days (9 bars, prior `[12,18]`, window Oct 2–8);
  - pace ignores prior-window days;
  - no window start leaves the window unchanged;
  - a windowStart never moves bars before the reset-implied start;
  - the Monday week shows prior-window days `[f,f,t,t,f,f,f]`.

**`DailyBudgetWeekOffsetTests`:**

- WeekOffset: `previous(0) = -1`, `next(-4) = -3`, `next(0) = 0`, `canGoNext`.
- Weekly offset −1 shows the previous period (Aug 13–19).
- Weekly offset skips the early-reset lead-in (Sep 25 – Oct 1, `[…,12,18]`).
- Monday week offset −1 inside the period (Aug 10 start, spend 4).
- Monday offset 0 ignores history before the period.
- Monday offset −1 uses history before the period (Sep 21 start, 8).

### 5.8 gRPC hex fixtures (copy verbatim into Rust tests)

**Fixture A** (`testGRPCProductBreakdown` and the client tests):

```
000000005f0a5d0d0000104212001a00220b08b1debfd20610b8efb07f2a0b08b1d3e4d20610b8efb07f
3a070804150000b8413a07080215000050413a020806421c0802120b08b1debfd20610b8efb07f1a0b08b1d3e4d20610b8efb07f
580162006801800000000f677270632d7374617475733a300d0a
```

Decoded:

- Data frame, length 0x5f.
- `[1,1]` = 36.0f.
- `[1,4]` = {1: 1783623473 (2026-07-09T18:57:53Z), 2: nanos 267139000}.
- `[1,5]` = {1784228273 (2026-07-16T18:57:53Z), nanos}.
- Field 7 entries: (4, 23.0f), (2, 13.0f), (6 with no percent).
- `[1,8]` = {1: 2, 2: ts, 3: ts}.
- `[1,11]=1`, `[1,12]` empty, `[1,13]=1`.
- Trailer frame `80 0000000f "grpc-status:0\r\n"`.

**Fixture B** (`testGRPCLiveCreditsConfigEnumMap`):

```
000000006d0a6b0d0000804212001a00220b08b1c889d30610b8efb07f2a0b08b1bdaed30610b8efb07f
3a070802150000f0413a070804150000a8413a07080515000040413a070803150000803f
421c0802120b08b1c889d30610b8efb07f1a0b08b1bdaed30610b8efb07f580162006801
800000000f677270632d7374617475733a300d0a
```

Decoded:

- `[1,1]` = 64.0f.
- `[1,4]` = 1784833073 (2026-07-23T18:57:53Z).
- `[1,5,1]` = 1785437873 (2026-07-30T18:57:53Z).
- Products: (2, 30), (4, 21), (5, 12), (3, 1).

**Builder for synthetic frames** (from the test helper):

```
inner   = 0x0d + f32le(used)
        + Σ [0x3a, len, 0x08, varint(enum), (0x15 + f32le(pct))?]
        + 0x2a, len, 0x08, varint(resetAt=2_000_000_000)
payload = 0x0a, len(inner), inner
frame   = 0x00, u32be(len(payload)), payload
```

### 5.9 `Grokbot/GrokbotUsageClientTests.swift`

`fetchedAt` = 1_787_000_000. Account email is `user@example.com`.

| Test | Payload → assert |
|---|---|
| `testParsesCursorFundedWeeklyAllowance` | `{currentPeriodStart:"2026-08-20T11:00:00Z", nextResetTimestampUtc:"2026-08-27T11:00:00Z", usagePercent:42.5, includedLimitZero:false, hasNonZeroIncludedLimit:true, hasAvailableUsage:true}` gives used 42.5, remaining 57.5, `.cursor`, allowance true, reset parsed, email passed through. |
| `testParsesSuperGrokFundedAllowance` | `includedUsageSuperGrokPlan:"super_grok_heavy"`, `grokPlanLabel:"SuperGrok Heavy"` gives `.superGrok("SuperGrok Heavy")` and caption `via SuperGrok Heavy`. |
| `testSuperGrokPlanWithoutLabelUsesPlanIdentifier` | `.superGrok("super_grok")`. |
| `testMissingResetYieldsNilRatherThanSubstitutedDate` | Only usagePercent and currentPeriodStart: `resetsAt` nil. |
| `testAcceptsSnakeCaseProtoFieldNames` | snake_case fields, `included_limit_zero:true`: used 77, reset set, allowance false. |
| `testParsesStructuredTimestamps` | `{seconds:1787000000, nanos:500000000}` gives 1_787_000_000.5. |
| `testHTMLBodyIsTreatedAsUnauthorized` | `<!doctype html>…Sign in…` gives `.unauthorized(grokbot)`. |
| `testNotAuthenticatedErrorPayloadIsUnauthorized` | `{"error":"not_authenticated"}` gives `.unauthorized(grokbot)`. |
| `testMissingUsagePercentReportsNoBotAccess` | `{"hasAvailableUsage":false}` gives `.custom` (no access). |
| `testMissingUsagePercentWithAllowanceFieldsIsZero` | Period fields and hasNonZeroIncludedLimit give 0%, allowance true, reset set. |
| `testMissingUsagePercentSnakeCaseAllowanceFieldsIsZero` | Only `next_reset_timestamp_utc` gives 0%. |
| `testPercentIsClampedToPoolBounds` | 140 → 100; −8 → 0. |

### 5.10 `Grokbot/GrokbotUsagePollerTests.swift`

| Test | Asserts |
|---|---|
| `testBarsAnchorToTheProviderResetInstant` | Reset Aug 27 11:00, now Aug 25 15:00: 7 bars Aug 20–26, budget 100/7. |
| `testNoResetYieldsNoBars` | Empty. |
| `testResetMorningKeepsPeriodStartAsFirstBar` | now Aug 27 09:00: first bar is Thursday Aug 20, last Aug 26. |
| `testPeriodLengthIsDerivedFromTheSnapshotSpan` | A 14-day span gives `daysInPeriod` 7, 7 bars, budget 100/7. |
| `testFiveDaySpanStillYieldsSevenBars` | 5-day span gives 7. |
| `testDaysInPeriodUsesCalendarDaysNotTruncatedHours` | Aug 20 12:00 → Aug 27 11:00 gives 7. |
| `testSameDayStartAndResetFallsBackToAWeek` | Gives 7, 7 bars. |
| `testDaysInPeriodFallsBackToSevenWithoutPeriodStart` | Gives 7. |
| `testReportedSpanIsNotSanitized` | `reportedSpanDays` 14, `daysInPeriod` 7; nil without dates. |
| `testEntitlementCaptions` | `via Cursor`, `via SuperGrok Heavy`, `via SuperGrok`. |
| `testGrokbotIsARegisteredUsageProvider` | Present in `usageProviders`; displayName `Grokbot`; polled by `grokbot` and `overview`, not by `cursor`. |
| `testIsNewWindowDetectsRollover` | `QuotaWindowTransition.isRollover`: +7 days → true; +1 day → false; nil previous → false; backwards → false. (Core) |
| `testEarlyResetKeepsHistoryAndAnchorsBarsToNewPeriodStart` | Real `DailyQuotaDeltaStore`. Polls 10, 30, 50 (start −3d, reset +4d), then 3 (start 0, reset +7): 7 window bars from today (first 3); 3 prior bars (−3d first; 20, 20). Pace start is today; earned 100/7; headroom 100/7 − 3. |
| `testNewPeriodStartWinsOverUnchangedResetInstant` | `interruptedWindowStart` = the old start day; the window opens today; 7 bars. |
| `testNormalRolloverShowsNoPriorWindowBars` | 7 bars, no prior bars, first 3; the store keeps yesterday at 40. |
| `testSamePeriodDropIsNotTreatedAsEarlyReset` | 60 → 20 in the same period: no interrupted start, no prior bars. |

### 5.11 `Cursor/CursorUsageClientTests.swift`

| Test | Asserts |
|---|---|
| `testParseSummaryUsesTotalAutoAPIPercents` | Summary with `billingCycleStart:"2026-07-13T00:00:00.000Z"`, `billingCycleEnd:"2026-08-14T12:00:00.000Z"`, `membershipType:"ultra"`, plan {used 7600, limit 40000, total 19, auto 16, api 30}, onDemand {enabled, used 2309, limit null} gives used 19; 3 pools (total remaining 81, auto 84, api 70); plan $76/$400; `Cursor Ultra`; dates parsed; `resetsAt == end`. |
| `testParseSummaryLeavesCycleDatesNilWhenOmitted` | Dates nil. |
| `testParseSummaryAveragesAutoAndAPIWhenTotalMissing` | auto 10, api 30 gives 20; 3 pools. |
| `testParseSummaryFallsBackToUsedOverLimitCents` | used 2500, limit 10000 gives 25%, $25/$100, 1 pool. |
| `testAggregateCostStats` | Events (1202¢, 1M in + 0.5M out, now) and (5000¢, 2M in, −2 days), cycle −20 days: $62.02, tokens 3.5M, in 3M, out 0.5M. With a cycle start of −1 day: $12.02. |
| `testHourWeightsBucketByRequestsCosts` | requestsCosts 2.5 + 1.5 at 10h gives 4; tokens 150 at 14h gives 150; hour 9 gives 0. |
| `testQuotaHourWeightsUseChargedCentsAndPlanLimit` | 100 + 300 cents, limit $400: `weights[10] = averageWeeksPerMonth`. |
| `testTokenHourWeightsAggregateEventTokens` | 100+50+25+5 = 180. |
| `testParseUsageEventsPage` | total 2, pageCount 2, 2 events. |
| `testParseUsageEventsPageCoercesTotal` | `"1234"` → 1234; missing → 0. |
| `testSafeIntegerConversionDoesNotTrap` | 1e30 → Int64.max; −1e30 → Int64.min; NaN and ∞ → 0; `safeInt(1e30)` = Int.max. |
| `testTokenCountHandlesHugeValues` | Input 1e30 gives Int64.max. |
| `testEventTimestampStringUnits` | `"1775418973898"` → ms; `"1775418973"` → s; `"0"`, `"-5"` → nil. |
| `testCursorDomainFilter` | See §3.1. |
| `testDailyEstimateWeightByDayExcludesBotAndClipsToCycle` | Cycle Aug 22 17:00 – Aug 23 17:00. Events: 22@10 (5000, before the cycle), 22@20 (300), 22@21 `grok-bot-default`, 23@12 (1220) give `{22: 300, 23: 1220}`, sum 1520. |
| `testRejectUnauthorizedBodyTreatsHTMLAsUnauthorized` | An HTML "Sign in to Cursor" page gives `.unauthorized(cursor)`. |
| `testRejectUnauthorizedBodyMapsErrorString` | `not_authenticated` and `unauthorized` both give `.unauthorized`. |
| `testRejectUnauthorizedBodyTreatsMalformedAsBadResponse` | `{"individualUsage":` gives `.badResponse`. |
| `testRejectUnauthorizedBodyPassesNormalSummary` | No throw. |
| `testGrokBotEventsAreExcludedFromAggregations` | default (100¢, 1000 tok) and grok-bot (900¢, 9000 tok) give cost $1, tokens 1000, activity 1000, tokens/hour 1000, quota < 2. |
| `testParseUsageEventsPageCountsSkippedEvents` | 3 raw events (one bot, one without a timestamp) give pageCount 3, 1 event. |

### 5.12 `Cursor/CursorEventCacheTests.swift`

| Test | Asserts |
|---|---|
| `testEventsWindowStartIsCycleStartWithinCap` | A cycle start 10 days back is returned as-is. |
| `testEventsWindowStartCapsAtThirtyOneDays` | A cycle start 90 days back, or nil, gives `startOfDay(now) − 31 days`. |
| `testEventsArePagedOnceWithinRefreshInterval` | Two fetches 60 s apart: 1 events page, 2 summaries, same costStats ($2.50). At +301 s, 2 pages. |
| `testRefreshPagesOnlyEventsSinceTheLastFetch` | The second fetch's startMs equals `(now − 900 s)` in ms; the cached event (1 h before) is kept, $2.50. |
| `testWholeWindowIsPagedAgainAfterTheFullRefreshInterval` | At +3601 s both start values are equal (full window); $2.50. |
| `testEventsFailureReusesCachedAggregates` | A later events failure returns the cached $4 and a non-empty hourly. |
| `testEventsFailureWithoutCacheLeavesCostStatsNil` | costStats nil; hourly and weights empty. |
| `testCacheIsScopedToSession` | Key `a` hits; `b` misses; `clear` empties. |
| `testPollerKeepsPreviousEventFiguresWhenEventsAreUnavailable` | The second poll (costStats nil, same nil cycle start) keeps the stats ($7/70) and the previous hourly. |
| `testAuthPageMatchesIdentityHostsExactlyOrBySuffix` | See §3.1. |

Fixtures:

- Summary: start = now − 5 days, end = now + 25 days, plan {used 100, limit 2000, totalPercentUsed 5}.
- Events page: one event 1 h before midday today, `{"model":"default","chargedCents":N,"tokenUsage":{"inputTokens":10,"outputTokens":5}}`, `totalUsageEventsCount` 1. It returns an empty page (`total 0`) when the requested startMs is after the event.

### 5.13 `Cursor/CursorUsagePollerTests.swift`

| Test | Asserts |
|---|---|
| `testTrackedDayUsesMeasuredDeltaAndOthersBackfilled` | Observed {23: 9, 25: 3}; estimate 100 each on 22, 23, 24, 26, 27; used 9; cycle Aug 22 17:00 → Sep 22 17:00; now Aug 28. Result: 7 bars from Mon Aug 24, budget 100/31; Aug 25 = 3; Aug 26 = 1.2. |
| `testFirstTrackedDayUsesEstimate` | Observed {25: 1}, estimates 50 + 50, used 4: Aug 25 = 2. |
| `testCompletedDayUsesPoolPercentDelta` | Store records 66 → 68 yesterday and 68 → 71 today: today = 3. |
| `testBuildDailyBudgetDaysNilWithoutCycleDates` | nil. |
| `testTrackedExceedingPoolPercentIsRescaled` | Observed {23:1, 25:6, 26:4}, used 5: Aug 25 = 3, Aug 26 = 2, chart sum 5. |
| `testEarlyResetKeepsOldCycleDaysAsHistoryAndRestartsPoolMath` | Interrupted start Sep 10, new cycle Oct 2 17:00. Week of Mon Sep 28: Sep 29 prior 2, Oct 1 prior 5, Oct 3 live 3; budget 100/31; live sum ≤ 4. |
| `testWithoutEarlyResetOldDaysStayHidden` | No prior flags; Oct 1 = 0. |
| `testBuildDailyBudgetDaysWeekOffsetShowsPreviousMonday` | offset −1: starts Aug 17; Aug 18 = 4, not prior. |

---

## 6. Fuzz-testing targets (untrusted input parsers)

| # | Function (Swift) | Input | Invariants for a Rust `cargo-fuzz` harness |
|---|---|---|---|
| 1 | `GRPCWebParser.parseUsage(data, now)` | Arbitrary bytes (server body) | No panic, no stack overflow, bounded time. Result is Ok or Err. On Ok: `0 <= used <= 100`; each product id is unique, with percent in (0.05, 100]; products sorted by display order. |
| 2 | `GRPCWebParser` internals (`dataFrames`, `scanProtobuf`, `scanForField7Products`, `parseProductSubMessage`, `readVarint`) | bytes | No out-of-bounds read or overflow (the §4 divergences: bound varints to `end`, cap recursion). The path length is ≤ 5 for `scanProtobuf`. |
| 3 | `GRPCWebParser.validateTrailers(data)` / `trailerFields` | bytes | No panic on invalid UTF-8, bad percent escapes, or huge lengths. Unauthorized only for status 16 or `unauthenticated`. |
| 4 | `GRPCWebParser.validateHeaders` | Header strings (`grpc-status`, `grpc-message`) | Integer parse never panics (overflow, signs, whitespace). |
| 5 | `GrokUsageResponseParser.parseJSON` | JSON bytes | No panic or stack overflow on deep wrapper nesting. On Some: used and remaining in 0...100; products clamped. |
| 6 | `GrokbotUsageClient.parseUsageStatus` | Body bytes (JSON or HTML) | Only `.unauthorized`, `.badResponse`, or `.custom` errors. On Ok, used in 0...100. Timestamp math with huge `seconds`/`nanos` (non-finite or overflowing) must not panic. |
| 7 | `CursorUsageClient.parseSummary` | JSON | Percents clamped; money finite; no panic on wrong types (strings for numbers, nested `val` wrappers). |
| 8 | `CursorUsageClient.parseUsageEventsPage` + `CursorUsageEvent.init` + `eventTimestamp` + `date(fromEpochNumber)` | JSON | `pageCount >= events.count`. `safeInt`/`safeInt64` saturate. Timestamps are > 0 and finite. Token sums never overflow (summed in f64, then saturated). |
| 9 | `CursorUsageClient.rejectUnauthorizedBody` / `ProviderHTTP.jsonObject` / `looksLikeLoginPage` | bytes | Total over arbitrary bytes, including non-UTF-8 prefixes (Latin-1 decode of the first 64 KiB). |
| 10 | `CursorUsageClient.parseEmail` | JSON | Only returns strings containing `@`. |
| 11 | `GrokHistoryStore` payload decode (`grok_history.dat`) | Local file (semi-trusted, may be corrupt) | Decode failure gives `storeFailed`; the file is never overwritten. |
| 12 | `GrokExportService.csvEscape` | Arbitrary strings (account email, ids) | Property: the output never starts with `= + - @ \t \r` unless prefixed by `'` (or inside quotes after `"'`). The output is a single CSV field per RFC 4180. |
| 13 | `GrokDailyUsageBuilder.billingPeriodWeekBounds` / `week` | Arbitrary reset, now, and snapshot sequences (dates from untrusted payloads) | Terminates (520-iteration cap). The window has 7 or 8 days. No panic on extreme dates. |
| 14 | `CursorUsagePoller.buildDailyBudgetDays` | Arbitrary observed and estimate maps with extreme values | Bar values finite and non-negative. When rescaled, the sum ≈ usedPercent. |
| 15 | Cookie-header pruning (`ProviderAuthSession.pruneCookieHeader`, Core) for `sso`/`sso-rw`/`WorkosCursorSessionToken` | Header strings | Output contains only allowlisted names, or the input unchanged when none are present. |
