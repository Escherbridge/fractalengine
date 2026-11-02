//! Manager-authorized hierarchy transactions; see src/AGENTS.md §hierarchy-mutations.

use fe_runtime::messages::EntityType;
use serde_json::{json, Value};
use tracing::instrument;

use crate::repo::Db;

/// A stored ancestry and authorization snapshot fenced at transaction use.
pub(crate) struct ParentMutation {
    guards: Vec<String>,
    bindings: Vec<(String, Value)>,
    scope: String,
    role: fe_policy::RoleLevel,
}

impl ParentMutation {
    pub(crate) fn into_parts(self) -> (Vec<String>,Vec<(String,Value)>) {
        (self.guards,self.bindings)
    }

    /// Rebind this trusted fence for one member of a multi-target read transaction.
    pub(crate) fn scoped_read_fence(self, index: usize) -> (Vec<String>, Vec<(String, Value)>) {
        let prefix = format!("scoped_query_{index}_");
        let guards = self
            .guards
            .into_iter()
            .map(|sql| sql.replace("$entity_", &format!("${prefix}")))
            .collect();
        let bindings = self
            .bindings
            .into_iter()
            .map(|(name, value)| (name.replacen("entity_", &prefix, 1), value))
            .collect();
        (guards, bindings)
    }

    pub(crate) async fn commit(
        self,
        db: &Db,
        statements: &[&str],
        bindings: Vec<(String, Value)>,
    ) -> anyhow::Result<()> {
        self.execute(db, statements, bindings).await.map(|_| ())
    }

    async fn execute(
        self,
        db: &Db,
        statements: &[&str],
        bindings: Vec<(String, Value)>,
    ) -> anyhow::Result<surrealdb::IndexedResults> {
        let mut script = String::from("BEGIN TRANSACTION;\n");
        for statement in self
            .guards
            .iter()
            .map(String::as_str)
            .chain(statements.iter().copied())
        {
            script.push_str(statement.trim().trim_end_matches(';'));
            script.push_str(";\n");
        }
        script.push_str("COMMIT TRANSACTION;");
        let mut query = db.query(script);
        for binding in self.bindings.into_iter().chain(bindings) {
            query = query.bind(binding);
        }
        Ok(query.await?.check()?)
    }
}

/// Resolve the target and require Manager using the DB thread's local identity.
pub(crate) async fn prepare_parent_mutation(
    db: &Db,
    entity_type: EntityType,
    entity_id: &str,
    local_did: &str,
) -> anyhow::Result<ParentMutation> {
    prepare_parent_access(
        db,
        entity_type,
        entity_id,
        local_did,
        fe_policy::RoleLevel::Manager,
    )
    .await
}

pub(crate) async fn prepare_parent_access(
    db: &Db,
    entity_type: EntityType,
    entity_id: &str,
    local_did: &str,
    required: fe_policy::RoleLevel,
) -> anyhow::Result<ParentMutation> {
    let scope = match entity_type {
        EntityType::Verse => super::crud::resolve_verse_scope_handler(db, entity_id).await?,
        EntityType::Fractal => super::crud::resolve_fractal_scope_handler(db, entity_id).await?,
        EntityType::Petal => super::crud::resolve_petal_scope_handler(db, entity_id).await?,
    }
    .ok_or_else(|| anyhow::anyhow!("hierarchy target not found or not permitted"))?;
    let parts = crate::parse_scope(&scope)?;
    let target = match entity_type {
        EntityType::Verse => Some(parts.verse_id.as_str()),
        EntityType::Fractal => parts.fractal_id.as_deref(),
        EntityType::Petal => parts.petal_id.as_deref(),
    };
    anyhow::ensure!(target == Some(entity_id), "invalid hierarchy target scope");

    prepare_scope_permission(db,&scope,local_did,required).await
}

