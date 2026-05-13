//! Layer compositing and rendering support.
//!
//! This module is only available when the `render` feature is enabled.

pub mod stack;
pub mod style;
pub mod geojson;

pub use stack::{LayerId, LayerStack, LayerType, MapLayer};
pub use style::{compute_vertex_colors, ColorMode, TrackPoint, viridis};
pub use geojson::{parse_geojson, GeoJsonResult, MarkerInstance, PolygonMesh, PolylineMesh};
