#[cfg(feature = "backend-servo")]
pub mod servo;

pub(crate) mod win32_popup;

#[cfg(feature = "backend-tauri")]
pub mod tauri;

#[cfg(all(windows, feature = "backend-tauri"))]
mod qa_navigation;

#[cfg(all(windows, feature = "backend-tauri"))]
mod fixture_surface;
#[cfg(all(windows, feature = "backend-tauri"))]
mod fixture_stream;

pub mod stub;

pub fn embedded_browsing_supported() -> bool {
    cfg!(all(
        feature = "backend-tauri",
        not(feature = "backend-servo")
    ))
}

// Type alias selects the active backend at compile time.

#[cfg(feature = "backend-servo")]
pub type ActiveBackend = servo::ServoBackend;

#[cfg(all(feature = "backend-tauri", not(feature = "backend-servo")))]
pub type ActiveBackend = tauri::TauriBackend;

/// External launcher only; it never reports an embedded page load.
#[cfg(not(any(feature = "backend-servo", feature = "backend-tauri")))]
pub type ActiveBackend = stub::StubBackend;
