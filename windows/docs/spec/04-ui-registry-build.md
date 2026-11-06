# 04: UI shell, provider registry, sign-in UI, build/CI/release, assets

Source of truth: the Swift app at commit `3ac5c6b` (branch `windows-port`, MARKETING_VERSION `1.10.1`).
Scope: `TokenMon/UI/**`, `TokenMon/Providers/Registry/**`, the Overview panel views
(`TokenMon/Features/Overview/OverviewPanelView.swift`, `OverviewHourlyUsageChart.swift`), the app
shell that hosts them (`TokenMon/App/TokenMonApp.swift`, `AppDelegate.swift`, the parts of `AppModel.swift`
that the UI calls), their tests, and the build/release tooling. Per-provider panels (`GrokPanelView`,
`ClaudePanelView`, ...), pollers, auth sessions, `DailyBudget` maths, `Format`, settings storage and the
updater internals belong to other spec slices and are only referenced here.

Units: macOS "points" (pt) = Windows DIPs (device-independent pixels at 96 DPI = 1.0 scale). Every size
below is in pt/DIP unless it says px. Colors are sRGB; hex values are `round(component * 255)` and the
float components are the canonical values (store floats, not hex, for exact parity).

---

## 1. MonitorProvider registry

### 1.1 Identifiers, names, order (`MonitorProvider.swift`)

`enum MonitorProvider: String` (raw value = persisted id, also used in settings keys and the
sign-in window id). Declaration order (`allCases`) is:
`overview, grok, cursor, opencode, claude, chatgpt, openrouter, grokbot`.

