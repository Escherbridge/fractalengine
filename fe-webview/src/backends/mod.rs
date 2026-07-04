#[cfg(feature = "backend-servo")]
pub mod servo;

pub(crate) mod win32_popup;

#[cfg(feature = "backend-tauri")]
pub mod tauri;

pub mod stub;

// Type alias selects the active backend at compile time.

#[cfg(feature = "backend-servo")]
pub type ActiveBackend = servo::ServoBackend;

#[cfg(all(feature = "backend-tauri", not(feature = "backend-servo")))]
pub type ActiveBackend = tauri::TauriBackend;

#[cfg(not(any(feature = "backend-servo", feature = "backend-tauri")))]
pub type ActiveBackend = stub::StubBackend;