/// Capture and fence one stored scope's authority at the requested role floor.
pub(crate) async fn prepare_scope_permission(
    db: &Db,scope: &str,local_did: &str,required: fe_policy::RoleLevel,
) -> anyhow::Result<ParentMutation> {
    let parts=crate::parse_scope(scope)?;
    let mut scopes = vec![scope.to_string()];
    while let Some(parent) = crate::parent_scope(scopes.last().unwrap()) {
        scopes.push(parent);
    }
    let mut snapshot = db.query(
        "SELECT verse_id, created_by, default_access FROM verse WHERE verse_id = $entity_verse; \
         SELECT scope, role FROM role WHERE peer_did = $entity_actor AND scope IN $entity_scopes ORDER BY scope, role",
    )
        .bind(("entity_verse", parts.verse_id.clone()))
        .bind(("entity_actor", local_did.to_string()))
        .bind(("entity_scopes", scopes.clone()))
        .await?.check()?;
    let verse: Vec<Value> = snapshot.take(0)?;
    let roles: Vec<Value> = snapshot.take(1)?;
    anyhow::ensure!(verse.len() == 1, "hierarchy ancestry not found");
    let role = require_snapshot_role(&verse[0], &roles, &scopes, local_did, required)?;

    let mut guards = vec![
        "IF (SELECT verse_id, created_by, default_access FROM verse WHERE verse_id = $entity_verse) != $entity_verse_snapshot { THROW 'hierarchy authority changed'; }".into(),
        "IF (SELECT scope, role FROM role WHERE peer_did = $entity_actor AND scope IN $entity_scopes ORDER BY scope, role) != $entity_role_snapshot { THROW 'hierarchy authority changed'; }".into(),
    ];
    let mut bindings = vec![
        ("entity_verse".into(), json!(parts.verse_id)),
        ("entity_actor".into(), json!(local_did)),
        ("entity_scopes".into(), json!(scopes)),
        ("entity_verse_snapshot".into(), json!(verse)),
        ("entity_role_snapshot".into(), json!(roles)),
    ];
    if let Some(fractal) = parts.fractal_id {
        guards.push("IF array::len((SELECT VALUE fractal_id FROM fractal WHERE fractal_id = $entity_fractal)) != 1 { THROW 'ambiguous hierarchy identity'; }".into());
        guards.push("IF array::len((SELECT VALUE fractal_id FROM fractal WHERE fractal_id = $entity_fractal AND verse_id = $entity_verse)) != 1 { THROW 'hierarchy ancestry changed'; }".into());
        bindings.push(("entity_fractal".into(), json!(fractal)));
    }
    if let Some(petal) = parts.petal_id {
        guards.push("IF array::len((SELECT VALUE petal_id FROM petal WHERE petal_id = $entity_petal)) != 1 { THROW 'ambiguous hierarchy identity'; }".into());
        guards.push("IF array::len((SELECT VALUE petal_id FROM petal WHERE petal_id = $entity_petal AND fractal_id = $entity_fractal)) != 1 { THROW 'hierarchy ancestry changed'; }".into());
        bindings.push(("entity_petal".into(), json!(petal)));
    }
    Ok(ParentMutation {
        guards,
        bindings,
        scope: scope.to_owned(),
        role,
    })
}

/// Authorize precisely the captured values that the transaction will fence.
pub(crate) fn require_snapshot_role(
    verse: &Value,
    roles: &[Value],
    scopes: &[String],
    local_did: &str,
    required: fe_policy::RoleLevel,
) -> anyhow::Result<fe_policy::RoleLevel> {
    let mut ordered_roles = Vec::with_capacity(scopes.len());
    for scope in scopes {
        let mut at_scope = roles
            .iter()
            .filter(|row| row["scope"].as_str() == Some(scope.as_str()));
        if let Some(row) = at_scope.next() {
            anyhow::ensure!(at_scope.next().is_none(), "ambiguous hierarchy authority");
            ordered_roles.push(
                row["role"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("invalid stored role"))?,
            );
        }
    }
    let role = crate::role_manager::resolve_role_snapshot(
        local_did,
        verse["created_by"].as_str().unwrap_or(""),
        verse["default_access"].as_str().unwrap_or("viewer"),
        ordered_roles,
    );
    anyhow::ensure!(
        role.is_at_least(required),
        "Hierarchy access refused: stored role {role} is below {required}"
    );
    Ok(role)
}

