//! Provider switcher grid (`ProviderSwitcherView`, spec 04 section 1.5).

use tokenmon_core::ProviderId;

/// Maximum tabs per row.
pub const MAX_PER_ROW: usize = 4;
/// Row height in px.
pub const ROW_HEIGHT: f64 = 28.0;
/// Grid width (420 - 2*12).
pub const GRID_WIDTH: f64 = 396.0;

/// A real tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabVm {
    /// Provider (or Overview).
    pub provider: ProviderId,
    /// Label (`All` for Overview).
    pub label: &'static str,
    /// Selected tab.
    pub selected: bool,
    /// Logo asset key (`None` for Overview).
    pub logo: Option<&'static str>,
}

/// One cell of the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellVm {
    /// Tab, or `None` for padding.
    pub tab: Option<TabVm>,
    /// 1pt divider at the leading edge (cell index > 0 and previous cell is real).
    pub leading_divider: bool,
}

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowVm {
    /// Always `MAX_PER_ROW` cells.
    pub cells: Vec<CellVm>,
    /// Horizontal divider across the top (every row but the first).
    pub top_divider: bool,
}

/// `rows(for:)`: chunks of 4, last row padded with `None`.
///
/// ```
/// use tokenmon_ui::panels::switcher::rows;
/// let r = rows(&[1, 2, 3, 4, 5]);
/// assert_eq!(r.len(), 2);
/// assert_eq!(r[1], vec![Some(5), None, None, None]);
/// ```
pub fn rows<T: Clone>(items: &[T]) -> Vec<Vec<Option<T>>> {
    items
        .chunks(MAX_PER_ROW)
        .map(|c| {
            let mut row: Vec<Option<T>> = c.iter().cloned().map(Some).collect();
            row.resize(MAX_PER_ROW, None);
            row
        })
        .collect()
}

/// Provider behind the cell at `(row, col)`, or `None` for padding cells and
/// out-of-range coordinates. Resolves the `select-tab(row, col)` callback.
///
/// ```
/// use tokenmon_core::ProviderId;
/// use tokenmon_ui::panels::switcher::{build_switcher, tab_provider};
/// let rows = build_switcher(&[ProviderId::Grok], ProviderId::Overview);
/// assert_eq!(tab_provider(&rows, 0, 1), Some(ProviderId::Grok));
/// assert_eq!(tab_provider(&rows, 0, 3), None);
/// assert_eq!(tab_provider(&rows, 4, 0), None);
/// ```
pub fn tab_provider(rows: &[RowVm], row: usize, col: usize) -> Option<ProviderId> {
    rows.get(row)?
        .cells
        .get(col)?
        .tab
        .as_ref()
        .map(|t| t.provider)
}

/// Builds the grid for `[overview] + visible` with `selected` highlighted.
pub fn build_switcher(visible: &[ProviderId], selected: ProviderId) -> Vec<RowVm> {
    let tabs: Vec<TabVm> = std::iter::once(ProviderId::Overview)
        .chain(visible.iter().copied())
        .map(|p| TabVm {
            provider: p,
            label: p.switcher_label(),
            selected: p == selected,
            logo: p.logo_asset_key(),
        })
        .collect();
    rows(&tabs)
        .into_iter()
        .enumerate()
        .map(|(ri, row)| {
            let cells = (0..row.len())
                .map(|i| CellVm {
                    tab: row[i].clone(),
                    leading_divider: i > 0 && row[i - 1].is_some(),
                })
                .collect();
            RowVm {
                cells,
                top_divider: ri > 0,
            }
        })
        .collect()
}
