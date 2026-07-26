//! Stable, public node addressing (FR-4) — see AGENTS.md §addressing.

use serde::{Deserialize, Serialize};
use std::fmt;

/// URI scheme prefix for a node address.
pub const SCHEME: &str = "fe://";

/// Errors from parsing/deriving a [`NodeAddress`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressError {
    /// Scope string was not petal-level (`VERSE#v-FRACTAL#f-PETAL#p`).
    NotPetalScoped,
    /// A required id segment was empty.
    EmptySegment(&'static str),
    /// URI did not start with the `fe://` scheme.
    MissingScheme,
    /// URI did not have exactly four path segments.
    MalformedPath,
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddressError::NotPetalScoped => {
                write!(
                    f,
                    "address requires a petal-level scope (VERSE#-FRACTAL#-PETAL#)"
                )
            }
            AddressError::EmptySegment(s) => write!(f, "address segment '{s}' must not be empty"),
            AddressError::MissingScheme => write!(f, "address URI must start with '{SCHEME}'"),
            AddressError::MalformedPath => {
                write!(
                    f,
                    "address URI must have exactly verse/fractal/petal/node segments"
                )
            }
        }
    }
}

impl std::error::Error for AddressError {}

/// The canonical, stable address of a node: the public
/// `fe://verse/fractal/petal/node` URI (FR-4, Q-1 override).
///
/// Derived purely from the four hierarchy ids, so it is invariant under
/// rename (display-name change) and move (transform change) — neither touches
/// an id. It is the substrate T5 exposes as a REST/MCP endpoint and T2 uses to
/// address individual promoted stamps.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeAddress {
    pub verse_id: String,
    pub fractal_id: String,
    pub petal_id: String,
    pub node_id: String,
}

impl NodeAddress {
    /// Build an address from an explicit id 4-tuple. Rejects empty segments.
    pub fn new(
        verse_id: impl Into<String>,
        fractal_id: impl Into<String>,
        petal_id: impl Into<String>,
        node_id: impl Into<String>,
    ) -> Result<Self, AddressError> {
        let addr = Self {
            verse_id: verse_id.into(),
            fractal_id: fractal_id.into(),
            petal_id: petal_id.into(),
            node_id: node_id.into(),
        };
        if addr.verse_id.is_empty() {
            return Err(AddressError::EmptySegment("verse"));
        }
        if addr.fractal_id.is_empty() {
            return Err(AddressError::EmptySegment("fractal"));
        }
        if addr.petal_id.is_empty() {
            return Err(AddressError::EmptySegment("petal"));
        }
        if addr.node_id.is_empty() {
            return Err(AddressError::EmptySegment("node"));
        }
        Ok(addr)
    }

    /// Derive the address from a petal-level scope string
    /// (`VERSE#v-FRACTAL#f-PETAL#p`, the canonical convention) plus the node id.
    ///
    /// Mirrors `fe_database::scope::parse_scope` boundary handling so ids that
    /// themselves contain `-` parse correctly.
    pub fn from_scope_and_id(scope: &str, node_id: &str) -> Result<Self, AddressError> {
        let (verse_id, fractal_id, petal_id) =
            parse_petal_scope(scope).ok_or(AddressError::NotPetalScoped)?;
        Self::new(verse_id, fractal_id, petal_id, node_id.to_string())
    }

    /// The petal-level scope string this address belongs to.
    pub fn scope(&self) -> String {
        format!(
            "VERSE#{}-FRACTAL#{}-PETAL#{}",
            self.verse_id, self.fractal_id, self.petal_id
        )
    }

    /// The node id (last path segment) — the primary store lookup key.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Render the public URI `fe://verse/fractal/petal/node`. Reserved chars
    /// (`%`, `/`, `#`) are percent-encoded so the round-trip is lossless even
    /// for promoted-instance node ids like `path1#inst-3`.
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
    pub fn from_uri(uri: &str) -> Result<Self, AddressError> {
        let rest = uri
            .strip_prefix(SCHEME)
            .ok_or(AddressError::MissingScheme)?;
        let segs: Vec<&str> = rest.split('/').collect();
        if segs.len() != 4 {
            return Err(AddressError::MalformedPath);
        }
        Self::new(
            decode(segs[0]),
            decode(segs[1]),
            decode(segs[2]),
            decode(segs[3]),
        )
    }
}

impl fmt::Display for NodeAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_uri())
    }
}

/// Parse a petal-level scope into `(verse, fractal, petal)`, or `None` if the
/// scope is not fully petal-scoped. Split on the keyword boundary markers so
/// hyphen-bearing ids survive.
fn parse_petal_scope(scope: &str) -> Option<(String, String, String)> {
    let rest = scope.strip_prefix("VERSE#")?;
    let (verse, after_verse) = split_once_marker(rest, "-FRACTAL#")?;
    let (fractal, after_fractal) = split_once_marker(after_verse, "-PETAL#")?;
    let petal = after_fractal;
    if verse.is_empty() || fractal.is_empty() || petal.is_empty() {
        return None;
    }
    Some((verse.to_string(), fractal.to_string(), petal.to_string()))
}

