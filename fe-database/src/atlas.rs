use serde::{Deserialize, Serialize};

/// Visibility level for a petal (space).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    #[default]
    Private,
    Unlisted,
}

/// Metadata associated with a petal (space).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PetalMetadata {
    pub visibility: Visibility,
    pub tags: Vec<String>,
    pub description: Option<String>,
}

/// Axis-aligned bounding box for a room, in local space units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RoomBounds {
    /// Minimum corner [x, y, z].
    pub min: [f32; 3],
    /// Maximum corner [x, y, z].
    pub max: [f32; 3],
}

/// Default spawn point inside a room.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SpawnPoint {
    /// World-space position [x, y, z].
    pub position: [f32; 3],
    /// Initial yaw angle in degrees (-180 to 180).
    pub yaw: f32,
}

/// Metadata associated with a room.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RoomMetadata {
    pub description: Option<String>,
    pub bounds: Option<RoomBounds>,
    pub spawn_point: Option<SpawnPoint>,
}

/// A partial update for model metadata — all fields optional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelMetadataUpdate {
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub external_url: Option<String>,
    pub config_url: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Aggregate summary of the entire space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SpaceOverview {
    pub petal_count: u64,
    pub room_count: u64,
    pub model_count: u64,
    pub peer_count: u64,
    pub estimated_storage_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&Visibility::Public).unwrap(),
            "\"public\""
        );
        assert_eq!(
            serde_json::to_string(&Visibility::Private).unwrap(),
            "\"private\""
        );
        assert_eq!(
            serde_json::to_string(&Visibility::Unlisted).unwrap(),
            "\"unlisted\""
        );
    }

    #[test]
    fn visibility_deserializes_from_lowercase() {
        let v: Visibility = serde_json::from_str("\"unlisted\"").unwrap();
        assert!(matches!(v, Visibility::Unlisted));
    }

    #[test]
    fn petal_metadata_default_is_private_empty() {
        let m = PetalMetadata::default();
        assert!(matches!(m.visibility, Visibility::Private));
        assert!(m.tags.is_empty());
        assert!(m.description.is_none());
    }

    #[test]
    fn model_metadata_update_default_is_all_none() {
        let m = ModelMetadataUpdate::default();
        assert!(m.display_name.is_none());
        assert!(m.tags.is_empty());
        assert!(m.metadata.is_none());
    }

    #[test]
    fn space_overview_default_is_zeroes() {
        let o = SpaceOverview::default();
        assert_eq!(o.petal_count, 0);
        assert_eq!(o.peer_count, 0);
    }
}
