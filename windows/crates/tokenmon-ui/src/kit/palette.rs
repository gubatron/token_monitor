//! Palette helpers: product colors and `Srgb` to Slint conversions.

use slint::Color;
use tokenmon_core::Srgb;

/// Converts an [`Srgb`] to a Slint color, rounding to 8 bits per channel and
/// keeping the alpha.
///
/// # Examples
///
/// ```
/// use tokenmon_core::ProviderId;
/// use tokenmon_ui::kit::palette::to_color;
/// let c = to_color(ProviderId::Grok.accent());
/// assert_eq!((c.red(), c.green(), c.blue(), c.alpha()), (28, 97, 209, 255));
/// ```
pub fn to_color(c: Srgb) -> Color {
    let (r, g, b) = c.to_rgb8();
    Color::from_argb_u8((c.alpha.clamp(0.0, 1.0) * 255.0).round() as u8, r, g, b)
}

/// Grok product palette (`ProductColor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProductColor {
    /// Chat (= Grok accent).
    Chat,
    /// Grok Build.
    Build,
    /// Voice.
    Voice,
    /// API.
    Api,
    /// Imagine / media.
    Imagine,
    /// Everything else.
    Other,
}

impl ProductColor {
    /// Maps a product id (case-insensitive) to its color slot.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_ui::kit::palette::ProductColor;
    /// assert_eq!(ProductColor::from_product_id("Grok Build"), ProductColor::Build);
    /// assert_eq!(ProductColor::from_product_id("video"), ProductColor::Imagine);
    /// assert_eq!(ProductColor::from_product_id("zzz"), ProductColor::Other);
    /// ```
    pub fn from_product_id(id: &str) -> Self {
        match id.to_lowercase().as_str() {
            "build" | "grok_build" | "grok-build" | "grok build" => Self::Build,
            "api" => Self::Api,
            "chat" | "grok" | "text" => Self::Chat,
            "imagine" | "image" | "video" | "media" => Self::Imagine,
            "voice" => Self::Voice,
            _ => Self::Other,
        }
    }

    /// The exact sRGB floats from the spec.
    pub const fn srgb(self) -> Srgb {
        match self {
            Self::Chat => Srgb::rgb(0.11, 0.38, 0.82),
            Self::Build => Srgb::rgb(0.55, 0.78, 1.00),
            Self::Voice => Srgb::rgb(0.40, 0.55, 0.82),
            Self::Api => Srgb::rgb(0.22, 0.32, 0.48),
            Self::Imagine => Srgb::rgb(0.75, 0.90, 1.00),
            Self::Other => Srgb::rgb(0.45, 0.45, 0.45),
        }
    }
}

/// `ModelPalette.purple` (OpenCode Zen / OpenCode monthly bar in Overview).
pub const MODEL_PURPLE: Srgb = Srgb::rgb(0.58, 0.44, 0.86);
