//! Per-version write-time admission (QA-0165); see field_clock/AGENTS.md §admission.

use super::wire::FieldVersion;
use crate::handlers::crud::{
    resolve_fractal_scope_handler, resolve_node_petal_id_handler, resolve_petal_scope_handler,
    resolve_verse_scope_handler,
};
use crate::repo::Db;
use fe_policy::RoleLevel;

/// A verse's owner and admitted role statements, the only inputs to admission.
pub(crate) struct VerseView {
    pub(crate) owner: String,
    pub(crate) admitted: Vec<fe_policy::RoleStatement>,
}

impl VerseView {
    /// Load the verse owner and its admitted role statements (owner `""` before the manifest).
    pub(crate) async fn load(db: &Db, verse_id: &str) -> anyhow::Result<Self> {
        let owner = crate::role_grants::verse_owner(db, verse_id)
            .await?
            .unwrap_or_default();
        let admitted = crate::role_grants::load_admitted(db, verse_id).await?;
        Ok(Self { owner, admitted })
    }

    /// The verdict that needs no scope: legacy floor and owner admit, an anonymous or ownerless one refuses.
    fn settled(&self, version: &FieldVersion) -> Option<bool> {
        if version.is_legacy_floor() {
            Some(true)
        } else if version.author.is_empty() || self.owner.is_empty() {
            Some(false)
        } else if version.author == self.owner {
            Some(true)
        } else {
            None
        }
    }

    /// Whether `version` is admissible for a `table` row whose deepest scope is `row_scope`.
    pub(crate) fn admits_at(&self, table: &str, row_scope: &str, version: &FieldVersion) -> bool {
        if let Some(verdict) = self.settled(version) {
            return verdict;
        }
        crate::role_grants::held_at_write_in(
            &self.owner,
            &self.admitted,
            &version.author,
            row_scope,
            required_level(table, &version.field),
            version.hlc,
        )
    }
}

/// Level a key's author must have held at write time (spec §3.2); the scope is always the row's deepest.
pub(crate) fn required_level(table: &str, key: &str) -> RoleLevel {
    let governance = match table {
        "verse" => {
            key == "namespace_id"
                || crate::handlers::replicated_row::VERSE_GOVERNANCE_FIELDS.contains(&key)
        }
        "petal" => key == "hexon_manifest",
        _ => false,
    };
    if governance {
        RoleLevel::Manager
    } else {
        RoleLevel::Editor
    }
}

/// Deepest scope of a stored row, `None` when it is unknown locally or sits in another verse.
async fn stored_row_scope(
    db: &Db,
    verse_id: &str,
    table: &str,
    rid: &str,
) -> anyhow::Result<Option<String>> {
    let scope = match table {
        "verse" if rid == verse_id => resolve_verse_scope_handler(db, rid).await?,
        "fractal" => resolve_fractal_scope_handler(db, rid).await?,
        "petal" => resolve_petal_scope_handler(db, rid).await?,
        "node" => match resolve_node_petal_id_handler(db, rid).await? {
            Some(petal_id) => resolve_petal_scope_handler(db, &petal_id).await?,
            None => None,
        },
        _ => None,
    };
    Ok(scope.filter(|scope| {
        crate::scope::parse_scope(scope).is_ok_and(|parts| parts.verse_id == verse_id)
    }))
}

/// Whether `version`'s author held the key's required level at the version's hlc (no current-role conjunction).
pub(crate) async fn admissible(
    db: &Db,
    verse_id: &str,
    table: &str,
    rid: &str,
    version: &FieldVersion,
    view: &VerseView,
) -> anyhow::Result<bool> {
    if let Some(verdict) = view.settled(version) {
        return Ok(verdict);
    }
    Ok(match stored_row_scope(db, verse_id, table, rid).await? {
        Some(scope) => view.admits_at(table, &scope, version),
        None => false,
    })
}
