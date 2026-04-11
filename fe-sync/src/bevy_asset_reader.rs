//! Re-export of `BlobAssetReader` from `fe-runtime`.
//!
//! The implementation lives in `fe-runtime::bevy_blob_reader` so that
//! `build_app` (also in `fe-runtime`) can register the asset source before
//! `DefaultPlugins` without a circular dependency.

pub use fe_runtime::bevy_blob_reader::BlobAssetReader;