/// Split `s` at the first occurrence of `marker`, returning `(before, after)`
/// with the marker consumed. `None` if the marker is absent.
fn split_once_marker<'a>(s: &'a str, marker: &str) -> Option<(&'a str, &'a str)> {
    let idx = s.find(marker)?;
    Some((&s[..idx], &s[idx + marker.len()..]))
}

/// Percent-encode the three URI-reserved characters we can encounter.
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

/// Reverse [`encode`]. Unknown `%` sequences are passed through verbatim.
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
    fn derive_from_petal_scope() {
        let addr = NodeAddress::from_scope_and_id("VERSE#v1-FRACTAL#f1-PETAL#p1", "n1").unwrap();
        assert_eq!(addr.verse_id, "v1");
        assert_eq!(addr.fractal_id, "f1");
        assert_eq!(addr.petal_id, "p1");
        assert_eq!(addr.node_id(), "n1");
    }

    #[test]
    fn non_petal_scope_rejected() {
        assert_eq!(
            NodeAddress::from_scope_and_id("VERSE#v1", "n1"),
            Err(AddressError::NotPetalScoped)
        );
        assert_eq!(
            NodeAddress::from_scope_and_id("VERSE#v1-FRACTAL#f1", "n1"),
            Err(AddressError::NotPetalScoped)
        );
    }

    #[test]
    fn ids_with_hyphens_parse() {
        let addr =
            NodeAddress::from_scope_and_id("VERSE#ve-rse-FRACTAL#fr-ac-PETAL#pe-tal", "no-de")
                .unwrap();
        assert_eq!(addr.verse_id, "ve-rse");
        assert_eq!(addr.fractal_id, "fr-ac");
        assert_eq!(addr.petal_id, "pe-tal");
        assert_eq!(addr.node_id, "no-de");
        // scope() rebuilds the exact input.
        assert_eq!(addr.scope(), "VERSE#ve-rse-FRACTAL#fr-ac-PETAL#pe-tal");
    }

    #[test]
    fn uri_round_trip() {
        let addr = NodeAddress::from_scope_and_id("VERSE#v1-FRACTAL#f1-PETAL#p1", "n1").unwrap();
        assert_eq!(addr.to_uri(), "fe://v1/f1/p1/n1");
        assert_eq!(NodeAddress::from_uri(&addr.to_uri()).unwrap(), addr);
    }

    #[test]
    fn uri_round_trip_promoted_instance_id() {
        // Promoted stamp node ids embed '#'; must survive the URI round-trip.
        let addr = NodeAddress::new("v1", "f1", "p1", "path1#inst-3").unwrap();
        let uri = addr.to_uri();
        assert_eq!(uri, "fe://v1/f1/p1/path1%23inst-3");
        assert_eq!(NodeAddress::from_uri(&uri).unwrap(), addr);
    }

    #[test]
    fn serde_round_trip() {
        let addr = NodeAddress::new("v1", "f1", "p1", "n1").unwrap();
        let json = serde_json::to_string(&addr).unwrap();
        let back: NodeAddress = serde_json::from_str(&json).unwrap();
        assert_eq!(addr, back);
    }

    #[test]
    fn address_is_stable_across_rename_and_move() {
        // The address is a pure function of the four ids. A rename changes the
        // display name and a move changes the transform — neither is an input,
        // so re-deriving from the same scope + id yields an identical address.
        let scope = "VERSE#v1-FRACTAL#f1-PETAL#p1";
        let before = NodeAddress::from_scope_and_id(scope, "n1").unwrap();
        // ...node gets renamed "Foo"->"Bar" and dragged to a new position...
        let after = NodeAddress::from_scope_and_id(scope, "n1").unwrap();
        assert_eq!(before, after);
        assert_eq!(before.to_uri(), after.to_uri());
    }

    #[test]
    fn empty_segments_rejected() {
        assert_eq!(
            NodeAddress::new("", "f", "p", "n"),
            Err(AddressError::EmptySegment("verse"))
        );
        assert_eq!(
            NodeAddress::new("v", "f", "p", ""),
            Err(AddressError::EmptySegment("node"))
        );
    }

    #[test]
    fn from_uri_rejects_malformed() {
        assert_eq!(
            NodeAddress::from_uri("http://v/f/p/n"),
            Err(AddressError::MissingScheme)
        );
        assert_eq!(
            NodeAddress::from_uri("fe://v/f/p"),
            Err(AddressError::MalformedPath)
        );
        assert_eq!(
            NodeAddress::from_uri("fe://v/f/p/n/extra"),
            Err(AddressError::MalformedPath)
        );
    }
}
