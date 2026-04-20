use std::fmt;

/// Level of a parsed scope string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeLevel {
    Verse,
    Fractal,
    Petal,
}

/// Parsed components of a hierarchical scope string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeParts {
    pub verse_id: String,
    pub fractal_id: Option<String>,
    pub petal_id: Option<String>,
}

/// Errors that can occur during scope parsing or building.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeError {
    /// Input string was empty.
    Empty,
    /// String does not begin with `VERSE#`.
    MissingVersePrefix,
    /// Verse ID segment is empty.
    EmptyVerseId,
    /// A `PETAL#` segment was present without a preceding `FRACTAL#` segment.
    PetalWithoutFractal,
    /// String contains an unrecognized segment prefix.
    UnrecognizedSegment(String),
    /// A `FRACTAL#` segment was present without a corresponding ID value.
    EmptyFractalId,
    /// A `PETAL#` segment was present without a corresponding ID value.
    EmptyPetalId,
}

impl fmt::Display for ScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScopeError::Empty => write!(f, "scope string is empty"),
            ScopeError::MissingVersePrefix => write!(f, "scope must start with 'VERSE#'"),
            ScopeError::EmptyVerseId => write!(f, "verse_id must not be empty"),
            ScopeError::PetalWithoutFractal => {
                write!(f, "PETAL# segment requires a preceding FRACTAL# segment")
            }
            ScopeError::UnrecognizedSegment(s) => {
                write!(f, "unrecognized scope segment: '{}'", s)
            }
            ScopeError::EmptyFractalId => write!(f, "fractal_id must not be empty"),
            ScopeError::EmptyPetalId => write!(f, "petal_id must not be empty"),
        }
    }
}

impl std::error::Error for ScopeError {}

/// Build a hierarchical scope string from its components.
///
/// # Format
/// - Verse only:          `"VERSE#<verse_id>"`
/// - Verse + Fractal:     `"VERSE#<verse_id>-FRACTAL#<fractal_id>"`
/// - Verse + Fractal + Petal: `"VERSE#<verse_id>-FRACTAL#<fractal_id>-PETAL#<petal_id>"`
///
/// # Panics
/// Panics if `petal_id` is `Some` but `fractal_id` is `None` — invalid hierarchy.
pub fn build_scope(
    verse_id: &str,
    fractal_id: Option<&str>,
    petal_id: Option<&str>,
) -> String {
    if petal_id.is_some() && fractal_id.is_none() {
        panic!("build_scope: petal_id requires fractal_id (invalid hierarchy)");
    }

    let mut s = format!("VERSE#{}", verse_id);
    if let Some(fid) = fractal_id {
        s.push_str(&format!("-FRACTAL#{}", fid));
        if let Some(pid) = petal_id {
            s.push_str(&format!("-PETAL#{}", pid));
        }
    }
    s
}

/// Parse a scope string into its constituent parts.
///
/// Accepts any string produced by [`build_scope`].
pub fn parse_scope(scope: &str) -> Result<ScopeParts, ScopeError> {
    if scope.is_empty() {
        return Err(ScopeError::Empty);
    }

    // Segments are separated by '-' but only at the top-level keyword boundaries
    // (VERSE#, FRACTAL#, PETAL#). We split by '-' then reassemble segments that
    // belong to each keyword prefix.
    //
    // Because IDs themselves might contain '-', we instead split on the known
    // boundary markers: "-FRACTAL#" and "-PETAL#".

    // Validate and strip VERSE# prefix
    if !scope.starts_with("VERSE#") {
        return Err(ScopeError::MissingVersePrefix);
    }

    let rest = &scope["VERSE#".len()..];

    // Find the optional -FRACTAL# boundary
    let (verse_part, after_verse) = if let Some(idx) = rest.find("-FRACTAL#") {
        (&rest[..idx], Some(&rest[idx + 1..]))
    } else {
        (rest, None)
    };

    if verse_part.is_empty() {
        return Err(ScopeError::EmptyVerseId);
    }

    let verse_id = verse_part.to_string();

    let (fractal_id, petal_id) = match after_verse {
        None => (None, None),
        Some(fractal_and_rest) => {
            // fractal_and_rest starts with "FRACTAL#..."
            if !fractal_and_rest.starts_with("FRACTAL#") {
                return Err(ScopeError::UnrecognizedSegment(fractal_and_rest.to_string()));
            }
            let after_fractal_prefix = &fractal_and_rest["FRACTAL#".len()..];

            let (fid, after_fractal) = if let Some(idx) = after_fractal_prefix.find("-PETAL#") {
                (&after_fractal_prefix[..idx], Some(&after_fractal_prefix[idx + 1..]))
            } else {
                (after_fractal_prefix, None)
            };

            if fid.is_empty() {
                return Err(ScopeError::EmptyFractalId);
            }

            let fractal_id = Some(fid.to_string());

            let petal_id = match after_fractal {
                None => None,
                Some(petal_seg) => {
                    if !petal_seg.starts_with("PETAL#") {
                        return Err(ScopeError::UnrecognizedSegment(petal_seg.to_string()));
                    }
                    let pid = &petal_seg["PETAL#".len()..];
                    if pid.is_empty() {
                        return Err(ScopeError::EmptyPetalId);
                    }
                    Some(pid.to_string())
                }
            };

            (fractal_id, petal_id)
        }
    };

    Ok(ScopeParts {
        verse_id,
        fractal_id,
        petal_id,
    })
}

