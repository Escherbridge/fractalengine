//! IoT path tracking and animation.

pub mod animation;
pub mod path_tracker;

pub use animation::TrackAnimator;
pub use animation::{parse_track_color_hex, track_color_to_hex, TrackStyle};
pub use path_tracker::{PathTracker, SnapResult};

#[cfg(feature = "render")]
pub use animation::{TrackRouteMap, TrackStyleMap};
