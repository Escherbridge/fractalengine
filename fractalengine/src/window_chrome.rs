//! Frameless primary window + packaged app icon (taskbar/window icon and the
//! title bar's brand mark) — see src/AGENTS.md §window-chrome.

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::{Entity, Local, Query, With};
use bevy::window::{PrimaryWindow, Window, WindowResizeConstraints};

/// The app icon, embedded so a packaged build needs no asset lookup.
const ICON_PNG: &[u8] = include_bytes!("../assets/branding/icon.png");

/// The primary window: no OS decorations (fe-ui's topbar is the title bar),
/// app title for the taskbar/Alt-Tab, and a floor below which panels clip.
pub fn primary_window() -> Window {
    Window {
        title: "FractalEngine".to_string(),
        name: Some("fractalengine".to_string()),
        decorations: false,
        resize_constraints: WindowResizeConstraints {
            min_width: 960.0,
            min_height: 600.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The embedded icon as straight RGBA8 pixels plus its size.
pub fn decode_icon() -> Result<(Vec<u8>, u32, u32), String> {
    let image = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
        .map_err(|error| format!("embedded icon is not a PNG: {error}"))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Ok((image.into_raw(), width, height))
}

/// The topbar's brand mark (same pixels as the window icon).
pub fn brand_icon() -> Option<fe_ui::ui_shell::topbar::BrandIcon> {
    match decode_icon() {
        Ok((rgba, width, height)) => Some(fe_ui::ui_shell::topbar::BrandIcon {
            width: width as usize,
            height: height as usize,
            rgba,
        }),
        Err(error) => {
            tracing::warn!(%error, "no brand icon for the title bar");
            None
        }
    }
}

/// Bevy system: give the primary winit window the app icon once it exists
/// (winit creates windows after startup, so this retries until it lands).
/// Main-thread only — winit window calls must not leave the event-loop thread.
pub fn set_window_icon(
    mut applied: Local<bool>,
    windows: Query<Entity, With<PrimaryWindow>>,
    _main_thread: NonSendMarker,
) {
    if *applied {
        return;
    }
    let Ok(entity) = windows.single() else {
        return;
    };
    let icon = match decode_icon().and_then(|(rgba, width, height)| {
        winit::window::Icon::from_rgba(rgba, width, height).map_err(|error| error.to_string())
    }) {
        Ok(icon) => icon,
        Err(error) => {
            tracing::warn!(%error, "window icon unavailable");
            *applied = true;
            return;
        }
    };
    bevy::winit::WINIT_WINDOWS.with_borrow(|winit_windows| {
        if let Some(window) = winit_windows.get_window(entity) {
            window.set_window_icon(Some(icon));
            *applied = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_decodes_to_a_square_rgba_mark() {
        let (rgba, width, height) = decode_icon().expect("icon decodes");
        assert_eq!((width, height), (256, 256));
        assert_eq!(rgba.len(), 256 * 256 * 4);
        assert!(
            winit::window::Icon::from_rgba(rgba, width, height).is_ok(),
            "winit accepts the pixels as a window icon"
        );
    }
}
