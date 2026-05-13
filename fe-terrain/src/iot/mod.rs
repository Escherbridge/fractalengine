//! IoT path tracking and animation.

pub mod path_tracker;
pub mod animation;

pub use path_tracker::{PathTracker, SnapResult};
pub use animation::TrackAnimator;

#[cfg(feature = "render")]
pub use animation::TrackRouteMap;
