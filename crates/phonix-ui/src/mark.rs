//! The Phonix Audio mark.
//!
//! One image for everything this family ships: the sequencer's windows, a
//! plugin's editor when it opens one of its own, and the standalone host.
//! They are one product, and a taskbar full of them should say so.
//!
//! The sizes are here rather than in any one application because an
//! installer has to write every one of them into the icon theme: a desktop
//! picks the size it wants from what it finds, and picks badly when it has to
//! scale the only one it was given.

use std::sync::{Arc, OnceLock};

/// The mark at every size a desktop asks for, smallest first.
///
/// An installer writes each of these into `hicolor/<n>x<n>/apps/`, under the
/// name its desktop entry's `Icon=` line gives.
pub const SIZES: &[(u32, &[u8])] = &[
    (16, include_bytes!("../assets/icons/phonix-mark-16.png")),
    (32, include_bytes!("../assets/icons/phonix-mark-32.png")),
    (48, include_bytes!("../assets/icons/phonix-mark-48.png")),
    (64, include_bytes!("../assets/icons/phonix-mark-64.png")),
    (128, include_bytes!("../assets/icons/phonix-mark-128.png")),
    (256, include_bytes!("../assets/icons/phonix-mark-256.png")),
    (512, include_bytes!("../assets/icons/phonix-mark-512.png")),
];

/// The name every desktop entry in this family gives its icon, and so the
/// file name an installer writes the sizes under.
pub const ICON_NAME: &str = "phonix";

/// The mark, decoded once, for a window to wear.
///
/// X11 takes it from here. Wayland does not: there a window's icon comes
/// from the desktop entry whose name matches the window's app id, so an
/// application that wants one on that platform has to be installed, not
/// merely run.
pub fn mark() -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| {
        let png = include_bytes!("../assets/icons/phonix-mark-256.png");
        let img = image::load_from_memory(png)
            .expect("the bundled mark is a PNG")
            .into_rgba8();
        let (width, height) = img.dimensions();
        Arc::new(egui::IconData { rgba: img.into_raw(), width, height })
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every size an installer will write is a PNG of that size.
    #[test]
    fn the_mark_is_square_at_every_size_it_ships() {
        for (n, png) in SIZES {
            let img = image::load_from_memory(png).expect("a PNG");
            assert_eq!((img.width(), img.height()), (*n, *n), "{n} px");
        }
    }

    /// The one a window wears decodes to pixels with something in them.
    #[test]
    fn the_window_icon_is_not_blank() {
        let icon = mark();
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
        assert!(icon.rgba.chunks(4).any(|p| p[3] > 0), "every pixel is transparent");
    }
}
