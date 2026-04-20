#[cfg(feature = "backend-servo")]
pub mod servo;

#[cfg(feature = "backend-wry")]
pub mod wry;

pub mod stub;

// Type alias selects the active backend at compile time.
// Servo takes priority if both features are enabled.

#[cfg(feature = "backend-servo")]
pub type ActiveBackend = servo::ServoBackend;

#[cfg(all(feature = "backend-wry", not(feature = "backend-servo")))]
pub type ActiveBackend = wry::WryBackend;

#[cfg(not(any(feature = "backend-servo", feature = "backend-wry")))]
pub type ActiveBackend = stub::StubBackend;
