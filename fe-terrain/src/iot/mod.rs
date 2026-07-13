//! IoT path tracking and animation.

pub mod path_tracker;
pub mod animation;

pub use path_tracker::{PathTracker, SnapResult};
pub use animation::TrackAnimator;
pub use animation::{parse_track_color_hex, track_color_to_hex, TrackStyle};

#[cfg(feature = "render")]
pub use animation::{TrackRouteMap, TrackStyleMap};
