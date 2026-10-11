//! Persisted node visibility view (`view.state` / `view.groups`,
//! hierarchy_visibility_groups Phase 3). Authorization lives in the dispatch
//! arm; see AGENTS.md §lifecycle "Node view".

use tracing::instrument;

use fe_runtime::messages::NodeViewDto;

use crate::handlers::entity_property::{
    delete_entity_property_handler, get_entity_properties_handler, set_entity_property_handler,
};
use crate::repo::Db;

/// Node property holding the tri-state override (`"show"`/`"hide"`; absent = auto).
pub const VIEW_STATE_KEY: &str = "view.state";
/// Node property holding the node's Group ids (absent = no groups).
pub const VIEW_GROUPS_KEY: &str = "view.groups";
/// Upper bound on Group ids per node.
pub const MAX_VIEW_GROUPS: usize = 64;
/// Upper bound on one Group id's length (chars).
pub const MAX_VIEW_GROUP_ID_LEN: usize = 64;

/// Rejects an unknown state or malformed group list before any auth or write. Pure.
pub(crate) fn validate_view_request(
    state: Option<&str>,
    groups: Option<&[String]>,
) -> Result<(), String> {
    if let Some(state) = state {
        if !matches!(state, "auto" | "show" | "hide") {
            return Err(format!(
                "invalid view state {state:?} (expected auto|show|hide)"
            ));
        }
    }
    if let Some(groups) = groups {
        if groups.len() > MAX_VIEW_GROUPS {
            return Err(format!(
                "too many groups ({} > {MAX_VIEW_GROUPS})",
                groups.len()
            ));
        }
        if let Some(bad) = groups
            .iter()
            .find(|g| g.trim().is_empty() || g.chars().count() > MAX_VIEW_GROUP_ID_LEN)
        {
            return Err(format!(
                "invalid group id {bad:?} (must be non-empty, at most {MAX_VIEW_GROUP_ID_LEN} chars)"
            ));
        }
    }
    Ok(())
}

/// Owning petal id of a LIVE (non-tombstoned) node, or `None` when the node is
/// missing or tombstoned — the tombstone-aware first step of the lifecycle
/// authorization order.
#[instrument(skip(db))]
pub(crate) async fn resolve_live_node_petal(
    db: &Db,
    node_id: &str,
) -> anyhow::Result<Option<String>> {
    let mut res = db
        .query("SELECT petal_id FROM node WHERE node_id = $nid AND tombstone = NONE LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("SetNodeView live-node lookup failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows
        .first()
        .and_then(|row| row["petal_id"].as_str())
        .map(str::to_owned))
}

/// The persisted view read out of a node's `properties` bag. Pure.
pub(crate) fn view_from_properties(node_id: &str, properties: &serde_json::Value) -> NodeViewDto {
    NodeViewDto {
        node_id: node_id.to_string(),
        state: properties
            .get(VIEW_STATE_KEY)
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        groups: properties
            .get(VIEW_GROUPS_KEY)
            .and_then(|v| v.as_array())
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().map(str::to_owned))
                    .collect()
            }),
    }
}

/// Write the requested view keys (each through the `PropertySet` /
/// `PropertyDeleted` op-log path `SetNodeProperty` uses), then read the row
/// back. `"auto"` and an empty group list REMOVE their key (absent = default).
/// Caller has already validated and authorized.
#[instrument(skip(db))]
pub(crate) async fn set_node_view_handler(
    db: &Db,
    node_id: &str,
    state: Option<&str>,
    groups: Option<&[String]>,
) -> anyhow::Result<NodeViewDto> {
    match state {
        Some("auto") => delete_entity_property_handler(db, node_id, VIEW_STATE_KEY).await?,
        Some(state) => {
            set_entity_property_handler(db, node_id, VIEW_STATE_KEY, &serde_json::json!(state))
                .await?
        }
        None => {}
    }
    match groups {
        Some([]) => delete_entity_property_handler(db, node_id, VIEW_GROUPS_KEY).await?,
        Some(groups) => {
            set_entity_property_handler(db, node_id, VIEW_GROUPS_KEY, &serde_json::json!(groups))
                .await?
        }
        None => {}
    }
    let properties = get_entity_properties_handler(db, node_id).await?;
    Ok(view_from_properties(node_id, &properties))
}

/// Every live node in `petal_id` that carries a persisted view property.
#[instrument(skip(db))]
pub(crate) async fn load_petal_view_state_handler(
    db: &Db,
    petal_id: &str,
) -> anyhow::Result<Vec<NodeViewDto>> {
    let mut res = db
        .query(
            "SELECT node_id, properties[$state_key] AS state, properties[$groups_key] AS groups \
             FROM node WHERE petal_id = $pid AND tombstone = NONE AND (properties[$state_key] != NONE OR properties[$groups_key] != NONE)",
        )
        .bind(("pid", petal_id.to_string()))
        .bind(("state_key", VIEW_STATE_KEY.to_string()))
        .bind(("groups_key", VIEW_GROUPS_KEY.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("LoadPetalViewState query failed: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("LoadPetalViewState statement failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res
        .take(0)
        .map_err(|e| anyhow::anyhow!("LoadPetalViewState take failed: {e}"))?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let node_id = row["node_id"].as_str()?;
            // Only the two view keys are projected (a track's bag also carries
            // its whole `gpx_points` array); re-key them for the shared reader.
            let bag = serde_json::json!({
                VIEW_STATE_KEY: row["state"],
                VIEW_GROUPS_KEY: row["groups"],
            });
            Some(view_from_properties(node_id, &bag))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_view_request_accepts_the_three_states_and_sane_groups() {
        for state in ["auto", "show", "hide"] {
            assert_eq!(validate_view_request(Some(state), None), Ok(()));
        }
        assert_eq!(
            validate_view_request(None, Some(&["g1".to_string(), "g2".to_string()])),
            Ok(())
        );
        assert_eq!(validate_view_request(None, Some(&[])), Ok(()));
    }

    #[test]
    fn validate_view_request_rejects_bad_input() {
        let too_long = "x".repeat(MAX_VIEW_GROUP_ID_LEN + 1);
        let too_many: Vec<String> = (0..=MAX_VIEW_GROUPS).map(|i| format!("g{i}")).collect();
        for (state, groups) in [
            (Some("hidden"), None),
            (Some(""), None),
            (None, Some(vec!["  ".to_string()])),
            (None, Some(vec![too_long])),
            (None, Some(too_many)),
        ] {
            assert!(
                validate_view_request(state, groups.as_deref()).is_err(),
                "{state:?} / {groups:?} must be rejected"
            );
        }
    }

    #[test]
    fn view_from_properties_reads_both_keys_and_treats_absence_as_none() {
        let bag = serde_json::json!({
            "view.state": "hide",
            "view.groups": ["g1", "g2"],
            "gis.track.name": "unrelated"
        });
        let view = view_from_properties("n1", &bag);
        assert_eq!(view.state.as_deref(), Some("hide"));
        assert_eq!(view.groups, Some(vec!["g1".to_string(), "g2".to_string()]));
        let empty = view_from_properties("n1", &serde_json::json!({}));
        assert_eq!((empty.state, empty.groups), (None, None));
    }
}
