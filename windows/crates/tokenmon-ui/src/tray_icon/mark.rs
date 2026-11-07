//! The TokenMon logo at tray-icon sizes, taken from the app icon.
//!
//! `tokenmon.ico` carries a hand-tuned 32-bit frame for every size Windows
//! asks for (16, 20, 24, 32, 40 px), so the tray shows the very same mark as
//! the exe and the macOS app instead of a drawn approximation (#81). The
//! frames are plain DIBs (`BITMAPINFOHEADER` + bottom-up BGRA), decoded here
//! to the straight RGBA that `tray_icon::Icon::from_rgba` expects.

#![allow(clippy::chunks_exact_to_as_chunks)] // as_chunks needs a newer MSRV than rust-version guarantees

/// The app icon (also embedded as the exe's icon resource by the app crate).
const APP_ICON: &[u8] = include_bytes!("../../../tokenmon-app/assets/tokenmon.ico");

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// The `size` x `size` frame of the `.ico` bytes `ico` as straight RGBA
/// (`size * size * 4` bytes, top row first), or `None` when the file has no
/// such 32-bit DIB frame or is malformed.
///
/// ```
/// use tokenmon_ui::tray_icon::decode_ico_frame;
/// assert!(decode_ico_frame(&[], 16).is_none());
/// ```
pub fn decode_ico_frame(ico: &[u8], size: u32) -> Option<Vec<u8>> {
    let count = usize::from(u16_at(ico, 4)?);
    let entry = (0..count).map(|i| 6 + 16 * i).find(|&at| {
        let w = u32::from(*ico.get(at).unwrap_or(&1));
        // 0 encodes 256.
        (if w == 0 { 256 } else { w }) == size
    })?;
    let len = usize::try_from(u32_at(ico, entry + 8)?).ok()?;
    let off = usize::try_from(u32_at(ico, entry + 12)?).ok()?;
    let dib = ico.get(off..off.checked_add(len)?)?;
    let header = usize::try_from(u32_at(dib, 0)?).ok()?;
    let (w, h2, bpp) = (u32_at(dib, 4)?, u32_at(dib, 8)?, u16_at(dib, 14)?);
    if bpp != 32 || w != size || h2 != size * 2 || header < 40 {
        return None;
    }
    let n = usize::try_from(size).ok()?;
    let pixels = dib.get(header..header + n * n * 4)?;
    let mut rgba = Vec::with_capacity(n * n * 4);
    for row in (0..n).rev() {
        for px in pixels[row * n * 4..(row + 1) * n * 4].chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    Some(rgba)
}

/// The TokenMon logo at `size` px as straight RGBA, from the app icon.
pub fn mark_rgba(size: u32) -> Option<Vec<u8>> {
    decode_ico_frame(APP_ICON, size)
}