/// Returns the parent scope of a given scope string, or `None` if already at
/// the verse level (no parent).
///
/// # Examples
/// - `"VERSE#v-FRACTAL#f-PETAL#p"` → `Some("VERSE#v-FRACTAL#f")`
/// - `"VERSE#v-FRACTAL#f"` → `Some("VERSE#v")`
/// - `"VERSE#v"` → `None`
pub fn parent_scope(scope: &str) -> Option<String> {
    // Walk from the right: look for the last occurrence of "-PETAL#" or "-FRACTAL#"
    if let Some(idx) = scope.rfind("-PETAL#") {
        return Some(scope[..idx].to_string());
    }
    if let Some(idx) = scope.rfind("-FRACTAL#") {
        return Some(scope[..idx].to_string());
    }
    // Already at verse level
    None
}

/// Returns the [`ScopeLevel`] of a scope string.
pub fn scope_level(scope: &str) -> Result<ScopeLevel, ScopeError> {
    let parts = parse_scope(scope)?;
    if parts.petal_id.is_some() {
        Ok(ScopeLevel::Petal)
    } else if parts.fractal_id.is_some() {
        Ok(ScopeLevel::Fractal)
    } else {
        Ok(ScopeLevel::Verse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- build_scope ---

    #[test]
    fn build_verse_only() {
        assert_eq!(build_scope("v1", None, None), "VERSE#v1");
    }

    #[test]
    fn build_verse_fractal() {
        assert_eq!(
            build_scope("v1", Some("f1"), None),
            "VERSE#v1-FRACTAL#f1"
        );
    }

    #[test]
    fn build_verse_fractal_petal() {
        assert_eq!(
            build_scope("v1", Some("f1"), Some("p1")),
            "VERSE#v1-FRACTAL#f1-PETAL#p1"
        );
    }

    #[test]
    #[should_panic(expected = "petal_id requires fractal_id")]
    fn build_petal_without_fractal_panics() {
        build_scope("v1", None, Some("p1"));
    }

    // --- parse_scope ---

    #[test]
    fn parse_verse_only() {
        let s = build_scope("v1", None, None);
        let parts = parse_scope(&s).unwrap();
        assert_eq!(parts.verse_id, "v1");
        assert_eq!(parts.fractal_id, None);
        assert_eq!(parts.petal_id, None);
    }

    #[test]
    fn parse_verse_fractal() {
        let s = build_scope("v1", Some("f1"), None);
        let parts = parse_scope(&s).unwrap();
        assert_eq!(parts.verse_id, "v1");
        assert_eq!(parts.fractal_id, Some("f1".to_string()));
        assert_eq!(parts.petal_id, None);
    }

    #[test]
    fn parse_verse_fractal_petal() {
        let s = build_scope("v1", Some("f1"), Some("p1"));
        let parts = parse_scope(&s).unwrap();
        assert_eq!(parts.verse_id, "v1");
        assert_eq!(parts.fractal_id, Some("f1".to_string()));
        assert_eq!(parts.petal_id, Some("p1".to_string()));
    }

    #[test]
    fn parse_round_trip() {
        let cases = [
            build_scope("verse-abc", None, None),
            build_scope("verse-abc", Some("fractal-xyz"), None),
            build_scope("verse-abc", Some("fractal-xyz"), Some("petal-123")),
        ];
        for s in &cases {
            let parts = parse_scope(s).unwrap();
            let rebuilt = build_scope(
                &parts.verse_id,
                parts.fractal_id.as_deref(),
                parts.petal_id.as_deref(),
            );
            assert_eq!(*s, rebuilt);
        }
    }

    #[test]
    fn parse_error_empty() {
        assert_eq!(parse_scope(""), Err(ScopeError::Empty));
    }

    #[test]
    fn parse_error_missing_verse_prefix() {
        assert_eq!(
            parse_scope("FRACTAL#f1"),
            Err(ScopeError::MissingVersePrefix)
        );
        assert_eq!(
            parse_scope("PETAL#p1"),
            Err(ScopeError::MissingVersePrefix)
        );
        assert_eq!(
            parse_scope("random-string"),
            Err(ScopeError::MissingVersePrefix)
        );
    }

    #[test]
    fn parse_error_empty_verse_id() {
        assert_eq!(parse_scope("VERSE#"), Err(ScopeError::EmptyVerseId));
    }

    // --- parent_scope ---

    #[test]
    fn parent_of_petal_is_fractal() {
        let s = build_scope("v", Some("f"), Some("p"));
        assert_eq!(parent_scope(&s), Some(build_scope("v", Some("f"), None)));
    }

    #[test]
    fn parent_of_fractal_is_verse() {
        let s = build_scope("v", Some("f"), None);
        assert_eq!(parent_scope(&s), Some(build_scope("v", None, None)));
    }

    #[test]
    fn parent_of_verse_is_none() {
        let s = build_scope("v", None, None);
        assert_eq!(parent_scope(&s), None);
    }

    // --- scope_level ---

    #[test]
    fn scope_level_verse() {
        let s = build_scope("v1", None, None);
        assert_eq!(scope_level(&s).unwrap(), ScopeLevel::Verse);
    }

    #[test]
    fn scope_level_fractal() {
        let s = build_scope("v1", Some("f1"), None);
        assert_eq!(scope_level(&s).unwrap(), ScopeLevel::Fractal);
    }

    #[test]
    fn scope_level_petal() {
        let s = build_scope("v1", Some("f1"), Some("p1"));
        assert_eq!(scope_level(&s).unwrap(), ScopeLevel::Petal);
    }

    #[test]
    fn scope_level_error_on_invalid() {
        assert!(scope_level("").is_err());
        assert!(scope_level("bad-input").is_err());
    }
}
