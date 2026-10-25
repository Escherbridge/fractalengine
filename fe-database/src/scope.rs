//! Hierarchical scope string parsing — see fe-policy/AGENTS.md §scope-parser.
//! The pure parser moved into fe-policy (W5b, OF-3): this module is now a
//! re-export so every `fe_database::{parse_scope, build_scope, parent_scope,
//! scope_level, ScopeParts}` call site across the workspace is unchanged.
//! The parser's own unit tests live with the implementation in
//! `fe-policy/src/scope.rs`; the tests below are the differential check the
//! unification promised — both public entry points must agree, always.

pub use fe_policy::scope::{
    build_scope, parent_scope, parse_scope, scope_level, ScopeError, ScopeLevel, ScopeParts,
};
pub use fe_policy::scope_contains;

#[cfg(test)]
mod differential_tests {
    //! Drives `fe_database::scope::*` (this module, re-exported) and
    //! `fe_policy::scope::*` (the canonical implementation) over the same
    //! fixtures and asserts byte-identical results — the guard OF-3 asked
    //! for against a future re-duplication of this parser.

    /// Well-formed, malformed, interim `VERSE#<hex>` authority keys (see
    /// `fe-database/src/role_grants.rs::is_interim_authority_verse_id` and
    /// `fe-database/src/canon_log/canonical_epoch.rs::interim_authority_scope_key`),
    /// and a deep verse/fractal/petal chain with hyphen-bearing ids.
    fn fixtures() -> Vec<&'static str> {
        vec![
            // well-formed
            "VERSE#v1",
            "VERSE#v1-FRACTAL#f1",
            "VERSE#v1-FRACTAL#f1-PETAL#p1",
            "VERSE#verse-abc-FRACTAL#fractal-xyz-PETAL#petal-123",
            // malformed
            "",
            "FRACTAL#f1",
            "PETAL#p1",
            "random-string",
            "VERSE#",
            "VERSE#v1-FRACTAL#",
            "VERSE#v1-FRACTAL#f1-PETAL#",
            // interim authority verse key (canon_log §interim-authority): 64
            // lowercase hex, no FRACTAL#/PETAL# segments — structurally out
            // of the hierarchy parser's reach, same as any other verse id.
            "VERSE#a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
            // the full canonical-log interim key shape (verse hex + a
            // -PETAL#<hex> segment with no preceding -FRACTAL#, plus the
            // -RESOURCE#<hex> suffix `parse_scope` does not know about at
            // all): both entry points must still land on the same outcome.
            "VERSE#a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1-PETAL#b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2-RESOURCE#c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3",
        ]
    }

    #[test]
    fn parse_scope_agrees_across_both_entry_points() {
        for scope in fixtures() {
            assert_eq!(
                crate::scope::parse_scope(scope),
                fe_policy::scope::parse_scope(scope),
                "parse_scope diverged for {scope:?}"
            );
        }
    }

    #[test]
    fn parent_scope_agrees_across_both_entry_points() {
        for scope in fixtures() {
            assert_eq!(
                crate::scope::parent_scope(scope),
                fe_policy::scope::parent_scope(scope),
                "parent_scope diverged for {scope:?}"
            );
        }
    }

    #[test]
    fn scope_level_agrees_across_both_entry_points() {
        for scope in fixtures() {
            assert_eq!(
                crate::scope::scope_level(scope),
                fe_policy::scope::scope_level(scope),
                "scope_level diverged for {scope:?}"
            );
        }
    }

    #[test]
    fn build_scope_agrees_across_both_entry_points() {
        let cases: Vec<(&str, Option<&str>, Option<&str>)> = vec![
            ("v1", None, None),
            ("v1", Some("f1"), None),
            ("v1", Some("f1"), Some("p1")),
            ("verse-abc", Some("fractal-xyz"), Some("petal-123")),
        ];
        for (verse_id, fractal_id, petal_id) in cases {
            assert_eq!(
                crate::scope::build_scope(verse_id, fractal_id, petal_id),
                fe_policy::scope::build_scope(verse_id, fractal_id, petal_id),
                "build_scope diverged for {verse_id:?}/{fractal_id:?}/{petal_id:?}"
            );
        }
    }
}
