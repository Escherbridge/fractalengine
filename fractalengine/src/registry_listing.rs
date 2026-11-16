//! Registry search hits to bounded browse rows, shared by the Maps and Models bridges.

use fe_hexon::remote::RemoteHexonSummary;
use fe_ui::model_library::text::display_text;
use fe_ui::model_library::MAX_ATTRIBUTION_CHARS;
use fe_ui::terrain_map::dto::{MAX_REGISTRY_ROWS, MAX_REGISTRY_TEXT_CHARS};
use fe_ui::terrain_map::RegistryPackageRow;

/// The registry index's `hexon_type` spelling for map tilesets (`HexonType::TerrainTileset`, snake_case).
pub const REGISTRY_MAP_TYPE: &str = "terrain_tileset";
/// The registry index's `hexon_type` spelling for model packages (`HexonType::Model`).
pub const REGISTRY_MODEL_TYPE: &str = "model";

/// Longest version text kept from the network.
const MAX_REGISTRY_VERSION_LEN: usize = 64;

/// Hits of `hexon_type` whose id and version are installable, in registry order, row count capped.
pub fn registry_hits(hits: Vec<RemoteHexonSummary>, hexon_type: &str) -> Vec<RemoteHexonSummary> {
    hits.into_iter()
        .filter(|hit| {
            hit.hexon_type == hexon_type
                && fe_hexon::registry_source::valid_package_id(&hit.hexon_id)
                && hit.version.len() <= MAX_REGISTRY_VERSION_LEN
                && fe_hexon::registry_source::valid_version(&hit.version)
        })
        .take(MAX_REGISTRY_ROWS)
        .collect()
}

/// One hit as a browse row with its name and publisher clipped and its credit sanitised.
pub fn package_row(hit: &RemoteHexonSummary) -> RegistryPackageRow {
    RegistryPackageRow {
        hexon_id: hit.hexon_id.clone(),
        version: hit.version.clone(),
        name: clip(&hit.name),
        size_bytes: hit.size_bytes,
        publisher_did: clip(&hit.publisher_did),
        attribution: credit_text(hit.attribution.as_deref()),
    }
}

/// Display-safe credit text (control and bidi characters stripped, 512 chars); `None` when blank.
pub fn credit_text(attribution: Option<&str>) -> Option<String> {
    attribution
        .map(|text| display_text(text, MAX_ATTRIBUTION_CHARS))
        .filter(|text| !text.is_empty())
}

/// Bounded browse rows from a search response: one type only, ids and versions installable, text clipped, row count capped.
pub fn registry_rows(hits: Vec<RemoteHexonSummary>, hexon_type: &str) -> Vec<RegistryPackageRow> {
    registry_hits(hits, hexon_type).iter().map(package_row).collect()
}

fn clip(text: &str) -> String {
    text.chars().take(MAX_REGISTRY_TEXT_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(hexon_id: &str, version: &str, hexon_type: &str, name: &str) -> RemoteHexonSummary {
        serde_json::from_value(serde_json::json!({
            "hexon_id": hexon_id,
            "version": version,
            "hexon_type": hexon_type,
            "name": name,
            "publisher_did": "did:key:z6Mk",
            "size_bytes": 42,
        }))
        .expect("summary fixture parses")
    }

    #[test]
    fn registry_rows_table() {
        let long_name = "n".repeat(MAX_REGISTRY_TEXT_CHARS + 30);
        let long_version = format!("1.0.0-{}", "a".repeat(MAX_REGISTRY_VERSION_LEN));
        // (case, hits, type, expected ids in order, expected name length of the first row)
        let cases: Vec<(&str, Vec<RemoteHexonSummary>, &str, Vec<&str>, Option<usize>)> = vec![
            (
                "maps only for the map type",
                vec![
                    hit("city.map", "1.0.0", REGISTRY_MAP_TYPE, "City"),
                    hit("chairs", "1.0.0", REGISTRY_MODEL_TYPE, "Chairs"),
                ],
                REGISTRY_MAP_TYPE,
                vec!["city.map"],
                Some(4),
            ),
            (
                "models only for the model type",
                vec![
                    hit("city.map", "1.0.0", REGISTRY_MAP_TYPE, "City"),
                    hit("chairs", "1.0.0", REGISTRY_MODEL_TYPE, "Chairs"),
                ],
                REGISTRY_MODEL_TYPE,
                vec!["chairs"],
                Some(6),
            ),
            (
                "unsafe id, bad version and over-long version are dropped",
                vec![
                    hit("../escape", "1.0.0", REGISTRY_MAP_TYPE, "Bad id"),
                    hit("good", "not a version", REGISTRY_MAP_TYPE, "Bad version"),
                    hit("long", &long_version, REGISTRY_MAP_TYPE, "Long version"),
                    hit("kept", "2.1.0", REGISTRY_MAP_TYPE, "Kept"),
                ],
                REGISTRY_MAP_TYPE,
                vec!["kept"],
                Some(4),
            ),
            (
                "name clipped to the text cap",
                vec![hit("clipped", "1.0.0", REGISTRY_MAP_TYPE, &long_name)],
                REGISTRY_MAP_TYPE,
                vec!["clipped"],
                Some(MAX_REGISTRY_TEXT_CHARS),
            ),
            ("empty response", Vec::new(), REGISTRY_MAP_TYPE, Vec::new(), None),
        ];
        for (case, hits, hexon_type, expected_ids, first_name_len) in cases {
            let rows = registry_rows(hits, hexon_type);
            let ids: Vec<&str> = rows.iter().map(|row| row.hexon_id.as_str()).collect();
            assert_eq!(ids, expected_ids, "{case}");
            assert_eq!(rows.first().map(|row| row.name.chars().count()), first_name_len, "{case}");
        }

        let many: Vec<RemoteHexonSummary> = (0..MAX_REGISTRY_ROWS + 5)
            .map(|index| hit(&format!("map{index}"), "1.0.0", REGISTRY_MAP_TYPE, "Map"))
            .collect();
        assert_eq!(registry_rows(many, REGISTRY_MAP_TYPE).len(), MAX_REGISTRY_ROWS, "row count capped");

        // Map credit line: control and bidi characters stripped, blank credit dropped.
        let mut credited = hit("credited", "1.0.0", REGISTRY_MAP_TYPE, "Credited");
        credited.attribution = Some("Imagery \u{202E}by\nA. Maker".into());
        let mut blank = hit("blank", "1.0.0", REGISTRY_MAP_TYPE, "Blank");
        blank.attribution = Some(" \n ".into());
        let credits: Vec<Option<String>> =
            registry_rows(vec![credited, blank], REGISTRY_MAP_TYPE).into_iter().map(|row| row.attribution).collect();
        assert_eq!(credits, vec![Some("Imagery by A. Maker".to_string()), None], "credit sanitised");
    }
}
