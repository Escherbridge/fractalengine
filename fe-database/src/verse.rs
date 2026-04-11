use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VerseMember {
    pub member_id: String, // ULID
    pub verse_id: String,
    pub peer_did: String, // did:key
    pub status: MemberStatus,
    pub invited_by: String,       // did:key of inviter
    pub invite_sig: String,       // hex-encoded Ed25519 signature
    pub invite_timestamp: String, // ISO8601
    pub revoked_at: Option<String>,
    pub revoked_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MemberStatus {
    #[default]
    Active,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VerseOverview {
    pub verse_id: String,
    pub name: String,
    pub member_count: u64,
    pub fractal_count: u64,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FractalMetadata {
    pub fractal_id: String,
    pub verse_id: String,
    pub owner_did: String,
    pub name: String,
    pub description: Option<String>,
    pub petal_count: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct NodeMetadata {
    pub node_id: String,
    pub petal_id: String,
    pub display_name: Option<String>,
    /// ULID referencing the `asset` table. Named `asset_id` to match the DB column.
    pub asset_id: Option<String>,
    /// XZ-plane position as a GeoJSON Point [longitude=X, latitude=Z].
    /// Matches the `geometry<point>` DB column and enables spatial queries.
    pub position: [f32; 2],
    /// Y-axis height in metres. Stored separately from the 2-D `position` field.
    pub elevation: f32,
    pub rotation: [f32; 4], // quaternion xyzw
    pub scale: [f32; 3],
    pub interactive: bool,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- VerseMember serde roundtrip ---

    #[test]
    fn verse_member_default() {
        let m = VerseMember::default();
        assert_eq!(m.member_id, "");
        assert_eq!(m.status, MemberStatus::Active);
        assert!(m.revoked_at.is_none());
        assert!(m.revoked_by.is_none());
    }

    #[test]
    fn verse_member_serde_roundtrip() {
        let m = VerseMember {
            member_id: "01HZ1ABCDEF".to_string(),
            verse_id: "verse-1".to_string(),
            peer_did: "did:key:z6Mk".to_string(),
            status: MemberStatus::Active,
            invited_by: "did:key:z6Ml".to_string(),
            invite_sig: "deadbeef".to_string(),
            invite_timestamp: "2026-01-01T00:00:00Z".to_string(),
            revoked_at: None,
            revoked_by: None,
        };
        let json = serde_json::to_string(&m).expect("serialize");
        let back: VerseMember = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(m, back);
    }

    #[test]
    fn verse_member_revoked_serde_roundtrip() {
        let m = VerseMember {
            member_id: "01HZ2".to_string(),
            verse_id: "verse-2".to_string(),
            peer_did: "did:key:z6Mk".to_string(),
            status: MemberStatus::Revoked,
            invited_by: "did:key:z6Ml".to_string(),
            invite_sig: "cafebabe".to_string(),
            invite_timestamp: "2026-01-02T00:00:00Z".to_string(),
            revoked_at: Some("2026-06-01T00:00:00Z".to_string()),
            revoked_by: Some("did:key:z6Mo".to_string()),
        };
        let json = serde_json::to_string(&m).expect("serialize");
        let back: VerseMember = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(m, back);
    }

    #[test]
    fn member_status_serializes_lowercase() {
        let active = serde_json::to_string(&MemberStatus::Active).expect("serialize active");
        assert_eq!(active, "\"active\"");
        let revoked = serde_json::to_string(&MemberStatus::Revoked).expect("serialize revoked");
        assert_eq!(revoked, "\"revoked\"");
    }

    #[test]
    fn member_status_deserializes_lowercase() {
        let active: MemberStatus = serde_json::from_str("\"active\"").expect("deserialize active");
        assert_eq!(active, MemberStatus::Active);
        let revoked: MemberStatus =
            serde_json::from_str("\"revoked\"").expect("deserialize revoked");
        assert_eq!(revoked, MemberStatus::Revoked);
    }

    // --- VerseOverview serde roundtrip ---

    #[test]
    fn verse_overview_default() {
        let v = VerseOverview::default();
        assert_eq!(v.member_count, 0);
        assert_eq!(v.fractal_count, 0);
    }

    #[test]
    fn verse_overview_serde_roundtrip() {
        let v = VerseOverview {
            verse_id: "v-1".to_string(),
            name: "My Verse".to_string(),
            member_count: 5,
            fractal_count: 3,
            created_by: "did:key:z6Mk".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&v).expect("serialize");
        let back: VerseOverview = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(v, back);
    }

    // --- FractalMetadata serde roundtrip ---

    #[test]
    fn fractal_metadata_default() {
        let f = FractalMetadata::default();
        assert_eq!(f.petal_count, 0);
        assert!(f.description.is_none());
    }

    #[test]
    fn fractal_metadata_serde_roundtrip() {
        let f = FractalMetadata {
            fractal_id: "frac-1".to_string(),
            verse_id: "verse-1".to_string(),
            owner_did: "did:key:z6Mk".to_string(),
            name: "My Fractal".to_string(),
            description: Some("A test fractal".to_string()),
            petal_count: 7,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&f).expect("serialize");
        let back: FractalMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(f, back);
    }

    #[test]
    fn fractal_metadata_no_description_roundtrip() {
        let f = FractalMetadata {
            fractal_id: "frac-2".to_string(),
            verse_id: "verse-1".to_string(),
            owner_did: "did:key:z6Mk".to_string(),
            name: "Bare Fractal".to_string(),
            description: None,
            petal_count: 0,
            created_at: "2026-02-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&f).expect("serialize");
        let back: FractalMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(f, back);
    }

    // --- NodeMetadata serde roundtrip ---

    #[test]
    fn node_metadata_default() {
        let n = NodeMetadata::default();
        assert_eq!(n.position, [0.0_f32; 2]);
        assert_eq!(n.elevation, 0.0_f32);
        assert_eq!(n.rotation, [0.0_f32; 4]);
        assert_eq!(n.scale, [0.0_f32; 3]);
        assert!(!n.interactive);
        assert!(n.display_name.is_none());
        assert!(n.asset_id.is_none());
    }

    #[test]
    fn node_metadata_serde_roundtrip() {
        let n = NodeMetadata {
            node_id: "node-1".to_string(),
            petal_id: "petal-1".to_string(),
            display_name: Some("The Cube".to_string()),
            asset_id: Some("01HZ_ASSET_ULID".to_string()),
            position: [1.0, 3.0],
            elevation: 2.5,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            interactive: true,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&n).expect("serialize");
        let back: NodeMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(n, back);
    }

    #[test]
    fn node_metadata_no_optionals_roundtrip() {
        let n = NodeMetadata {
            node_id: "node-2".to_string(),
            petal_id: "petal-2".to_string(),
            display_name: None,
            asset_id: None,
            position: [0.0, 0.0],
            elevation: 0.0,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            interactive: false,
            created_at: "2026-03-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&n).expect("serialize");
        let back: NodeMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(n, back);
    }
}
