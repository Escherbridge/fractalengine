//! Content addressing (blob hashing) + render-side node addressing.
//!
//! `RenderNodeAddress` is a lock-step mirror of `fe_entity_store::NodeAddress`
//! (T1 owns the canonical `fe://verse/fractal/petal/node` URI). fe-renderer must
//! not depend on fe-entity-store, so the encoder is duplicated here and pinned
//! byte-for-byte by `render_uri_matches_t1_format` — the same mirror pattern
//! fe-database uses for `lifecycle_uri`. See `fe-renderer/src/AGENTS.md`.

use fe_network::AssetId;

pub fn content_address(bytes: &[u8]) -> AssetId {
    let hash = blake3::hash(bytes);
    AssetId(*hash.as_bytes())
}

pub fn validate_glb_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0..4] == [0x67, 0x6C, 0x54, 0x46]
}

// ---------------------------------------------------------------------------
// Render-side node address (mirror of fe_entity_store::NodeAddress, FR-1)
// ---------------------------------------------------------------------------

/// URI scheme prefix — must equal `fe_entity_store::address::SCHEME`.
pub const SCHEME: &str = "fe://";

/// The render-side view of a node's stable `fe://verse/fractal/petal/node`
/// address (T5 FR-1). Derived purely from the four hierarchy ids, so it is
/// invariant under rename/move. Lets viewport/picking code resolve a rendered
/// entity to its public endpoint without a data-layer round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RenderNodeAddress {
    pub verse_id: String,
    pub fractal_id: String,
    pub petal_id: String,
    pub node_id: String,
}

impl RenderNodeAddress {
    /// Build from an explicit id 4-tuple; `None` if any segment is empty.
    pub fn new(
        verse_id: impl Into<String>,
        fractal_id: impl Into<String>,
        petal_id: impl Into<String>,
        node_id: impl Into<String>,
    ) -> Option<Self> {
        let a = Self {
            verse_id: verse_id.into(),
            fractal_id: fractal_id.into(),
            petal_id: petal_id.into(),
            node_id: node_id.into(),
        };
        if a.verse_id.is_empty()
            || a.fractal_id.is_empty()
            || a.petal_id.is_empty()
            || a.node_id.is_empty()
        {
            return None;
        }
        Some(a)
    }

    /// Derive from a petal-level scope (`VERSE#v-FRACTAL#f-PETAL#p`) + node id.
    pub fn from_scope_and_id(scope: &str, node_id: &str) -> Option<Self> {
        let (v, f, p) = parse_petal_scope(scope)?;
        Self::new(v, f, p, node_id.to_string())
    }

    /// The petal-level scope string this address belongs to.
    pub fn scope(&self) -> String {
        format!(
            "VERSE#{}-FRACTAL#{}-PETAL#{}",
            self.verse_id, self.fractal_id, self.petal_id
        )
    }

    /// Render the public URI `fe://verse/fractal/petal/node`.
    pub fn to_uri(&self) -> String {
        format!(
            "{SCHEME}{}/{}/{}/{}",
            encode(&self.verse_id),
            encode(&self.fractal_id),
            encode(&self.petal_id),
            encode(&self.node_id),
        )
    }

    /// Parse a `fe://verse/fractal/petal/node` URI back into an address.
    pub fn from_uri(uri: &str) -> Option<Self> {
        let rest = uri.strip_prefix(SCHEME)?;
        let segs: Vec<&str> = rest.split('/').collect();
        if segs.len() != 4 {
            return None;
        }
        Self::new(
            decode(segs[0]),
            decode(segs[1]),
            decode(segs[2]),
            decode(segs[3]),
        )
    }
}

/// Parse a petal-level scope into `(verse, fractal, petal)`; keyword-boundary
/// split so hyphen-bearing ids survive (mirrors T1 + `fe_database::parse_scope`).
fn parse_petal_scope(scope: &str) -> Option<(String, String, String)> {
    let rest = scope.strip_prefix("VERSE#")?;
    let vi = rest.find("-FRACTAL#")?;
    let (verse, after_v) = (&rest[..vi], &rest[vi + "-FRACTAL#".len()..]);
    let fi = after_v.find("-PETAL#")?;
    let (fractal, petal) = (&after_v[..fi], &after_v[fi + "-PETAL#".len()..]);
    if verse.is_empty() || fractal.is_empty() || petal.is_empty() {
        return None;
    }
    Some((verse.to_string(), fractal.to_string(), petal.to_string()))
}

/// Percent-encode the three URI-reserved characters (mirror of T1 `encode`).
fn encode(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for ch in seg.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '/' => out.push_str("%2F"),
            '#' => out.push_str("%23"),
            other => out.push(other),
        }
    }
    out
}

/// Reverse [`encode`]; unknown `%` sequences pass through verbatim.
fn decode(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    let bytes = seg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            match &seg[i..i + 3] {
                "%25" => {
                    out.push('%');
                    i += 3;
                    continue;
                }
                "%2F" => {
                    out.push('/');
                    i += 3;
                    continue;
                }
                "%23" => {
                    out.push('#');
                    i += 3;
                    continue;
                }
                _ => {}
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_uri_matches_t1_format() {
        // Pinned byte-for-byte to fe_entity_store::NodeAddress::to_uri (T1).
        let a = RenderNodeAddress::from_scope_and_id("VERSE#v1-FRACTAL#f1-PETAL#p1", "n1").unwrap();
        assert_eq!(a.to_uri(), "fe://v1/f1/p1/n1");
        assert_eq!(RenderNodeAddress::from_uri(&a.to_uri()).unwrap(), a);
    }

    #[test]
    fn promoted_instance_id_round_trips() {
        // Promoted stamp ids embed '#'; must survive the URI round-trip (T1 parity).
        let a = RenderNodeAddress::new("v1", "f1", "p1", "path1#inst-3").unwrap();
        assert_eq!(a.to_uri(), "fe://v1/f1/p1/path1%23inst-3");
        assert_eq!(RenderNodeAddress::from_uri(&a.to_uri()).unwrap(), a);
    }

    #[test]
    fn hyphen_ids_and_scope_rebuild() {
        let a = RenderNodeAddress::from_scope_and_id(
            "VERSE#ve-rse-FRACTAL#fr-ac-PETAL#pe-tal",
            "no-de",
        )
        .unwrap();
        assert_eq!(a.verse_id, "ve-rse");
        assert_eq!(a.node_id, "no-de");
        assert_eq!(a.scope(), "VERSE#ve-rse-FRACTAL#fr-ac-PETAL#pe-tal");
    }

    #[test]
    fn stable_across_rename_and_move() {
        let scope = "VERSE#v1-FRACTAL#f1-PETAL#p1";
        let before = RenderNodeAddress::from_scope_and_id(scope, "n1").unwrap();
        let after = RenderNodeAddress::from_scope_and_id(scope, "n1").unwrap();
        assert_eq!(before.to_uri(), after.to_uri());
    }

    #[test]
    fn rejects_malformed_and_non_petal_scope() {
        assert!(RenderNodeAddress::from_uri("http://v/f/p/n").is_none());
        assert!(RenderNodeAddress::from_uri("fe://v/f/p").is_none());
        assert!(RenderNodeAddress::from_uri("fe://v/f/p/n/extra").is_none());
        assert!(RenderNodeAddress::from_scope_and_id("VERSE#v1", "n1").is_none());
    }
}
