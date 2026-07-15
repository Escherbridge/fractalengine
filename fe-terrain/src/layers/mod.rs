//! Layer compositing and rendering support.
//!
//! This module is only available when the `render` feature is enabled.

pub mod geojson;
pub mod stack;
pub mod style;

pub use geojson::{parse_geojson, GeoJsonResult, MarkerInstance, PolygonMesh, PolylineMesh};
pub use stack::{layer_type_from_config_name, LayerId, LayerStack, LayerType, MapLayer};
pub use style::{compute_vertex_colors, viridis, ColorMode, TrackPoint};
