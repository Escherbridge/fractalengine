//! # fe-query
//!
//! LINQ-style query builder for SurrealQL with parameterized queries,
//! GIS validation, and schema DDL generation.
//!
//! This crate is intentionally independent of `fe-database` — it produces
//! parameterized query strings and bind values that any SurrealDB client
//! can execute.
//!
//! ## Features
//!
//! - `geo` (default): GIS coordinate validation for EPSG:4326, EPSG:3857,
//!   and local Cartesian coordinate systems.
//! - `graphql`: async-graphql integration (not yet implemented).

pub mod builder;
pub mod geo;
pub mod gis;

#[cfg(feature = "graphql")]
pub mod graphql;

#[cfg(any(feature = "datafusion", feature = "parquet"))]
pub mod columnar;

pub mod duckdb_compat;

// Re-export key types at crate root for ergonomic imports.
pub use builder::filter::Filter;
pub use builder::render::{BuiltQuery, ReturnClause, SortDir};
pub use builder::schema::{
    FieldSchema, IndexDef, SchemaObject, TableDef,
    FractalTable, NodeLogTable, NodeTable, PetalTable, VerseTable,
};
pub use builder::value::QueryValue;
pub use builder::{DeleteBuilder, InsertBuilder, QueryBuilder, UpdateBuilder};

pub use geo::{CRS, GeoPoint, GeoPolygon};
pub use geo::validate::GeoValidationError;