| allCases idx | id (raw) | displayName | switcherLabel | Default usage order (`usageProviders`) | Menu-bar graph supported | signInProvider | Logo asset (`ProviderLogo`) |
|---|---|---|---|---|---|---|---|
| 0 | `overview` | Overview | **All** | (excluded) | no | overview | none in UI (switcher shows no icon; panel header uses SF Symbol `square.grid.2x2`; menu bar in selected mode uses `TokenMonMark`); `ProviderLogo.image(.overview)` falls back to Grok mark |
| 1 | `grok` | Grok | Grok | 1st | yes (always drawn when enabled, see 2.2) | grok | `MenuBarIcon` (PDF, Grok singularity mark) |
| 2 | `cursor` | Cursor | Cursor | 2nd | yes | cursor | `CursorLogo` (cursor-logo.svg) |
| 3 | `opencode` | OpenCode | OpenCode | 3rd | yes | opencode | `OpenCodeLogo` (opencode-logo-light-square.svg) |
| 4 | `claude` | Claude | Claude | 4th | yes | claude | `AnthropicLogo` (anthropic_small.svg) |
| 5 | `chatgpt` | ChatGPT | ChatGPT | 5th | **no** | chatgpt | `OpenAILogo` (openai_small.svg) |
| 6 | `openrouter` | OpenRouter | OpenRouter | 6th | **no** | openrouter (but OpenRouter has no sign-in window: it connects with an API key in Settings) | `OpenRouterLogo` (openrouter.svg); fallback SF Symbol `network` |
| 7 | `grokbot` | Grokbot | Grokbot | 7th | yes | **cursor** (Grokbot bills through and shares Cursor's session) | `GrokbotLogo` (grokbot_small.svg); fallback SF Symbol `circle.dashed` |

Additional mark: `ProviderLogo.tokenmon` = asset `TokenMonMark` (the app mascot silhouette), used in the
Overview header and the selected-provider menu-bar label for Overview.

All `ProviderLogo` images are forced to **template** (monochrome alpha mask tinted by the caller), even assets
whose catalog entry is not template. Missing asset fallback = empty 16x16 image.

Static lists:
- `usageProviders` = `[grok, cursor, opencode, claude, chatgpt, openrouter, grokbot]` (default dropdown + menu-bar order).
- `menuBarGraphProviders` = `[grok, grokbot, claude, cursor, opencode]` (the order Settings lists the toggles; NOT the drawing order).
- `supportsMenuBarGraph(p)` = `menuBarGraphProviders.contains(p)`.

Functions (must be ported exactly):
- `normalizedOrder(raw)`: iterate `raw`; skip `overview`, duplicates and anything not in `usageProviders`; keep the rest in order;
  then append every `usageProviders` entry not yet seen, in `usageProviders` order. Result always has exactly 7 entries.
- `polls(provider)` on a mode `self`: `self == provider || self == .overview`.
- `signInProvider`: `grokbot -> cursor`, else self.

### 1.2 Accent colors (`ProviderColors.swift`, `ColorPalette.swift`)

`SRGB { red, green, blue, alpha = 1 }` converts to both SwiftUI `Color` (sRGB) and AppKit `NSColor(calibratedRed:...)`.
`accent` is the canonical brand color (used at full alpha in the menu bar). `accentColor` = `accent` at
**opacity 0.85** (used for bars/charts inside the panel).

| Provider | accent (R, G, B floats) | Hex | Panel `accentColor` |
|---|---|---|---|
| overview | 0.50, 0.50, 0.50 | `#808080` | same @ 0.85 alpha |
| grok | 0.11, 0.38, 0.82 | `#1C61D1` | @ 0.85 |
| opencode | 1.00, 0.55, 0.00 | `#FF8C00` | @ 0.85 |
| cursor | 0.18, 0.53, 0.38 | `#2E8761` | @ 0.85 |
| claude | 0.85, 0.47, 0.34 | `#D97857` | @ 0.85 |
| chatgpt | 0.16, 0.52, 0.46 | `#298575` | @ 0.85 |
| openrouter | 0.45, 0.36, 0.90 | `#735CE6` | @ 0.85 |
| grokbot | 0.38, 0.42, 0.50 | `#616B80` | @ 0.85 |

Related palettes referenced by components in this slice:

| Token | Floats | Hex | Used for |
|---|---|---|---|
| ProductColor.chat | = grok accent | `#1C61D1` | Grok "Chat" product, Grok menu-bar bar |
| ProductColor.build | 0.55, 0.78, 1.00 | `#8CC7FF` | Grok Build |
| ProductColor.voice | 0.40, 0.55, 0.82 | `#668CD1` | Voice |
| ProductColor.api | 0.22, 0.32, 0.48 | `#38527A` | API |
| ProductColor.imagine | 0.75, 0.90, 1.00 | `#BFE6FF` | Imagine |
| ProductColor.other | 0.45, 0.45, 0.45 | `#737373` | Other |
| ModelPalette.orange | = opencode accent | `#FF8C00` | OpenCode Go |
| ModelPalette.purple | 0.58, 0.44, 0.86 | `#9470DB` | OpenCode Zen / OpenCode monthly bar in Overview |
| `usageRemainingTrack` | `Color.primary` @ 0.12 | label color 12% | empty tracks |

ProductColor mapping from product id (`ProductColor.from`, lowercased): `build|grok_build|grok-build|grok build -> build`;
`api -> api`; `chat|grok|text -> chat`; `imagine|image|video|media -> imagine`; `voice -> voice`; else `other`.
Product short names (`ProductCatalog.shortName`): build→"Build", api→"API", chat→"Chat", imagine→"Imagine", voice→"Voice",
other→"Other", else `displayName` (capitalized id). Display order: `chat, build, imagine, voice, api, other`.

`ModelCompanyLogo` (used by model rows in provider panels): 28x28 circle, stroke `primary @ 0.08` 1pt.
Company detection on lowercased model id, first match wins: `muse-spark|muse_spark -> meta`, `kimi`, `qwen`, `glm`,
`mimo|minimax -> minimax`, `deepseek`, `nvidia`, `muse`, `claude|anthropic -> anthropic`, `gpt|openai -> openai`,
`llama|meta -> meta`, else `other`. With an asset: white circle + logo tinted black, padding 4 (deepseek 5).
Without (muse, other): filled circle + letter in 12pt bold rounded font.

| Company | Asset | Letter | Fallback bg (floats / hex) | Fg |
|---|---|---|---|---|
| kimi | KimiLogo (png) | K | 0.12,0.12,0.13 `#1F1F21` | white |
| qwen | AlibabaLogo | Q | 0.32,0.25,0.65 `#5240A6` | white |
| glm | ZaiLogo | Z | white | **black** |
| minimax | MinimaxLogo | M | 0.95,0.25,0.40 `#F24066` | white |
| deepseek | DeepSeekLogo | D | 0.11,0.38,0.82 `#1C61D1` | white |
| nvidia | NvidiaLogo | N | 0.45,0.72,0.11 `#73B81C` | white |
| muse | (none) | M | 0.90,0.20,0.22 `#E63338` | white |
| anthropic | AnthropicLogo | A | 0.85,0.65,0.25 `#D9A640` | white |
| openai | OpenAILogo | O | 0.10,0.10,0.10 `#1A1A1A` | white |
| meta | MetaLogo | M | 0.05,0.45,0.95 `#0D73F2` | white |
| other | (none) | • | primary @ 0.12 | white |

### 1.3 ProviderRegistry (`ProviderRegistry.swift`)

Model-layer map `provider -> Entry { provider, poller, auth }`. `auth` is an object identity; providers that share a
session hold the *same* object (Grokbot's entry holds `cursorAuth`).
- `all`: registered entries in `MonitorProvider.allCases` order (Overview never registered) →
  `grok, cursor, opencode, claude, chatgpt, openrouter, grokbot`.
- `pollers(sharingSessionWith: p)`: every registered poller whose `auth` is identical (`===`) to `p`'s, in allCases order
  (Cursor → `[cursorPoller, grokbotPoller]`). Unregistered `p` → `[]`.
- `startAll()`: `start()` on every poller.
Registration in `AppModel`: grok, opencode, cursor, claude, chatgpt, openrouter, grokbot (registration order is irrelevant; `all` sorts).

App wiring that iterates the registry: menu-open refresh (`setMenuOpen`), sign-in completion
(`signInCompleted(p)` refreshes `pollers(sharingSessionWith: p)` then hides the dock icon if no window is open),
usage alerts, and the menu-bar `usedPercent` dictionary (each poller's `headlineUsedPercent`, omitted when nil).
`setMenuOpen(open)`: sets `menuIsOpen` on every poller (switches to the active poll interval); on open, for each
provider where `settings.needsPolling(p)`, calls `refreshNow(ifOlderThan: 15 s)` (`menuOpenFreshness`).
Change coalescing for observers: 100 ms (`AppModel.changeCoalescing`).

### 1.4 Settings inputs consumed by this slice (owned by the Settings slice)

| Setting (UserDefaults key) | Default | Effect here |
|---|---|---|
| `selectedProvider` | `grok` (if unknown) | Active dropdown tab; selected-mode menu-bar label. If the selected provider gets disabled, it is reset to `overview`. |
| `providerOrder` | `usageProviders` | Normalized with `normalizedOrder`; drives tab order, Overview cards and composite menu-bar order. |
| `enabledProviderIDs` | all 7 | Must be a non-empty subset of `usageProviders` (otherwise the assignment is reverted). Disabled providers have no tab, no Overview card, no menu-bar segment. |
| `menuBarGraphProviders` | `{grok}` (migrated from legacy per-provider flags `showGrokBarInMenuBar` (default on), `showOpenCodeBarInMenuBar`, `showCursorBarInMenuBar`, `showClaudeBarInMenuBar`, `showGrokbotBarInMenuBar` (default off)) | Which providers draw a bar segment; intersected with `menuBarGraphProviders` support list. Turning a graph on triggers an immediate `refreshNow()` for that provider. |
| `showSelectedProviderInMenuBar` | false | true → single "selected provider" label; false → composite. |
| `showCategoriesInMenuBar` | true | Grok product dots+labels in the composite. |
| `visibleProductIDs` | all known (`build, api, chat, imagine, voice, other`) | Filters Grok products in the menu bar. |
| `collapsedPanelSections` | `[]` | `PanelSection` raw values `claudeModels`, `openCodeModels` (do not rename). |

`visibleUsageProviders` = `orderedUsageProviders.filter(enabled)`. `needsPolling(p)` = enabled && (`selectedProvider.polls(p)` || graph shown || alert watches p).

### 1.5 Provider switcher layout (`ProviderSwitcherView`)

Tabs = `[overview] + settings.visibleUsageProviders` (so 2..8 tabs).

| Property | Value |
|---|---|
| Max columns per row (`maxPerRow`) | **4** |
| Row height | **28** |
| Rows | `ceil(n/4)`; providers chunked in order; the last row is padded with empty (nil) cells up to 4 |
| Cell width | all 4 cells of every row equal width = containerWidth/4 (columns line up across rows; empty cells occupy space) |
| Container | full panel content width (420 − 2×12 = **396**); clip to rounded rect radius **6**; 1pt stroke `primary @ 0.12` around the whole grid |
| Vertical divider | 1pt `primary @ 0.12` at the leading edge of cell `i>0` **when cell `i-1` is a real tab** (so a divider also appears after the last real tab of a short row, between it and the first empty cell) |
| Horizontal divider | 1pt `primary @ 0.12` across the full top edge of every row except the first |
| Tab content | HStack spacing 4, centered: logo 12x12 (template, tinted with the text color; omitted for Overview) + label `switcherLabel` in caption font (11pt regular), `lineLimit(1)`, `minimumScaleFactor 0.7` (shrinks to 70% before truncating) |
| Selected tab | text `primary`; background `primary @ 0.08` filling the cell |
| Unselected tab | text `secondary`; clear background |
| Click | whole cell is hit area; sets `settings.selectedProvider` (persists) |
| Font weight never changes on selection (keeps geometry stable). |

Invariant tested: the grid's outer drawn edges coincide with a `PanelCard`'s edges (within 0.01pt) at 1x and 2x for every tab count 1..8.

---

## 2. Menu-bar status rendering (`MenuBarStatusRenderer`, `MenuBarController`)

### 2.1 Inputs

```
Input {
  selectedProvider, showSelectedProvider,
  usedPercent: [provider: Double]   // headline used % of each provider with data
  grokSnapshot: WeeklyUsageSnapshot? (usedPercent, products[])
  isGrokSignedIn                     // grokAuth.isSignedIn && !grokAuth.needsSignIn
  menuBarGraphs: Set, showGrokCategories, providerOrder, visibleProductIDs, enabledProviders
}
```

Output: `RenderedStatus { image (bitmap), regions: [Region{provider, minX, maxX}] (image coords, x from left), key }`.

### 2.2 Shared drawing constants

| Constant | Value |
|---|---|
| Image height | **22** |
| Font | system monospaced-digit, **12.5pt, medium** weight (Windows: Segoe UI Variable/Segoe UI with tabular figures `tnum`, 12.5 DIP, weight 500) |
| Text color ("chrome") | menu-bar label color for the current appearance (light menu bar: black/near-black; dark: white). Recomputed per appearance. |
| Icon box | 16x16 (OpenCode: **13x13** because its SVG is full-bleed) |
| gap | 7 |
| segmentGap (between providers) | 10 |
| Bar | 48 wide × 8 tall, capsule (corner radius 4) |
| Bar track | chrome color @ **0.22** alpha, full capsule |
| Bar fill | `max(2, 48 * clamp(used,0,100)/100)` wide from the left, clipped to the capsule (a **2pt sliver** is always shown, even at 0%) |
| Category dot | 7x7 circle |
| Percent text | `"\(Int(round(used)))%"`, or **"—"** (em dash U+2014) when no data |
| Percent slot width | `ceil(width of "100%")` in the font (fixed slot so the item width never depends on the value) |
| Text vertical position | baseline-independent: `y = midY − textHeight/2 − 0.5` |
| Icons | drawn into their box, then tinted to chrome color (template, `sourceIn`) |
| Bitmap | rendered at **2x** scale (pixels = ceil(pt*2)), RGBA, high interpolation |
| Minimum width | 20 |
| Trailing pad | +2 then `ceil` |

### 2.3 Composite mode (`showSelectedProvider == false`)

Pieces are built by iterating `normalizedOrder(providerOrder)`:
1. Skip providers not in `enabledProviders`.
2. `grok` → always a **Grok block** (even if its graph is off).
3. Any other provider → a **solid segment** only if `supportsMenuBarGraph` AND in `menuBarGraphs`. (ChatGPT/OpenRouter never appear.)

`selectedProvider` is ignored in composite mode.

**Grok block** (left → right):
- Grok icon 16x16.
- gap 7, then text centered in `grokTextSlot = ceil(max(width("Grok"), percentSlotWidth))`:
  - signed in AND snapshot present (`grokSigned`): `"NN%"` of `snapshot.usedPercent`;
  - otherwise the literal word **"Grok"**.
- If `grok ∈ menuBarGraphs`: gap 7 + bar 48 (fill = snapshot.usedPercent when grokSigned else 0 → 2pt sliver), color = ProductColor.chat (`#1C61D1`, full alpha). The slot is reserved even when signed out.
- If grokSigned AND `showGrokCategories`: for each product in `ProductCatalog.filtered(snapshot.products, visible: visibleProductIDs, threshold: 0.05)` (ordering per Grok slice): gap 7 + 7x7 dot in the product color + 4 + label `"<shortName> <Int(round(percentOfPool))>%"` (same font/color).
- Block width = `16 + 7 + grokTextSlot [+ 7 + 48] [+ Σ(7 + 7 + 4 + labelWidth)]`.

**Solid segment** (per provider): icon (16, OpenCode 13) + 7 + percent text centered in percentSlotWidth + 7 + bar 48 (fill = usedPercent or 0; color = `provider.accent` full alpha).
Width = `iconBox + 7 + percentSlotWidth + 7 + 48`.

Total width = Σ piece widths + 10 × (pieces−1), then `ceil(total + 2)`, minimum 20. No pieces (Grok disabled and no graphs) → empty 20x22 image.

Regions: one per piece, `[pieceStart, pieceEnd]`, in drawing order; gaps of 10 between regions. E.g. default order with graphs `{grok, opencode, cursor, claude, grokbot}` → regions `[grok, cursor, opencode, claude, grokbot]`.

### 2.4 Selected-provider mode (`showSelectedProvider == true`)

Fixed geometry for every provider (so the dropdown anchor never moves when switching):
- Icon 16x16 at x=0: `TokenMonMark` for Overview, else the provider logo (OpenCode is still drawn at 16 here).
- Text at `x = 16 + 7`, **left-aligned** (not centered) in a slot of `ceil(width("100%"))`.
  Value: `usedPercent[selected]`; for Grok when `!isGrokSignedIn` → nil. Overview never has a value → "—".
- Bar at `x = 23 + slot + 7`, 48×8; fill color = provider accent, or the chrome (label) color for Overview.
- Width = `ceil(barX + 48 + 2)`, min 20. Grok categories are never shown in this mode.
- One region covering `[0, imageWidth]` for the selected provider.

### 2.5 Caching, re-render triggers

- Cache: `NSCache`, **countLimit 40**, keyed by a string built from every drawn input. Key format
  (joined with `-`):
  1. `mb-<1|0 showSelected>-<selectedProvider>-<percents>` where `<percents>` = for each of the 7 `usageProviders` in default order, `Int(round(p))` or `-1`, joined by `-`;
  2. `<isGrokSignedIn>-<showGrokCategories>`;
  3. sorted menuBarGraphs raw values joined by `,`;
  4. providerOrder raw values joined by `,` (unnormalized, as given);
  5. sorted enabledProviders joined by `,`;
  6. `<productKey>-<sorted visibleProductIDs joined ,>-<appearanceName>` where productKey = `id:Int(round(percentOfPool))` of the filtered Grok products joined by `,`, and appearanceName is the best match of `darkAqua`/`aqua`.
  Percentages are rounded *as drawn*, so 41.2 and 40.9 share a key.
- The controller remembers the key on the button and **only swaps the image when the key changes**; regions are always refreshed.
- Re-render triggers: any input change (observed, coalesced 100 ms), the status button's `effectiveAppearance` change, and the
  distributed `AppleInterfaceThemeChangedNotification` (re-rendered one runloop later).
- The status item stays variable-length (an explicit length animates and shifts the whole menu bar).

### 2.6 Click behaviour (`MenuBarController.handleStatusItemClick`)

- Fires on **left mouse up** only. No right-click menu.
- Hit-test: live pointer x mapped into the button, minus the image origin inside the button (AppKit pads the button), then
  `providerSnapped(atX:, tolerance 8)`: exact region match first; otherwise the region with the smallest horizontal distance
  if that distance ≤ 8pt; else nil.
- Panel **visible**: if the click resolves to a provider ≠ current `selectedProvider` → switch tab in place (`selectedProvider = clicked`, panel stays open). Otherwise (same provider or miss) → hide panel.
- Panel **hidden**: if a provider resolved → `selectedProvider = clicked`; then show panel. A miss keeps the current selection.
- In selected mode the single region means a click always resolves to the current selection (→ open / close toggle).

### 2.7 Windows tray mapping (proposal)

Windows notification-area icons are a fixed square (16x16 at 100%, 20x20 at 125%, 24x24 at 150%, 32x32 at 200%, via
`SM_CXSMICON`), cannot be wider, and cannot be hit-tested per segment. Proposal for parity of *information*, not pixels:

| macOS behaviour | Windows equivalent |
|---|---|
| Composite bitmap with N segments | **One tray icon** (`Shell_NotifyIconW`, NIF_ICON/NIF_TIP/NIF_GUID) rendered dynamically at `GetSystemMetricsForDpi(SM_CXSMICON, dpi)`; regenerate on DPI change (`WM_DPICHANGED`/`TaskbarCreated`). |
| Icon content | Mode A ("selected provider", and the default when one graph is on): provider logo silhouette on top (~60% height) plus a horizontal usage bar along the bottom (2px at 16px, 3–4px at 32px) in the provider accent over a track of the tray text color @ 22%; same 2px minimum sliver rule. Mode B (composite with ≥2 graphs): up to 4 vertical mini-bars side by side (one per segment in composite order, each in its accent, track @ 22%); with >4 segments show the 4 first in order. Overview selected: TokenMon mark + bar in the text color. Consider drawing the `NN` number instead of the logo at ≥24px when a single provider is shown. |
| Light/dark menu bar tint | Read `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\SystemUsesLightTheme` (taskbar theme, not `AppsUseLightTheme`); chrome color = black on light taskbar, white on dark. Re-render on `WM_SETTINGCHANGE` with `lParam == "ImmersiveColorSet"`. Include the theme in the cache key exactly like `appearanceName`. |
| Cache key / swap-only-on-change | Port the same key string (replace appearance name with `light`/`dark` + icon pixel size) and only call `NIM_MODIFY` when it changes (avoids flicker). LRU of 40. |
| Per-segment text | **Tooltip** (`szTip`, max 127 chars; use NIF_SHOWTIP + NOTIFYICON_VERSION_4): one line per segment in composite order, e.g. `Grok 41% · Cursor 12% · OpenCode — · Claude 63%` or multi-line `Grok 41%\nChat 22% Build 9%`. "—" for no data; "Grok" when signed out. Truncate at 127 chars. Optionally a richer custom hover flyout. |
| Click a provider's bar to open its tab | Not possible per-pixel. Left click toggles the popup on the current `selectedProvider` (same toggle semantics: click while open → close). Provider choice happens in the popup's switcher. Right click → context menu with one item per composite segment ("Open Grok", "Open Cursor", …: sets `selectedProvider` and opens the popup), separator, "Settings…", "Quit TokenMon". |
| Escape closes panel | Same (popup handles `VK_ESCAPE`). |
| Fixed-width anchor | Not needed: popup anchors to the tray icon rect from `Shell_NotifyIconGetRect`. |

---

## 3. Panel window (dropdown)

### 3.1 Window (`MenuBarPanel`, `MenuBarController.showPanel/panelFrame`)

| Property | Value |
|---|---|
| Kind | borderless, non-activating floating panel, no arrow; can become key, never main; level popUpMenu; shadow on; not movable; joins all Spaces / full-screen auxiliary; no open/close animation; transparent window background |
| Width | **420** fixed (`MenuBarPanelView.panelWidth`) |
| Initial frame before measurement | 420 × 480 |
| Height | content height as reported by layout (`max(200, ceil(measured))`), clamped to `screen.visibleFrame.height − 16` |
| Position | x = status-button midX − 210, clamped to `[visible.minX + 8, visible.maxX − 420 − 8]`; top edge = button bottom − **4** gap; y clamped to `[visible.minY + 8, visible.maxY − height − 8]` |
| Resize | every time content height changes (tab switch, section expand, new data) the panel re-fits **on the next runloop tick**, keeping the top edge fixed and growing the bottom. Re-fit only if height or origin differs by > 0.5. |
| Scrolling | content is inside one vertical scroll view; scroll bar is visible **only** when the screen clamped the height (`isHeightClamped`), otherwise hidden; bounce only when content is taller. Content top-aligned (never vertically centered while a resize is pending). |
| Show | rebuild content, set frame, make key and order front, `model.setMenuOpen(true)` |
| Hide | order out, drop the content tree, reset measured height, `model.setMenuOpen(false)` |
| Dismissal | (a) panel loses key status; (b) app resigns active (click into another app); (c) **Escape** key (keyCode 53) while visible (event swallowed); (d) clicking the status item on the active provider or on a miss. |

Windows: a borderless, topmost, tool window (`WS_POPUP`, `WS_EX_TOOLWINDOW | WS_EX_TOPMOST`, no taskbar button), DWM rounded corners
(`DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND`) and shadow, placed above the tray icon rect (taskbar at bottom: bottom edge =
icon top − 4; taskbar top/left/right: mirror), clamped 8 DIP inside the monitor work area, height clamped to work-area height − 16.
Grow from the edge nearest the taskbar (anchor edge fixed). Dismiss on `WM_ACTIVATE(WA_INACTIVE)`, Escape, or tray click per 2.6.

### 3.2 Root layout (`MenuBarPanelView`)

VStack, alignment leading, spacing 0, padding **12** all sides, width 420, background = window background color, clipped to
rounded rect radius **12**:
1. `ProviderSwitcherView` (section 1.5).
2. 12pt spacer.
3. Content of `settings.selectedProvider`: `OverviewPanelView` | `GrokPanelView` | `OpenCodePanelView` | `CursorPanelView` |
   `ClaudePanelView` | `ChatGPTPanelView` | `OpenRouterPanelView` | `GrokbotPanelView` (non-overview panels are specified in the
   provider slices). Sign-in buttons inside them open `AppWindow.signIn(provider)`; Grokbot opens `signIn(.cursor)`;
   OpenRouter has none (API key in Settings). `openPreferences` (cog) opens Settings.
4. `UpdateAvailableIndicator`: only when the update checker has an available release; top padding 10; plain button with
   `arrow.down.circle` icon + `"TokenMon <ver> is available"` (or `"Installing TokenMon <ver>…"` while installing), caption
   font, tint/accent color, disabled when `!checker.canAct`, tooltip "Download, verify, and install the update"; optional
   `statusMessage` below in caption2 secondary; click → `performPrimaryAction()`.

Opening any window (Settings or sign-in) first shows the Dock icon (`setActivationPolicy(.regular)`) and activates the app;
when the last such window closes the app returns to accessory (no Dock icon) after a 0.25 s grace period (cancelled if another window
opens). Windows: show a taskbar button only while Settings / sign-in windows exist (they are normal top-level windows).

### 3.3 Typography (`PanelTypography`)

| Token | Size | Weight | Notes |
|---|---|---|---|
| title | 13 | semibold | panel/section titles |
| body | 12 | regular | |
| bodySemibold | 12 | semibold | |
| bodyDigit | 12 | semibold | monospaced digits |
| microDigit | 10 | semibold | monospaced digits (chart labels) |
| caption | 11 | regular | |
| captionSemibold | 11 | semibold | |
| micro | 10 | regular | |
| metricLabel | 12 | regular | |

Semantic text colors (map to Windows): `primary` ≈ label (light: black @ 0.85; dark: white @ 0.85), `secondary` (light: black @ 0.50; dark: white @ 0.55),
`tertiary` (light: black @ 0.26; dark: white @ 0.25). System backgrounds: window background (light ≈ `#ECECEC`, dark ≈ `#323232`),
control background (light `#FFFFFF`, dark ≈ `#1E1E1E`), separator (light black @ 0.10, dark white @ 0.10). These macOS values are
approximate; follow the Windows app theme (`AppsUseLightTheme`) for the panel.

Text styles:
- `PanelSectionHeader(title)`: micro (10) **semibold**, letter tracking **1.6pt**, UPPERCASED, secondary.
- `PanelSectionHeaderRow(title, trailing?)`: header + spacer + optional trailing in bodyDigit primary; spacing 8, first-baseline aligned.
- `panelMetaLabel(uppercased)`: micro semibold, secondary; when uppercased also tracking 1.6 + uppercase (e.g. "— WEEKLY").
- `resetCaption()`: metricLabel (12 regular), tertiary.

Chart stem constants (`PanelChartStem`): height **68**, width **12**, corner radius **5** (shared by daily bars and Overview hourly bars).

### 3.4 Shared components

**PanelCard**: VStack leading, spacing 8, padding 12, full width; background control-background color in rounded rect radius 12;
1pt stroke separator color.

**MetricStat(title, value, monospaced = true)**: VStack leading spacing 6: `PanelSectionHeader(title)` + value in bodyDigit primary,
1 line, min scale 0.55; padding 14; full width. **MetricStatGrid([4 stats])**: 2×2 (first two on top), cells equal; 1pt
separator-color hairlines full height (vertical, centered) and full width (horizontal, centered); background control-background,
radius 12, 1pt separator stroke, clipped.

**SlimUsageTrack(label, percent, color, caption?, showsLabel = true)**: VStack spacing 5. If showsLabel: HStack spacing 8:
label (bodyDigit primary), spacer, `"\(Int(round(clamp(percent))))% Used"` (bodyDigit primary). Track: height 8, capsule
`usageRemainingTrack`; fill capsule in `color` width `trackWidth * clamp(p)/100` (no minimum). Caption (if non-empty) in resetCaption style.

**SegmentedUsageBar(products, height = 8)**: visible = products with `percentOfPool > 0.05`; used = min(100, Σ max(0,p));
remainder = max(0, 100 − used); slots = visible + (remainder > 0.5 ? 1 : 0); gaps of **2** between slots; usable = width − 2×(slots−1).
Widths = `segmentWidths(percents: visible% + [remainder if > 0.5], usable, minWidth: 3)`:
each = max(3, usable × max(0,p)/100); if usable > 0 and Σ > usable, scale all by usable/Σ. Segments: rounded rect radius 2 in product
color; remainder slot in `usageRemainingTrack`.

**CategoryRow(product)**: HStack spacing 5: 7x7 circle in product color, short name (bodySemibold primary, 1 line), `"\(Int(round(percentOfPool)))%"` (bodyDigit primary).

**StaleDataCaption(message, lastRefreshedAt?)**: VStack leading spacing 2, caption font secondary, full width: message; then
`"Showing data from <relative time> ago."` when a date is given (relative style, e.g. "5 min" → "Showing data from 5 min ago.";
live-updating).

**CollapsibleSectionHeader(title, isExpanded)**: full-row plain button; `PanelSectionHeader(title)` + spacer + chevron-down
(caption size, secondary) rotated **0°** when expanded, **−90°** (pointing right) when collapsed; toggle animated ease-in-out 0.2 s;
accessibility label = title, value "Expanded"/"Collapsed". State persists via `collapsedPanelSections`.

**ProviderHeaderRow(provider, title, trailing)**: HStack spacing 8: `ProviderHeaderLabel` (HStack spacing 6: 14x14 logo or, for
Overview, SF Symbol `square.grid.2x2` 12pt medium primary; title in `title` font) + spacer(min 8) + trailing + `SettingsCogButton`.
**SettingsCogButton**: `gearshape` icon, body font (12), secondary, plain, tooltip "Settings", opens Settings.

**ProviderSignInButton(provider, title?)**: standard button, caption font, label `"Sign In to <displayName>…"` (… = U+2026) unless overridden.

**ProviderSignOutButton(provider, action)**: full-width row, right-aligned (spacer first), spacing 6.
- Idle: plain icon button `person.crop.circle.badge.minus`, caption size, tertiary color, tooltip/a11y "Sign out".
- Click → confirm state: text `"Sign out of <displayName>?"` (caption, secondary) + `checkmark.circle.fill` in **red** (tooltip/a11y "Confirm sign out") + `xmark.circle` secondary (tooltip/a11y "Cancel").
- Confirm → back to idle and run `action` (typically `auth.signOut()`). Cancel → back to idle. Confirmation state is local, not persisted.

**DailyBudgetBarsView** (shared by Weekly/Monthly variants) inputs: `days[DailyBudgetDay{date, spentUSD (= % of pool), budgetUSD (= daily allowance %), percentOfBudget, isOverBudget, isPriorWindow, tokens?}]`, accent, title (default "Daily Budget"), allowancePeriod (`.weekly|.monthly`), allowanceNoun ("weekly"/"monthly"), periodUsedPercent?, resetsAt?, periodStart?, onPreviousWeek?, onNextWeek?, canGoNext, showsLiveFooter.
Layout VStack leading spacing 8:
1. Header (first-baseline): `PanelSectionHeader(title)`; if dailyBudget (`days.first.budgetUSD`) > 0: `"%.1f%%"` of it (bodyDigit primary); spacer(min 8); range label (bodyDigit primary):
   same month → `"<MMMM d of first> – <day of last>"` (e.g. "October 13 – 19"); else `"<MMMM d> – <abbrev month + day>"` (e.g. "September 29 – Oct 5"). En dash with spaces.
2. `WeekChartNavigation` if either callback is non-nil.
3. Bars row: `days.count` equal columns, height `68 + 42 = 110`, hover overlay (below). Each column VStack spacing 4:
   - Stem 12×68, rounded radius 5, track `primary @ 0.12` always full strength;
     fill from the bottom in `accent` (opacity **0.45** if `isPriorWindow`, else 1), height `min(68, max(6, 68 × percentOfBudget/100))`,
     drawn only if `percentOfBudget/100 > 0.005` and the day is not in the future (date > start of today). Fill can exceed 100% of
     allowance but is capped at the stem height.
   - Hovered column: 1pt outline `primary @ 0.35` on the stem (0 alpha otherwise).
   - Labels (height 32, VStack spacing 1): `"\(max(0, Int(round(spentUSD))))%"` (microDigit primary, min scale 0.65) and the
     first 2 chars of the locale's abbreviated weekday (`EEE`, e.g. "Mo") (microDigit secondary). Future days: labels at 0.5 opacity.
4. Footer (metricLabel 12, secondary), only if `showsLiveFooter`:
   - with pace (requires `periodUsedPercent`; start = `periodStart` or `DailyBudget.pacePeriodStart`; weekly with unknown start uses nil elapsed; monthly with unknown start → no pace): `DailyBudget.paceCaption(pace)` (strings in the Core slice, e.g. "Usage 3.2% over today's allowance", "No usage yet · 14.3% today", "X% usage left today from unused prior days");
   - else if every day spent ≤ 0.001: `"No usage yet · %.1f%%/day"` of dailyBudget;
   - else if today's day `isOverBudget`: `"Over today's allowance"`; else no footer.
- `WeeklyDailyBudgetBarsView`: allowancePeriod weekly, noun "weekly", periodStart = `DailyBudget.weeklyPacePeriodStart(days)`.
- `MonthlyDailyBudgetBarsView`: monthly, noun "monthly", periodStart = `DailyBudget.monthlyPacePeriodStart(days, knownStart, resetsAt)`; passes `resetsAt` through (tested).

**WeekChartNavigation(onPreviousWeek?, onNextWeek?, canGoNext)**: HStack spacing 8: left chevron button, spacer, right chevron button.
Each button 22×22, captionSemibold icon (`chevron.left` / `chevron.right`), background rounded rect radius 5 `primary @ 0.08`;
enabled color secondary, disabled `secondary @ 0.35`. Left disabled iff `onPreviousWeek == nil`; right disabled iff `!canGoNext`.

**Week paging (`NavigableDailyBudgetCard`, `WeekOffset`)**: local state `weekOffset` (0 = window containing today, negative = past).
`previous(o) = o − 1` (unbounded), `next(o) = min(0, o + 1)`, `canGoNext(o) = o < 0`. Data comes from `daysForWeek(offset)`; empty → the
whole card is hidden. Past weeks (`offset != 0`) pass `periodUsedPercent = nil`, `resetsAt = nil`, `periodStart = nil`,
`showsLiveFooter = false`. Wrapped in `PanelCard`; z-index raised so the hover label can overflow the following cards.
State is not persisted (resets to 0 when the panel is rebuilt, i.e. on every open).

**Hover label (`DailyBudgetHover`, `DailyBudgetHoverLabel`)**, shared by every daily bar chart:
- `columnIndex(x, columnWidth, columns)`: nil if columns < 1, columnWidth ≤ 0 or x < 0; `Int(x / columnWidth)` if < columns else nil. columnWidth = rowWidth / days.count.
- Pointer gap = **14**. `labelOrigin(pointer, labelSize, containerWidth)`: left = px − 14 − w; right = px + 14;
  x = left if left ≥ 0; else right if right + w ≤ containerWidth; else `max(0, min(left, containerWidth − w))`. y = py − h/2 (may go negative / overflow the row; the label is not clipped).
- Label hidden (opacity 0) until its size has been measured once; never hit-testable; cleared on hover end.
- Content: VStack leading spacing 4, padding h10 v8, background window-background color in rounded rect radius 8,
  1pt border `primary @ 0.12`, shadow black @ 0.25 radius 6 y-offset 2:
  1. Date `"EEEE, MMM d"` (e.g. "Tuesday, Oct 14"), captionSemibold primary.
  2. Unless future: `usageLine(day, noun)` (caption secondary):
     - prior window: `"%.1f%% of the <noun> pool before the provider reset it early"`;
     - budget ≤ 0: `"No budget"`;
     - spent ≤ 0.001: `"0% of the %.1f%% daily allowance"` (e.g. "0% of the 14.3% daily allowance");
     - else `"%.1f%% of <noun> · %d%% of daily allowance"` with `round(spent/budget×100)` (e.g. "31.0% of weekly · 217% of daily allowance").
  3. If `day.tokens.totalTokens > 0`: divider (40% opacity); HStack first-baseline spacing 4: `Format.tokens(total)` (title 13 semibold mono digits primary), "tokens" (caption secondary), and if `costUSD > 0`: spacer(min 10) + `"≈ " + Format.usd(cost) + ("+" if hasUnpriced)` (bodyDigit secondary). Then a 2-column grid (h-spacing 12, v-spacing 1): rows "Input", "Output", "Cache read", "Cache write" → `Format.tokens(n)` (label caption secondary; value microDigit primary right-aligned). If models non-empty: divider + up to **3** model rows (name → tokens).

**DailyUsageChartView** (Grok billing-period week; same visual system):
header `PanelSectionHeader("Daily Budget")` + `"%.1f%%"` of `GrokDailyUsageBuilder.dailyCapPercent` (= 100/7 ≈ 14.3) + spacer + `week.rangeLabel`;
`WeekChartNavigation` always shown (`canGoNext` default true); columns from `week.displayDays` with fill fraction
`GrokDailyUsageBuilder.fillFraction(totalPercent)`, fill color = Grok `accentColor`, drawn if fraction > 0 and not future (min 6pt);
labels `"\(Int(round(totalPercent)))%"` and `weekdaySymbol.prefix(2)`; hover with allowanceNoun "weekly" over days mapped to
`DailyBudgetDay(date: dayStart, spent: totalPercent, budget: dailyCapPercent)`. Footer: pace caption when `periodUsedPercent`
given (elapsed days from `week.weekStart` through today, incl. `resetsAt`), else if `week.isEstimated || !week.hasDailyData`:
`"Bars update between polls · week totals above"`.

### 3.5 Overview panel (`OverviewPanelView`)

VStack leading spacing **10**:
1. Header HStack (center, spacing 8): TokenMon mark 24×24 (primary tint), "TokenMon" (title), spacer, `"v<CFBundleShortVersionString>"` (bodyDigit tertiary; fallback "v1.0.0"), Settings cog.
2. One card per `settings.visibleUsageProviders` (user order). Every card is a `PanelCard` with:
   header HStack (center, spacing 8): logo 14×14, name (title), meta label (uppercased panelMetaLabel), spacer, `"NN% Used"` (bodyDigit primary);
   then an 8pt capsule track `primary @ 0.12` with fill width `w × clamp(p)/100` (no minimum); then a caption line; then sign-in/out controls.

| Card | Meta | Percent source (0 if nil) | Fill color | Caption (first match) | Sign-in button shown when | Sign-out shown when |
|---|---|---|---|---|---|---|
| Grok | "— Weekly" | `grok.snapshot.usedPercent` | grok accentColor | `Format.resetCaption(resetsAt)` ("Resets Sun 24 August 12:00am"-style, format `EEE dd MMMM h:mma`, lowercase am/pm); else if no snapshot "No Grok data yet" (caption tertiary) | `!signedIn || needsSignIn` | `signedIn && !needsSignIn` |
| OpenCode | "— Monthly" | monthly window `usedPercent`, else `snapshot.monthlyUsedPercent` | ModelPalette.purple `#9470DB` (full alpha) | monthly window reset caption; else "No OpenCode data yet" | `needsSignIn && snapshot == nil` | `!needsSignIn` |
| Cursor | "— Monthly" | `usedPercent` | cursor accentColor | reset caption; else "No Cursor data yet" | `needsSignIn && snapshot == nil` | `!needsSignIn` |
| Claude | "— Weekly" | `sevenDay.usedPercent` | claude accentColor | `sevenDay.resetsAt` caption; else "No Claude data yet" | `needsSignIn && snapshot == nil` | `!needsSignIn` |
| ChatGPT | "— Codex" | `headlineUsedPercent` | chatgpt accentColor | reset caption; else "No ChatGPT data yet" | `needsSignIn && snapshot == nil` | `!needsSignIn` |
| Grokbot | "— Weekly" | `usedPercent` | grokbot accentColor | reset caption; else "No Grokbot data yet" | Cursor `needsSignIn && grokbot snapshot == nil`, button title **"Sign In to Cursor…"**, opens Cursor sign-in | never (session lives on the Cursor card) |
| OpenRouter | "— Credits" | `usedPercent`; header shows `Format.usd(keyUsageUSD)` instead of "% Used" when the snapshot exists but `usedPercent == nil` | openrouter accentColor | if snapshot and `budgetSource == nil`: `"No credit limit — $X spent on this key"`; else if `remainingUSD`: `"$X of credits left"`; else if no snapshot "No OpenRouter data yet" (all caption tertiary) | `needsSignIn && snapshot == nil` → selects the OpenRouter **tab** (API key flow) instead of a window | `!needsSignIn` |

`Format.usd` = en_US currency, `$` symbol (e.g. "$12.34").

3. `OverviewDailyUsageCard` inside a timeline that re-renders at each **local midnight** (`midnightSchedule(from:)` = today's 00:00
   plus the next 7 midnights, 8 entries, DST-correct) so a panel left open rolls over to the new day.

**OverviewDailyUsageCard** (PanelCard): local `dayOffset` (0 = today, −1 = yesterday; not persisted).
- Title (`PanelSectionHeader`): "Usage Today" / "Usage Yesterday" / `"Usage " + "EEE, MMM d"` (e.g. "Usage Mon, Oct 20").
- Series: today → live `todaySeries(now)`; past → `history.series(on: day)` (history keeps 30 days).
- `OverviewHourlyUsageChart(usage: non-empty or nil, emptyMessage)`: today `"No Grok, Grokbot, OpenCode, Cursor, or Claude activity yet today."`, past `"No activity recorded on this day."`.
- Navigation shown if `canGoBack` or `dayOffset < 0`. `canGoBack(day, earliestDay)` = earliestDay exists and startOfDay(earliest) < startOfDay(day). Left: `dayOffset −= 1` (nil/disabled when it can't go back); right: `min(0, dayOffset + 1)`, enabled iff `dayOffset < 0`.

**OverviewHourlyUsageChart**:
- Stack order bottom→top, fixed: Grok, Grokbot, Cursor, Claude, OpenCode Go, OpenCode Zen.
- Colors: grok/grokbot/cursor/claude `accentColor` (0.85); Go = ModelPalette.orange @ 0.85; Zen = ModelPalette.purple @ **0.78**.
- Visible hours: 6…22 widened to include the earliest/latest hour with activity (`visibleHourRange`). If no visible hour has activity → empty message (caption tertiary, min height 48, leading).
- Bars row height `68 + 19 = 87`; one equal-width column per visible hour; VStack spacing 5:
  - Bar height = `barHeight(activity, maxActivity, 68)` = 0 if no activity else `min(68, max(3, 68 × activity/max))`; max = busiest visible hour.
  - Segment heights = `segmentHeights(activities, total: barHeight, minimum: 1)`: proportional; if ≥2 non-zero slices and `1 × count ≤ total`, lift any slice < 1 to 1 and subtract the total lift from the tallest slice. Stacked square-cornered rects, whole bar clipped to radius 5, width 12.
  - Hour with no activity: a 12×2 capsule `primary @ 0.12` at the bottom.
  - Tooltip (`help`): `"<hourLabel>: <Title> <%.2f>% <quotaLabel>[ · <Format.tokens(tokens)>] · …"`; hourLabel `12a, 1a … 12p, 1p …`; quotaLabel: Grok/Grokbot "of weekly", Claude "of 5-hour window", Cursor/Go/Zen "of weekly share". Grokbot never shows tokens.
  - Axis label under each column (bodyDigit 12 secondary, min scale 0.7, height 14): shown only at `axisHours(range)` = both ends + every hour divisible by 3 that is ≥ 2 hours from both ends (6…22 → 6, 9, 12, 15, 18, 22). Label text: 0/12 → "12", 13–23 → hour−12, else hour (no a/p).
- Legend: HStack spacing 10 of providers with any activity in the visible hours, in stack order; each = 8×10 rounded rect (radius 2) + title (bodySemibold secondary): "Grok", "Grokbot", "Cursor", "Claude", "Go", "Zen".

---

## 4. Sign-in UI

### 4.1 Window

- One window per **sign-in session** (`WindowGroup(id: "signin", for: MonitorProvider)` keyed by `provider.signInProvider`; opening again focuses the existing one). Grokbot → Cursor's window.
- Default size **920×700**, minimum content **880×640**, resizable; title = `config.title`. Activates the app on appear.
- On successful sign-in: `model.signInCompleted(signInProvider)` (refresh all pollers sharing the session; Dock-icon hide check).

Per-provider configs (owned by provider slices; listed for completeness):

| Session | title | subtitle | start URL | return delay |
|---|---|---|---|---|
| Grok | Sign in to Grok | Sign in with your Grok / xAI account. This window finishes on its own once you're back on grok.com. | `https://accounts.x.ai/sign-in?redirect=https%3A%2F%2Fgrok.com%2F%3F_s%3Dusage` | 1.5 s |
| Cursor | Sign in to Cursor | Sign in to your Cursor account. This window finishes on its own once you reach the dashboard. | `https://cursor.com/dashboard/usage` | 0.8 s |
| Claude | Sign in to Claude | Sign in to your Claude account. This window finishes on its own once you return to claude.ai. | `https://claude.ai/new` | 0.8 s |
| ChatGPT | Sign in to ChatGPT | Sign in to your ChatGPT account. This window finishes on its own once you return to chatgpt.com. | `https://chatgpt.com/` | 0.8 s |
| OpenCode | Sign in to OpenCode | Sign in to the OpenCode console. This window finishes on its own once the console loads. | `https://opencode.ai/console/login` | 0.8 s |

`ProviderSignInConfig` also carries `isAuthHost(host, path)`, `isReturnPage(url)`, optional `onReturned(url)` (runs when the return
page is detected, before the delayed capture) and `afterCapture()` (async, after a successful capture, before dismissal).

### 4.2 Layout (`ProviderSignInSheet`)

VStack spacing 0: header, divider, navigation bar, divider, web view (fills), divider, footer.

- **Header** (padding default ~16, HStack top-aligned spacing 12): green `checkmark.circle.fill` (title2) only in the signed-in phase;
  VStack (spacing 2): title (title2 ≈ 17pt semibold) and status text (callout ≈ 12pt, secondary, wraps); spacer; **"Cancel"** button (Escape) → close window.
- **Status text**: signingIn: `"Complete sign-in in the popup above."` while a popup is open; else after **25 s** `"Signed in? Choose Finish Sign-In to complete."`; else `config.subtitle`. finishing: `"Finishing sign-in…"`. signedIn: `"Signed in as <email>"` or `"Signed in"`. failed: the message.
- **Navigation bar** (borderless buttons, HStack spacing 8, padding h16 v8):
  - Back `chevron.backward`: enabled iff `canGoBack || hasPopup`; tooltip "Back to the sign-in page" when a popup is open with no history, else "Back"; a11y "Back".
  - Forward `chevron.forward`: enabled iff `canGoForward`; tooltip/a11y "Forward".
  - Reload/Stop: icon `xmark` + "Stop" while loading, else `arrow.clockwise` + "Reload".
  - Address label (caption, secondary, 1 line, middle truncation, fills width, tooltip = full URL): `"Sign-in popup"` when a popup is open, else `host + path` of the current URL (raw string if it has no host).
  - **"Close popup"** button only while a popup is open (tooltip "Return to the provider sign-in page").
  - Small spinner while loading.
- **Footer** (padding ~16): small spinner while finishing; spacer; primary button (Enter/default) titled by phase: "Finish Sign-In" / "Finishing…" / "Try Again" / "Signed In"; disabled while finishing or signed in.

### 4.3 Embedded browser (`SignInBrowserView`, `SignInBrowserController`)

- One main web view + a **stack of popup web views** (OAuth `window.open`). Each popup is created with the configuration the engine
  supplies (same isolated cookie store), placed on top of the main view filling the bounds, gets focus. `window.close()` from a popup → dismiss it.
- Data store: an **isolated per-provider store** (`auth.signInDataStore`); cookies never shared across providers.
- JavaScript on; JS may open windows automatically; back/forward swipe gestures on.
- **Default browser User-Agent** (never the API client UA; Google refuses non-browser UAs). Must not contain "TokenMon".
- Active web view = top popup or main. Controller publishes `canGoBack, canGoForward, isLoading, currentURL, popupDepth` of the active
  view on commit/finish/fail and on property changes; **only changed values are assigned** (no redundant notifications).
  `attach()` is idempotent. `hasPopup = popupDepth > 0`; `backIsEnabled = canGoBack || hasPopup`.
- Back policy (`SignInPopupStack.backAction(canGoBack, popupDepth)`): history → goBack; else popup open → closePopup; else none.
  `SignInBackAction` raw values `goBack`, `closePopup`, `none`; display names "Back", "Close popup", "None".
- Forward: only if the active view can go forward. Reload button: stop if loading else reload.
- Dismissing a popup: stop loading, detach delegates, remove, re-observe the new active view, publish, focus it.

### 4.4 Completion detection

`SignInReturnGate { isAuthHost, isReturnPage, didSeeAuth = false }`. `note(url)` on every **finished** navigation (main or popup):
host lowercased; if `!didSeeAuth && isAuthHost(host, path)` → `didSeeAuth = true`, event `.authHost`; then if `didSeeAuth && isReturnPage(url)`
→ return `.returnPage` (so the same URL can be both); else the event. `matchesReturnPage(url) = didSeeAuth && isReturnPage(url)`.
Event names: `none` "None", `authHost` "Auth host", `returnPage` "Return page". The initial load of the start URL is ignored until an auth host was seen.

Browser handling of `.returnPage`:
- If the navigation was in the **main** view, set `didNavigateMainAfterAuth = true`.
- Fire `onReturned(url)` only if it was the main view AND no popup is open.
- When a popup closes (user, Close popup, or `window.close()`), fire if `didNavigateMainAfterAuth && no popups && matchesReturnPage(main.url)`.
- `onReturned` fires **at most once** (`didFireReturn` latch) until `rearmReturn()` resets it (rearm publishes nothing).

Sheet flow (`captureAfterReturn`), on `onReturned`:
1. Bail if dismissed / finishing / signed in.
2. While a popup is open, poll every 200 ms up to 40 times (8 s).
3. Sleep `returnDelay` (default 0.8 s; Grok 1.5 s). Re-check bail conditions.
4. phase = finishing; `capture()` = `auth.captureCookiesFromWebKit()` then `afterCapture()` if ok.
5. Success → **confirmAndDismiss**: phase signedIn(email = `auth.accountEmail`), call `onComplete`, wait **1.1 s**, close window (if not already closed).
6. Failure → wait 1.2 s, retry capture once (bail if dismissed/signed in); still failing → phase failed(message) and `rearmReturn()`.
Manual **Finish Sign-In / Try Again**: bail if dismissed/busy/signed in; phase finishing; capture; success → confirmAndDismiss; failure → failed + rearm.
Failure message = `auth.lastAuthError` or `"Couldn't find a signed-in session yet. Finish signing in, then try again."`.

Windows mapping: WebView2 with a **separate user-data folder (or profile) per provider** to isolate cookies; handle
`NewWindowRequested` by creating a child WebView2 sharing the same environment/profile and set `args.NewWindow`; handle
`WindowCloseRequested` for popups; read cookies with `CookieManager.GetCookiesAsync`. Keep the default WebView2 UA.

### 4.5 Sign-out

Sign-out is only reachable from `ProviderSignOutButton` (two-step confirm, 3.4) inside the panels/Overview cards; it calls the session's
`signOut()` (semantics in the auth slice; README: local history is kept for when the same account signs back in).

---

## 5. Test parity checklist

| File | Test | Behavior asserted |
|---|---|---|
| TokenMonTests/Providers/Registry/ProviderSwitcherLayoutTests.swift | testRowsPadTheLastRowToFullWidth | For 1..8 tabs, every row from `rows(for:)` has exactly 4 cells; flattening non-nil cells reproduces the input order. |
| ″ | testGridEdgesMatchCardEdgesForEveryProviderCount | At width 396 and scales 1x/2x (light), the switcher's leftmost/rightmost drawn pixel (alpha > 0.02) equals a PanelCard's within 0.01pt for 1..8 tabs. |
| TokenMonTests/UI/MenuBar/MenuBarStatusRendererTests.swift | testAppearanceIsPartOfTheRenderedKey | Light vs dark render: different keys and different bitmaps; same appearance re-render reuses the key. |
| ″ | testSelectedProviderLabelHasFixedWidthAcrossProviders | Selected mode: all 8 providers (incl. Overview) produce identical image size. |
| ″ | testSelectedProviderLabelMatchesOverviewSwitchGeometry | Selected mode: Overview size == OpenRouter size. |
| ″ | testSelectedProviderLabelHasFixedWidthWithAndWithoutData | Selected mode: Grok with no data == Claude with 42% (same width/height). |
| ″ | testCompositeModeRendersVisibleTrackAtZeroUsage | Composite: 0% and 9% Grok produce the same width; the 0% image has non-transparent pixels. |
| ″ | testCompositeModeGrokbotBarWidensLabel | Enabling the Grokbot graph widens the composite; height unchanged. |
| ″ | testCompositeModeIgnoresSelectedProvider | Composite width is the same for selected Grok and OpenRouter. |
| ″ | testCompositeRegionsFollowProviderOrder | Graphs {grok, opencode, cursor, claude, grokbot}, default order → regions `[grok, cursor, opencode, claude, grokbot]`, non-overlapping, minX ≤ maxX, region midpoint maps back to its provider; x = last.maxX+50 and x = −5 → nil. |
| ″ | testSelectedProviderModeHasSingleRegion | Selected Claude → one region `[0, imageWidth]`. |
| ″ | testCompositeOmitsDisabledProviderRegion | Claude region present when enabled, absent when Claude is disabled. |
| ″ | testSnappedProviderResolvesGapClicks | Midpoint of each inter-segment gap: exact lookup nil, snapped lookup non-nil. |
| ″ | testSnappedProviderHandlesEdgePaddingButRejectsFarMisses | first.minX−4 → first provider; last.maxX+2 → last provider; last.maxX+50 and −50 → nil. |
| TokenMonTests/UI/MenuBar/SegmentedUsageBarTests.swift | testWidthsAreProportionalWhenTheyFit | `[50,25,25]`, usable 100, min 3 → `[50,25,25]`. |
| ″ | testFloorsAreNormalizedSoTheyDoNotOverflow | `[0.1,0.1,0.1]`, usable 5, min 3 → sum 5 (±1e-4). |
| ″ | testZeroUsableKeepsFloors | `[50,50]`, usable 0 → `[3,3]`. |
| TokenMonTests/UI/Components/ColorPaletteTests.swift | testSRGBComponentsRoundTrip | SRGB(0.5,0.25,0.75,0.4) keeps components (±1e-4). |
| ″ | testSRGBConversionHelpers | SRGB(0.25,0.5,0.75) → framework color → RGB 0.25/0.5/0.75 (±1e-3). |
| ″ | testProductColorSRGB | build.blue == 1.0; chat.red == 0.11. |
| TokenMonTests/UI/Components/DailyBudgetHoverTests.swift | testColumnIndexMapsThePointerToABar | (0,40,7)→0; (119.9)→2; (279)→6; (280)→nil; (−1)→nil; (10, width 0, cols 0)→nil. |
| ″ | testLabelFloatsBesideThePointer | size 100×60, container 300: pointer (200,50) → (86,20); (40,50) → (54,20); size 250×60 at (100,50) → (0,20). |
| ″ | testUsageLineDescribesTheDay | spent 31, budget 100/7, "weekly" → "31.0% of weekly · 217% of daily allowance"; spent 0 → "0% of the 14.3% daily allowance". |
| TokenMonTests/UI/Components/MonthlyDailyBudgetBarsViewTests.swift | testResetsAtReachesTheSharedBars | Monthly view passes `resetsAt` unchanged to the shared bars and sets allowancePeriod `.monthly`. |
| TokenMonTests/UI/SignIn/SignInBrowserNavigationTests.swift | testBackGoesBackWhenHistoryExists | canGoBack → goBack at depth 0 and 1. |
| ″ | testBackClosesPopupWhenPopupHasNoHistory | no history, depth 1 or 2 → closePopup. |
| ″ | testBackDoesNothingOnMainPageWithoutHistory | no history, depth 0 → none. |
| ″ | testBackActionRawValuesRoundTrip | All cases round-trip raw values; non-empty display names; unknown raw → nil. |
| ″ | testReturnGateIgnoresStartURLUntilAuthHost | Cursor start URL → none; Google auth → authHost (didSeeAuth true); start URL again → returnPage. |
| ″ | testReturnGateDoesNotCaptureBeforeAuth | claude.ai/new → none; claude.ai/login → authHost; /new → returnPage. |
| ″ | testChatGPTReturnPageExcludesLoginAndAuth | ChatGPT predicates: auth.openai.com and chatgpt.com/auth/login are auth hosts; chatgpt.com/ and /c/abc return pages; /auth/login, /login, auth.openai.com are not. (Provider slice.) |
| ″ | testClaudeReturnPageExcludesClerkAndAuthPaths | clerk.claude.ai/sign-in is authHost and never a return page; claude.ai/new, www.claude.ai/chat/abc are; /login, /signin, clerk, auth.claude.ai are not. (Provider slice.) |
| ″ | testApplyDoesNotPublishWhenNothingChanged | First apply notifies observers; 5 identical applies notify nothing. |
| ″ | testApplyPublishesOnceForEachRealChange | depth 0 → hasPopup/backIsEnabled false; depth 1 → change notified, both true; URL change → notified and stored. |
| ″ | testAttachIsIdempotent | Re-attaching the same browser 5× notifies nothing. |
| ″ | testSignInBrowserDoesNotOverrideTheUserAgent | No custom UA; (opt-in `TOKENMON_RUN_WEBKIT_TESTS=1`) `navigator.userAgent` contains "Mozilla/5.0" and "AppleWebKit", not "TokenMon". Windows: assert WebView2 UA unmodified, contains "Mozilla/5.0", not "TokenMon". |
| ″ | testRearmReturnDoesNotPublish | `rearmReturn()` notifies nothing. |
| ″ | testGrokAuthHostUsesExactOrSuffixMatch | accounts.x.ai, x.com, api.x.com true; netflix.com, x.com.evil.example false. (Provider slice.) |
| ″ | testGrokReturnPageRejectsLookalikeHost | grok.com/?_s=usage, www.grok.com true; grok.com.evil.example, notgrok.com false. (Provider slice.) |
| TokenMonTests/Features/Overview/ProviderHourlyUsageTests.swift | testMidnightScheduleStartsAtTodayThenEachLocalMidnight | America/Denver 2026‑03‑07 23:59 → 8 entries starting 03‑07 00:00, then 03‑08, 03‑09 (DST) — all at hour 0. |
| TokenMonTests/Features/Overview/OverviewHourlyHistoryTests.swift | testSegmentHeightsAreProportionalAndSumToTheBar | `[1,3]`, total 40 → `[10,30]`. |
| ″ | testThinSlicesAreLiftedWithoutOverflowingTheBar | `[0.001,0.002,10]`, total 20, min 1 → `[1,1,18]`, sum 20. |
| ″ | testSlicesStayProportionalWhenTheBarIsTooShortForMinimums | `[1,1,2]`, total 2 → sum 2, last = 1. |
| ″ | testBarHeightFollowsActivityWithAVisibleMinimum | track 60: 5/10 → 30; 10/10 → 60; 0.001/10 → 3; 0 → 0. |
| ″ | testVisibleHoursWidenToIncludeLateAndEarlyActivity | [10] → 6…22; [2,23] → 2…23. |
| ″ | testAxisLabelsMatchTheDefaultDayAndNeverCrowd | 6…22 → {6,9,12,15,18,22}; 2…23 → {2,6,9,12,15,18,21,23}. |
| ″ | testDayTitles | 0 → "Usage Today"; −1 → "Usage Yesterday"; −2 starts with "Usage ". |
| ″ | testBackArrowNeedsAnEarlierSavedDay | earliest nil → false; same day → false; previous day → true. |
| TokenMonTests/Providers/Grok/DailyBudgetWeekOffsetTests.swift | testWeekOffsetNeverMovesPastTheCurrentWeek | previous(0) = −1, previous(−2) = −3, next(0) = 0, next(−1) = 0, next(−4) = −3, canGoNext(0) false, canGoNext(−1) true. (Other tests in this file cover `DailyBudget` builders; Core slice.) |
| TokenMonTests/Features/Settings/AppSettingsTests.swift | testNormalizedOrderDropsOverviewAndDuplicates | `[overview, grok, grok, claude]` → starts with grok, one grok, no overview, 7 entries. |

The remaining tests in the Overview history file (record/merge/retention/persistence) belong to the Overview-model slice.

---

## 6. Build, CI and release pipeline (today)

### 6.1 Project / build

- XcodeGen `project.yml`: app `TokenMon` (bundle id **`com.modelmonitor.app`**, product/module TokenMon), macOS deployment **14.0**,
  Swift **5.10**, Xcode version 26.0, hardened runtime, `ENABLE_DEBUG_DYLIB: NO`, `MARKETING_VERSION "1.10.1"`,
  `CURRENT_PROJECT_VERSION "21"` (overridden in release builds). Test bundle `TokenMonTests` (`com.modelmonitor.tests`) hosted in the app;
  coverage gathered for the app target only. No Swift packages / third-party dependencies.
- Info.plist: `LSUIElement = true` (agent app, no Dock icon), category `public.app-category.developer-tools`,
  copyright "Copyright © 2026 Austin Faulkner-Pearce. MIT License.", min system = deployment target.
  Entitlements: empty (not sandboxed). `PrivacyInfo.xcprivacy`: no tracking, no collected data, UserDefaults API reason `CA92.1` (checked present by `make release`).
- **Versioning**: user-facing version = `MARKETING_VERSION` in project.yml (semver `X.Y.Z`). Build number (`CFBundleVersion`) =
  `git rev-list --count HEAD` (commit count on linear master, currently 494) passed as `CURRENT_PROJECT_VERSION`; falls back to
  project.yml's value outside git. CI checks out with `fetch-depth: 0` for this reason. Release tag must be `v<MARKETING_VERSION>`.

### 6.2 Makefile targets

| Target | Does |
|---|---|
| help / tasks (default) | lists targets + detected signing identities |
| build | Debug build, ad-hoc signed (`CODE_SIGN_IDENTITY=-`), DerivedData in `build/DerivedData` |
| run | build, kill running instance (wait ≤ 5 s), `open` (retry after `lsregister -f`) |
| install / uninstall | Release build → ditto to `/Applications` (override `INSTALL_DIR`) / remove |
| release | `_release-app` → `_stage-dist` (ditto app to `dist/`, require PrivacyInfo) → `pkg` → `_zip` → `_dsym` → (NOTARY=1: `notarize`); prints SHA-256 of binary, pkg, zip, dSYM zip |
| _release-app | Release build; Developer ID Application identity (auto-detected from keychain) with `--timestamp --options=runtime` and team id parsed from the identity, else ad-hoc; passes the build number |
| pkg | non-relocatable component pkg (`BundleIsRelocatable = NO`, verified) installing to `/Applications/TokenMon.app`, identifier `com.modelmonitor.app`, version = MARKETING_VERSION; signed with Developer ID Installer if found |
| _zip | `ditto -c -k --keepParent dist/TokenMon.app dist/TokenMon-<ver>.zip` |
| _dsym | zips `TokenMon.app.dSYM` → `dist/TokenMon-<ver>-dSYM.zip` (fails if missing) |
| archive / export | xcarchive / Developer ID export via `Scripts/ExportOptions.plist` (`method developer-id`, `signingStyle automatic`) |
| notarize | requires dist app + Developer ID; `Scripts/notarize.sh` app (zip → `notarytool submit --wait --output-format json`, profile `AC_PASSWORD`; non-Accepted → print log, exit 1; staple + validate), rebuild zip and pkg from the stapled app, notarize pkg if Installer identity |
| test | `xcodebuild test -enableCodeCoverage YES` (Debug, ad-hoc) |
| test-core | `Scripts/run_core_tests.sh`: `swiftc` compile of all `TokenMon/Core/**/*.swift` + MonitorProvider, ColorPalette, ProviderColors, Grok models/parsers/daily builder/preview/export + `Tests/Manual/CoreTestsMain.swift`, target `<arch>-apple-macos14.0`, run binary; fails naming any missing listed file |
| lint / lint-fix | `swiftlint lint --strict --reporter github-actions-logging` (warnings = errors; `.swiftlint.yml`: line 160/200, type body 460/520, ...) |
| format / format-fix | `swiftformat . --lint` (`.swiftformat`, swiftversion 5.10, many rules disabled) |
| secrets | `gitleaks dir . --no-banner` + `gitleaks detect --no-banner --log-opts="--all"` |
| project | `xcodegen generate` |
| icon | `swift Scripts/generate_icon.swift <AppIcon.appiconset>` |
| check, open, clean (build/ .build), distclean (+ dist/) | toolchain info / open Xcode / cleanup |

### 6.3 CI (`.github/workflows/ci.yml`)

Triggers: push to `main`/`master`, any pull_request, workflow_dispatch, **workflow_call** (reused by release). Permissions `contents: read`.
Concurrency group `ci-<event>-<ref>`, cancel in progress only for pull requests. Every action pinned by full commit SHA; every
downloaded tool pinned by version and verified with SHA-256 (`shasum -a 256 -c` / `sha256sum -c`) and then version-checked.

| Job | Runner / timeout | Steps / gate |
|---|---|---|
| `static-checks` "Lint, format, and XcodeGen drift" | macos-15, 15 min | checkout (`actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1` v7.0.1, no persisted creds); install SwiftLint **0.59.1** (`portable_swiftlint.zip`, sha256 `58f9be8a4677900c945e2c618168223f4dd620a0cc65c9ccc5ea0f70433e89c1`), SwiftFormat **0.62.1** (`swiftformat.zip`, `7cb1cb1fae04932047c7015441c543848e8e60e1572d808d080e0a1f1661114a`), XcodeGen **2.46.0** (`xcodegen.zip`, `4d9e34b62172d645eed6457cac13fc222569974098ef4ee9c3368bedf0196806`) into /usr/local/bin, prepended to PATH; `make lint`; `make format`; `xcodegen generate` then fail if `git status --porcelain TokenMon.xcodeproj TokenMon/Resources/TokenMon.entitlements` is non-empty. |
| `secrets` "Secret scan (gitleaks)" | ubuntu-latest, 10 min | full-history checkout; gitleaks **8.30.1** linux x64 tarball (sha256 `551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb`); `gitleaks dir . --no-banner --redact -v`; `gitleaks detect --no-banner --redact -v --log-opts="--all"`. |
| `build-and-test` "Build, test, and Release build" | macos-26 (Xcode 26.x), 45 min | full-history checkout; show toolchain; `make test-core`; `make test`; (not on tags) convert coverage: newest `build/DerivedData/Logs/Test/*.xcresult` → `python3 -I Scripts/xccov_to_lcov.py <xcresult> $GITHUB_WORKSPACE > coverage.lcov`; upload with `codecov/codecov-action@303a32d7a59b442fa8d48b6a1cc6825c09c847a5` (v7.1.1), `files: coverage.lcov`, `disable_search: true`, token `CODECOV_TOKEN`, `fail_ci_if_error` only when the token is set; `make release` (proves packaging works). No caching. |

**Coverage**: `xccov_to_lcov.py` runs `xcrun xccov view --archive --json`, keeps files whose realpath is under `<root>/TokenMon/`,
emits `TN:`, `SF:<relative path>`, `DA:<line>,<count>` per executable line, `LF`, `LH`, `end_of_record`; exits 1 if no app file has data.
`codecov.yml`: `require_ci_to_pass: false`; project status informational; **patch target 50%**, only on PRs; ignores
`TokenMonTests/**`, `Tests/**`, `TokenMon/App/AppDelegate.swift`, `TokenMon/**/*View.swift`, `TokenMon/UI/MenuBar/CategoryRow.swift`,
`TokenMon/Providers/OpenCode/OpenCodeLimitBar.swift`, `TokenMon/Features/Overview/OverviewHourlyUsageChart.swift`.

**Secret scanning** (`.gitleaks.toml`): default ruleset + allowlist paths `build/`, `.build/`, `dist/`, `DerivedData/`; custom rules:
- `provider-session-cookie-header`: `(?i)\bCookie:\s*(?:[^"'\r\n]*;\s*)?(?:__Host-console_session|__Secure-next-auth\.session-token|WorkosCursorSessionToken|sessionKey|sso-rw|sso)=[A-Za-z0-9._%+/=-]{20,}`
- `provider-session-cookie-value`: `(?i)\b(__Host-console_session|__Secure-next-auth\.session-token|WorkosCursorSessionToken|sessionKey)=[A-Za-z0-9._%+/-]{20,}`
- `openrouter-api-key`: `\bsk-or-v1-[0-9a-f]{64}\b` (keyword `sk-or-v1-`).
Short test fixtures pass because of the length requirements.

### 6.4 Release (`.github/workflows/release.yml`)

Trigger: push of tag `v*`. Permissions `contents: read` (publish job `contents: write`). Concurrency `release-<ref>`, never cancelled.
1. `check-tag` (ubuntu, 5 min): tag without `v` must equal `MARKETING_VERSION` parsed from project.yml, else fail with "Bump project.yml, or tag v<version>."
2. `gates`: reuses `ci.yml` (all three jobs) on the tagged commit (coverage upload skipped on tags).
3. `publish` (macos-26, 45 min): full checkout; `make release` (ad-hoc signed; runner has no Developer ID, **not notarized**);
   assert `dist/TokenMon-<v>.zip`, `dist/TokenMon-<v>.pkg`, `dist/TokenMon-<v>-dSYM.zip` exist; if the GitHub release exists:
   refuse to overwrite same-named assets on run attempt 1, `--clobber` on attempt ≥ 2, otherwise upload; if it does not exist:
   `gh release create <tag> <files> --title <tag> --generate-notes --verify-tag`.

Artifacts per release: `TokenMon-<ver>.zip` (app, consumed by the updater), `TokenMon-<ver>.pkg` (fallback installer),
`TokenMon-<ver>-dSYM.zip` (symbols; ignored by the updater).

### 6.5 Update feed the in-app updater consumes (details in the Update slice)

- Endpoint: `GET https://api.github.com/repos/faulknerpearce/token_monitor/releases/latest`, every **6 h** while automatic checks are on, plus on demand.
- JSON fields read: `draft`, `prerelease` (either true → ignore), `tag_name` (fallback `name`) parsed as a version (`v` prefix, optional `Name-` prefix, `+build` stripped, `-pre` suffix), `html_url`, `published_at`, `assets[].name`, `assets[].browser_download_url`, `assets[].digest` (`sha256:<64 hex>`).
- Offered only when strictly newer than the running `CFBundleShortVersionString` and not a prerelease.
- Assets accepted only from `https://github.com/faulknerpearce/token_monitor/releases/download/<tag>/<file>` (exactly 7 path components, https, port 443, no credentials); redirects allowed to `objects.githubusercontent.com`, `release-assets.githubusercontent.com`.
- Asset choice per extension (`zip`, `pkg`): exclude names containing `dsym`; prefer names starting `tokenmon`.
- Download cap 80 MB, extracted cap 256 MB, symlinks refused; SHA-256 must equal the GitHub digest (no digest → open release page instead); signature/bundle-id/version/quarantine checks; atomic replace; relaunch.
- Windows implication: publish `TokenMon-<ver>-windows-x64.zip` (and/or `.msi`) on the same release; GitHub computes the asset `digest` automatically; the Windows updater should select by a Windows-specific suffix (the macOS rule "prefer `tokenmon*`.zip" would otherwise pick the mac zip), verify SHA-256 against `digest`, and verify the Authenticode signature (when signed) plus the PE version resource instead of codesign/bundle id.

### 6.6 Windows CI/release proposal (for parity of gates)

| macOS gate | Windows counterpart |
|---|---|
| SwiftLint strict | `cargo clippy --all-targets --all-features -- -D warnings` |
| SwiftFormat lint | `cargo fmt --all -- --check` |
| XcodeGen drift | `cargo metadata`/lockfile drift: `cargo build --locked` |
| gitleaks (same `.gitleaks.toml`) | unchanged job (repo-wide) |
| `make test` + xccov→LCOV + Codecov (patch 50%) | `cargo llvm-cov --lcov --output-path coverage.lcov` on `windows-latest`, same codecov action pin, keep `disable_search`, flag `windows` |
| `make release` dry run in CI | `cargo build --release --locked` + packaging script (zip; optional MSI via WiX/cargo-wix) |
| Pinned tools + SHA-256 | pin `dtolnay/rust-toolchain`/actions by SHA, `rust-toolchain.toml` with exact version, download tools (cargo-llvm-cov, wix) by version + SHA-256 |
| Tag == MARKETING_VERSION | tag == `Cargo.toml` `package.version` (or keep a single shared version file) |
| Build number = commit count | embed `git rev-list --count HEAD` as the 4th field of the PE `FileVersion` (e.g. `1.10.1.494`) via `winres`/`embed-resource` |
| ad-hoc signing | unsigned or Authenticode (`signtool sign /fd SHA256 /tr <timestamp> /td SHA256`); SmartScreen warns when unsigned |

---

## 7. Asset inventory

Asset catalog: `TokenMon/Resources/Assets.xcassets/`. "Template" = rendered as a single-color mask tinted at draw time.

| Asset (catalog name) | File | Format / intrinsic size | Template | Used for | Windows conversion |
|---|---|---|---|---|---|
| MenuBarIcon | MenuBarIcon.imageset/MenuBarIcon.pdf | PDF vector, 16×16 pt, 1270 B | yes | Grok logo (menu bar, switcher, headers) | Convert to SVG (e.g. `pdftocairo -svg`) and rasterize to alpha masks at 16/20/24/32/48 px for tray and UI; keep SVG for the panel |
| OpenCodeLogo | OpenCodeLogo.imageset/opencode-logo-light-square.svg | SVG 300×300, full-bleed square "O" | yes | OpenCode logo (menu bar box 13) | SVG usable directly (resvg); pre-render tray masks |
| CursorLogo | CursorLogo.imageset/cursor-logo.svg | SVG viewBox −8 −6 65 68 (padded cube) | yes | Cursor logo | SVG; pre-render masks |
| AnthropicLogo | AnthropicLogo.imageset/anthropic_small.svg | SVG 48×48 | yes | Claude logo; ModelCompany anthropic | SVG |
| OpenAILogo | OpenAILogo.imageset/openai_small.svg | SVG 48×48 | yes | ChatGPT logo; ModelCompany openai | SVG |
| OpenRouterLogo | OpenRouterLogo.imageset/openrouter.svg | SVG viewBox 0 0 24 24 | yes | OpenRouter logo | SVG |
| GrokbotLogo | GrokbotLogo.imageset/grokbot_small.svg | SVG 259×259 (viewBox −15 −15 259 259) | yes | Grokbot logo | SVG |
| TokenMonMark | TokenMonMark.imageset/TokenMonMark.png | PNG 1024×1024, 25 KB | yes | Overview header mark, selected-mode Overview menu-bar icon | Downscale to 16/20/24/32/48 px alpha masks; also usable as tray fallback icon |
| AlibabaLogo | AlibabaLogo.imageset/alibaba_small.svg | SVG 48×48 | no (full color) | ModelCompany qwen | SVG |
| DeepSeekLogo | DeepSeekLogo.imageset/deepseek_small.svg | SVG 48×48 | no | ModelCompany deepseek | SVG |
| MetaLogo | MetaLogo.imageset/meta_small.svg | SVG 48×48 | no | ModelCompany meta | SVG |
| MinimaxLogo | MinimaxLogo.imageset/minimax_small.svg | SVG 48×48 | no | ModelCompany minimax | SVG |
| NvidiaLogo | NvidiaLogo.imageset/nvidia_small.svg | SVG 48×48 | no | ModelCompany nvidia | SVG |
| ZaiLogo | ZaiLogo.imageset/zai_small.svg | SVG viewBox 0 0 30 30 (Illustrator export with `<style>` classes) | no | ModelCompany glm | SVG (verify the renderer supports CSS classes; resvg does) |
| KimiLogo | KimiLogo.imageset/kimi.png | PNG 96×96 | no | ModelCompany kimi | PNG as-is |
| AppIcon | AppIcon.appiconset/icon_16x16.png (16), icon_16x16@2x.png (32), icon_32x32.png (32), icon_32x32@2x.png (64), icon_128x128.png (128), icon_128x128@2x.png (256), icon_256x256.png (256), icon_256x256@2x.png (512), icon_512x512.png (512), icon_512x512@2x.png (1024) | opaque RGB PNGs, mascot on off-white full-bleed square | no | app icon | Build **`TokenMon.ico`** with 16, 20, 24, 32, 40, 48, 64, 256 px layers (from the 1024 source or `Scripts/app-icon-source.png`; 256 layer PNG-compressed); embed as the EXE icon resource and window icon. Consider transparent rounded variant since Windows does not mask icons. |

Other image files:

| Path | Format | Notes |
|---|---|---|
| `Scripts/app-icon-source.png` | PNG 1024×902 | Source for `generate_icon.swift`: draws mascot inside 10% padding on white, remaps luminance to off-white `(0.995,0.997,1.0)` ↔ near-black `(0.02,0.02,0.04)`, flattens to opaque RGB, writes the 10 PNGs above. Port as a script that also emits the .ico. |
| `Docs/tokenmon-logo.png` | PNG 1024×1024 | README logo only. |

Symbol-font icons (SF Symbols) used in this slice, need Windows equivalents (Segoe Fluent Icons / Segoe MDL2 or bundled SVGs):
`square.grid.2x2` (Overview header), `gearshape` (Settings), `chevron.down` (collapsible), `chevron.left`/`chevron.right` (paging),
`chevron.backward`/`chevron.forward`, `arrow.clockwise`, `xmark` (browser bar), `checkmark.circle.fill` (sign-in success, sign-out confirm),
`xmark.circle` (sign-out cancel), `person.crop.circle.badge.minus` (sign-out), `arrow.down.circle` (update), fallbacks `network`, `circle.dashed`.

Tray icons on Windows must be `.ico`/HICON: generate them at runtime (section 2.7) with `CreateIconIndirect` from a 32-bit premultiplied
BGRA bitmap at the size `GetSystemMetricsForDpi(SM_CXSMICON)` returns. Ship a static `tray-fallback.ico` (TokenMonMark, white and black
variants at 16/20/24/32) for startup before the first render.
