//! F16/A29 V1 — install verification for the two F15-produced `.hexon`
//! tilesets (switzerland-zurich-alps, japan-mount-fuji). The archives live in
//! the sibling `gis-tile-etl` repo's `dist/` directory, not in this repo, so
//! this test is `#[ignore]`d by default (DEC-C22 FIX L: a workspace sweep
//! without `FE_GIS_DIST_DIR` must show the test did not run, not a silent
//! green skip) and requires an explicit `--ignored` invocation. Once run,
//! a missing/unset `FE_GIS_DIST_DIR` or missing archive is a loud panic, not
//! a skip — misconfiguration should fail, not pass quietly.
//!
//! Run:
//!   FE_GIS_DIST_DIR="C:\Users\<you>\...\gis-tile-etl\dist" \
//!   cargo test -p fe-terrain --test gis_hexon_install_test -- --ignored --nocapture

use fe_terrain::tiles::HexonStore;

/// One region's expected on-disk meta (pinned 2026-10-09 via
/// `gis-tile-etl verify dist/<file>`), plus an INDEPENDENTLY-derived expected
/// ground-sample-distance so this test does not simply re-call the function
/// it is checking (`fe_format::manifest::derive_scale_from_bounds`).
///
/// GSD math (standard Web-Mercator tile resolution formula):
///   resolution_m_per_px = (equatorial_circumference_m / tile_size_px)
///                         * cos(center_lat) / 2^max_zoom
/// equatorial_circumference_m = 40_075_016.686, tile_size_px = 256.
struct Region {
    hexon_id: &'static str,
    file_name: &'static str,
    bounds: [f64; 4],
    zoom_range: (u8, u8),
    tile_count: u32,
    expected_gsd_m: f64,
}

const REGIONS: &[Region] = &[
    Region {
        hexon_id: "tileset-switzerland-zurich-alps",
        file_name: "switzerland-zurich-alps.hexon",
        bounds: [47.2, 8.3, 47.5, 8.8],
        zoom_range: (8, 10),
        tile_count: 10,
        // center_lat = (47.2+47.5)/2 = 47.35; cos(47.35 deg) = 0.677654
        // (40_075_016.686 / 256) * 0.677654 / 2^10 = 103.57 m/px
        expected_gsd_m: 103.57,
    },
    Region {
        hexon_id: "tileset-japan-mount-fuji",
        file_name: "japan-mount-fuji.hexon",
        bounds: [35.25, 138.6, 35.5, 138.9],
        zoom_range: (8, 10),
        tile_count: 8,
        // center_lat = (35.25+35.5)/2 = 35.375; cos(35.375 deg) = 0.815815
        // (40_075_016.686 / 256) * 0.815815 / 2^10 = 124.65 m/px
        expected_gsd_m: 124.65,
    },
];

fn temp_store_dir(label: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("fe_gis_hexon_install_test_{label}_{ts}"))
}

#[test]
#[ignore = "requires FE_GIS_DIST_DIR pointing at gis-tile-etl/dist"]
fn install_f15_hexons_backfills_correct_scale_and_meta() {
    let dist_dir = std::env::var("FE_GIS_DIST_DIR").expect(
        "FE_GIS_DIST_DIR must be set when running this #[ignore]'d test -- \
         point it at gis-tile-etl's dist/ directory (sibling repo); \
         misconfiguration should fail loudly, not skip quietly",
    );
    let dist_dir = std::path::PathBuf::from(dist_dir);

    let store = HexonStore::with_dir(temp_store_dir("store")).expect("create hexon store");

    for region in REGIONS {
        let path = dist_dir.join(region.file_name);
        assert!(
            path.is_file(),
            "missing {} -- is FE_GIS_DIST_DIR pointed at gis-tile-etl/dist?",
            path.display()
        );
        let bytes = std::fs::read(&path).expect("read hexon archive bytes");

        // Install twice: `install_tileset` (and the `install_sample_hexons`
        // CLI that wraps it) must be idempotent -- a second install REPLACES
        // the registry entry rather than erroring.
        let first = store.install_tileset(&bytes).expect("first install");
        let second = store
            .install_tileset(&bytes)
            .expect("second install (refresh)");

        assert_eq!(first.hexon_id, region.hexon_id);
        assert_eq!(second.hexon_id, region.hexon_id);
        assert_eq!(first.bounds, region.bounds, "{}: bounds", region.hexon_id);
        assert_eq!(
            first.zoom_range, region.zoom_range,
            "{}: zoom_range",
            region.hexon_id
        );
        assert_eq!(
            first.tile_count, region.tile_count,
            "{}: elevation tile_count",
            region.hexon_id
        );
    }

    let installed = store.list_installed();
    assert_eq!(
        installed.len(),
        REGIONS.len(),
        "exactly the two F15 tilesets are registered, no duplicates from the refresh pass"
    );
    for region in REGIONS {
        assert!(
            installed.iter().any(|t| t.hexon_id == region.hexon_id),
            "{} present in registry",
            region.hexon_id
        );
    }

    for region in REGIONS {
        let source = store
            .load_tileset(region.hexon_id)
            .expect("load_tileset (exercises backfill_scale_fields)");
        let meta = &source.tileset_meta;

        assert!(meta.has_satellite, "{}: has_satellite", region.hexon_id);
        assert_eq!(
            meta.satellite_tile_count, region.tile_count,
            "{}: satellite_tile_count",
            region.hexon_id
        );

        let gsd = meta.ground_sample_distance_m.unwrap_or_else(|| {
            panic!(
                "{}: ground_sample_distance_m not backfilled",
                region.hexon_id
            )
        });
        let native_scale = meta
            .native_scale
            .unwrap_or_else(|| panic!("{}: native_scale not backfilled", region.hexon_id));

        // 5% tolerance: generous enough to absorb the independent
        // calculator's own rounding, tight enough that a real regression
        // (wrong zoom, wrong tile_size, wrong hemisphere) fails loudly.
        let tol = region.expected_gsd_m * 0.05;
        assert!(
            (gsd - region.expected_gsd_m).abs() < tol,
            "{}: backfilled GSD {gsd:.3} m/px not within {tol:.3} of independently \
             computed expected {:.3} m/px",
            region.hexon_id,
            region.expected_gsd_m
        );

        let expected_native_scale = 1.0 / region.expected_gsd_m;
        let scale_tol = expected_native_scale * 0.05;
        assert!(
            (native_scale - expected_native_scale).abs() < scale_tol,
            "{}: backfilled native_scale {native_scale:.6} not within {scale_tol:.6} \
             of independently computed expected {:.6}",
            region.hexon_id,
            expected_native_scale
        );

        println!(
            "{}: gsd={gsd:.2} m/px (expected ~{:.2}), native_scale={native_scale:.6} \
             (expected ~{expected_native_scale:.6})",
            region.hexon_id, region.expected_gsd_m
        );
    }
}