/// Read settings only after fencing stored ancestry and Viewer authority.
pub(crate) async fn get_entity_details_handler(
    db: &Db,
    entity_type: EntityType,
    entity_id: &str,
    local_did: &str,
) -> anyhow::Result<fe_runtime::messages::EntityDetails> {
    let prepared = prepare_parent_access(
        db,
        entity_type,
        entity_id,
        local_did,
        fe_policy::RoleLevel::Viewer,
    )
    .await?;
    let scope = prepared.scope.clone();
    let role = prepared.role.to_string();
    let parts = crate::parse_scope(&scope)?;
    let result_index = prepared.guards.len() + 1;
    let (table, id) = table_identity(entity_type);
    let columns = match entity_type {
        EntityType::Verse => "name, default_access",
        EntityType::Fractal => "name, description",
        EntityType::Petal => "name",
    };
    let mut response = prepared
        .execute(
            db,
            &[&format!(
                "SELECT {columns} FROM {table} WHERE {id} = $entity_target"
            )],
            vec![("entity_target".into(), json!(entity_id))],
        )
        .await?;
    let rows: Vec<Value> = response.take(result_index)?;
    anyhow::ensure!(
        rows.len() == 1,
        "hierarchy details target is missing or ambiguous"
    );
    let row = &rows[0];
    let name = row["name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("invalid stored hierarchy name"))?
        .to_owned();
    let optional_text = |key: &str| -> anyhow::Result<Option<String>> {
        match row.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => {
                anyhow::ensure!(
                    value.len() <= 65_536,
                    "stored hierarchy field exceeds editor limit"
                );
                Ok(Some(value.clone()))
            }
            Some(_) => anyhow::bail!("invalid stored hierarchy field"),
        }
    };
    anyhow::ensure!(
        name.len() <= 65_536,
        "stored hierarchy name exceeds editor limit"
    );
    let default_access = optional_text("default_access")?;
    anyhow::ensure!(
        matches!(default_access.as_deref(), None | Some("viewer" | "none")),
        "invalid stored default access setting"
    );
    Ok(fe_runtime::messages::EntityDetails {
        entity_type,
        entity_id: entity_id.into(),
        verse_id: parts.verse_id,
        fractal_id: match entity_type {
            EntityType::Petal => parts.fractal_id,
            _ => None,
        },
        scope,
        name,
        default_access,
        description: optional_text("description")?,
        role,
    })
}

fn table_identity(entity_type: EntityType) -> (&'static str, &'static str) {
    match entity_type {
        EntityType::Verse => ("verse", "verse_id"),
        EntityType::Fractal => ("fractal", "fractal_id"),
        EntityType::Petal => ("petal", "petal_id"),
    }
}

/// Rename a stored hierarchy target as its local Manager.
#[instrument(skip(db))]
pub(crate) async fn rename_entity_handler(
    db: &Db,
    entity_type: EntityType,
    entity_id: &str,
    new_name: &str,
    local_did: &str,
) -> Result<(), String> {
    let result = async {
        anyhow::ensure!(!new_name.trim().is_empty(), "name must not be empty");
        let prepared = prepare_parent_mutation(db, entity_type, entity_id, local_did).await?;
        let (table, id) = table_identity(entity_type);
        prepared
            .commit(
                db,
                &[&format!(
                    "UPDATE {table} SET name = $entity_name WHERE {id} = $entity_target"
                )],
                vec![
                    ("entity_target".into(), json!(entity_id)),
                    ("entity_name".into(), json!(new_name)),
                ],
            )
            .await
    }
    .await;
    result.map_err(|e| format!("Rename failed: {e}"))
}

