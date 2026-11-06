//! Week paging state for the navigable daily charts (`WeekOffset`).

/// Offset of the displayed week: `0` is the window containing today,
/// negative values are past weeks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct WeekOffset(pub i32);

impl WeekOffset {
    /// One week earlier (unbounded).
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_ui::kit::week::WeekOffset;
    /// assert_eq!(WeekOffset(0).previous(), WeekOffset(-1));
    /// assert_eq!(WeekOffset(-2).previous(), WeekOffset(-3));
    /// ```
    #[must_use]
    pub const fn previous(self) -> Self {
        Self(self.0 - 1)
    }

    /// One week later, never past the current week.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_ui::kit::week::WeekOffset;
    /// assert_eq!(WeekOffset(0).next(), WeekOffset(0));
    /// assert_eq!(WeekOffset(-4).next(), WeekOffset(-3));
    /// ```
    #[must_use]
    pub const fn next(self) -> Self {
        Self(if self.0 + 1 < 0 { self.0 + 1 } else { 0 })
    }

    /// Whether the right chevron is enabled.
    pub const fn can_go_next(self) -> bool {
        self.0 < 0
    }

    /// Past weeks hide the live pace footer and reset info.
    pub const fn shows_live_footer(self) -> bool {
        self.0 == 0
    }
}