/// Physically delete one hierarchy subtree in a single transaction.
#[instrument(skip(db))]
pub(crate) async fn delete_entity_handler(
    db: &Db,
    entity_type: EntityType,
    entity_id: &str,
    local_did: &str,
) -> Result<(), String> {
    let result = async {
        let prepared = prepare_parent_mutation(db, entity_type, entity_id, local_did).await?;
        let statements = match entity_type {
            EntityType::Verse => vec![
                "LET $entity_delete_fractals = (SELECT VALUE fractal_id FROM fractal WHERE verse_id = $entity_target)",
                "IF array::len((SELECT VALUE fractal_id FROM fractal WHERE fractal_id IN $entity_delete_fractals)) != array::len(array::distinct($entity_delete_fractals)) { THROW 'ambiguous cascade identity'; }",
                "LET $entity_delete_petals = (SELECT VALUE petal_id FROM petal WHERE fractal_id IN $entity_delete_fractals)",
                "IF array::len((SELECT VALUE petal_id FROM petal WHERE petal_id IN $entity_delete_petals)) != array::len(array::distinct($entity_delete_petals)) { THROW 'ambiguous cascade identity'; }",
                "DELETE node WHERE petal_id IN $entity_delete_petals",
                "DELETE petal WHERE petal_id IN $entity_delete_petals",
                "DELETE fractal WHERE fractal_id IN $entity_delete_fractals",
                "DELETE role WHERE scope IN $entity_deleted_scopes",
                "DELETE verse_member WHERE verse_id = $entity_target",
                "DELETE verse WHERE verse_id = $entity_target",
            ],
            EntityType::Fractal => vec![
                "LET $entity_delete_petals = (SELECT VALUE petal_id FROM petal WHERE fractal_id = $entity_target)",
                "IF array::len((SELECT VALUE petal_id FROM petal WHERE petal_id IN $entity_delete_petals)) != array::len(array::distinct($entity_delete_petals)) { THROW 'ambiguous cascade identity'; }",
                "DELETE node WHERE petal_id IN $entity_delete_petals",
                "DELETE petal WHERE petal_id IN $entity_delete_petals",
                "DELETE fractal WHERE fractal_id = $entity_target",
            ],
            EntityType::Petal => vec![
                "DELETE node WHERE petal_id = $entity_target",
                "DELETE petal WHERE petal_id = $entity_target",
            ],
        };
        // Retain the old verse-only role cleanup, comparing parsed identifiers exactly.
        let mut response = db.query("SELECT scope FROM role").await?.check()?;
        let roles: Vec<Value> = response.take(0)?;
        let deleted_scopes: Vec<&str> = roles.iter().filter_map(|row| row["scope"].as_str())
            .filter(|scope| crate::parse_scope(scope).is_ok_and(|p| p.verse_id == entity_id))
            .collect();
        prepared.commit(db, &statements, vec![
            ("entity_target".into(), json!(entity_id)),
            ("entity_deleted_scopes".into(), json!(deleted_scopes)),
        ]).await
    }.await;
    result.map_err(|e| format!("Delete failed: {e}"))
}

/// Update a stored fractal's description as its local Manager.
#[instrument(skip(db))]
pub(crate) async fn update_fractal_description_handler(
    db: &Db,
    fractal_id: &str,
    description: &str,
    local_did: &str,
) -> anyhow::Result<()> {
    prepare_parent_mutation(db, EntityType::Fractal, fractal_id, local_did).await?
        .commit(db, &["UPDATE fractal SET description = $entity_description WHERE fractal_id = $entity_target"], vec![
            ("entity_target".into(), json!(fractal_id)),
            ("entity_description".into(), json!(description)),
        ]).await
}
