//! SPEC-4 §3.4-§3.6 apply markers and the single atomic projection-plus-marker commit.
//! Rationale, binding projection, and the §3.6 fallback: `canon_log/AGENTS.md`
//! §atomic-apply and §binding-projection.

use fe_canonical_log::cbor::CborValue;
use fe_canonical_log::envelope::Hash32;
use fe_canonical_log::materialize::identity::ProjectionIdentity;
use fe_canonical_log::materialize::traits::ProjectionMutation;

use crate::canon_log::{identifier_to_hex, local_stamp, op_id_to_hex, StorageError};
use crate::repo::Db;

/// Prefix reserved for the bindings this module adds to a materializer's statements.
const RESERVED_BINDING_PREFIX: &str = "__canon_";

/// The `{materializer_id}:{version}:{branch_id}:{op_id_hex}` durable marker key (§3.4).
pub fn marker_key(identity: &ProjectionIdentity, op_id: Hash32) -> String {
    format!(
        "{}:{}:{}:{}",
        identity.materializer_version.materializer_id,
        identity.materializer_version.version,
        identifier_to_hex(identity.branch_id),
        op_id_to_hex(op_id)
    )
}

/// A projection mutation that is allowed to advance an apply marker.
///
/// `ProjectionMutation::ReferencedArtifactUnavailable` deliberately has no representation here.
/// SPEC-4 §5 requires that state to leave the projection position pending replay, so the only
/// way to obtain a `CommittableMutation` is [`CommittableMutation::try_from_projection`], which
/// refuses it. The marker cannot advance on an availability gap because there is no value to
/// hand `commit_with_marker` that would make it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommittableMutation {
    /// Statements and bindings to apply to the projection.
    Apply {
        /// Storage statements produced by the materializer.
        statements: Vec<String>,
        /// Named bindings in the log crate's canonical value model.
        bindings: Vec<(String, CborValue)>,
    },
    /// A §4.6 recorded exclusion: evidence retained, nothing projected.
    Excluded {
        /// Why the operation is excluded.
        reason: String,
    },
}

/// Why a [`ProjectionMutation`] may not advance an apply marker.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum NotCommittable {
    /// §5 `referenced_artifact_unavailable`: retryable availability state, never a decision.
    #[error("operation references artifact {artifact_id:?}, which is not locally resolvable")]
    ReferencedArtifactUnavailable {
        /// The artifact the operation names.
        artifact_id: Hash32,
    },
}

impl CommittableMutation {
    /// Accepts an `Apply` or `Excluded` reduction; refuses an availability state.
    pub fn try_from_projection(mutation: &ProjectionMutation) -> Result<Self, NotCommittable> {
        match mutation {
            ProjectionMutation::Apply {
                statements,
                bindings,
            } => Ok(Self::Apply {
                statements: statements.clone(),
                bindings: bindings.clone(),
            }),
            ProjectionMutation::Excluded { reason } => Ok(Self::Excluded {
                reason: reason.clone(),
            }),
            ProjectionMutation::ReferencedArtifactUnavailable { artifact_id } => {
                Err(NotCommittable::ReferencedArtifactUnavailable {
                    artifact_id: *artifact_id,
                })
            }
        }
    }

    /// The durable `disposition` this mutation records.
    pub fn disposition(&self) -> String {
        match self {
            Self::Apply { .. } => "applied".to_string(),
            Self::Excluded { reason } => format!("excluded:{reason}"),
        }
    }
}

/// Every reason an atomic apply does not commit.
#[derive(Debug, thiserror::Error)]
pub enum ApplyMarkerError {
    /// The substrate failed; §3.5 requires replay to apply the operation again.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// A materializer binding collided with the marker bindings this module adds.
    #[error("materializer binding `{0}` uses the reserved `__canon_` prefix")]
    ReservedBinding(String),
    /// A binding value has no deterministic SurrealDB projection.
    #[error("binding `{name}` cannot be projected into a storage value: {detail}")]
    UnsupportedBinding {
        /// Binding whose value could not be projected.
        name: String,
        /// Why it could not.
        detail: String,
    },
    /// A materializer emitted an empty or whitespace-only statement.
    #[error("materializer emitted an empty statement at index {0}")]
    EmptyStatement(usize),
}

/// What one atomic apply did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkerCommit {
    /// Projection statements and the marker committed together (§3.5).
    Applied,
    /// Only the marker committed, recording a §4.6 exclusion.
    Excluded,
    /// A marker already existed; §3.4 makes re-applying a no-op.
    AlreadyProcessed {
        /// The disposition the existing marker records.
        disposition: String,
    },
}

/// Durable apply markers for one SurrealDB projection (§3.4).
pub struct SurrealApplyMarkerStore {
    db: Db,
}

impl SurrealApplyMarkerStore {
    /// Binds the store to an open database handle.
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// The handle this store reads and writes through.
    pub fn database(&self) -> &Db {
        &self.db
    }

    /// Whether this projection has already processed `op_id`.
    ///
    /// True for both `applied` and `excluded:*`: SPEC-4 §4.2 makes an operation eligible for its
    /// children once it has been *processed*, and a recorded exclusion is a processed operation.
    /// Use [`Self::disposition_of`] when the distinction matters.
    pub async fn is_applied(
        &self,
        identity: &ProjectionIdentity,
        op_id: Hash32,
    ) -> Result<bool, StorageError> {
        Ok(self.disposition_of(identity, op_id).await?.is_some())
    }

    /// The disposition recorded for `op_id` under this projection, if any.
    pub async fn disposition_of(
        &self,
        identity: &ProjectionIdentity,
        op_id: Hash32,
    ) -> Result<Option<String>, StorageError> {
        let mut response = self
            .db
            .query(
                "SELECT disposition FROM materializer_apply_marker \
                 WHERE marker_key = $key LIMIT 1",
            )
            .bind(("key", marker_key(identity, op_id)))
            .await
            .map_err(|error| StorageError::query("apply marker read", error))?
            .check()
            .map_err(|error| StorageError::query("apply marker read", error))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|error| StorageError::query("apply marker read", error))?;
        Ok(rows
            .first()
            .and_then(|row| row.get("disposition"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned))
    }

    /// Commits a projection mutation and its apply marker in ONE SurrealQL transaction (§3.5).
    ///
    /// An `Excluded` mutation writes only the marker. If the substrate cannot honour the
    /// transaction boundary, §3.6 forbids claiming exactly-once projection: the caller must
    /// rebuild from the last valid checkpoint or empty state
    /// (`rebuild::rebuild_projection`), which is why this returns an error rather than a
    /// partially-applied success.
    pub async fn commit_with_marker(
        &self,
        identity: &ProjectionIdentity,
        op_id: Hash32,
        mutation: &CommittableMutation,
    ) -> Result<MarkerCommit, ApplyMarkerError> {
        if let Some(disposition) = self.disposition_of(identity, op_id).await? {
            return Ok(MarkerCommit::AlreadyProcessed { disposition });
        }

        let statements: &[String] = match mutation {
            CommittableMutation::Apply { statements, .. } => statements.as_slice(),
            CommittableMutation::Excluded { .. } => &[],
        };
        let bindings: &[(String, CborValue)] = match mutation {
            CommittableMutation::Apply { bindings, .. } => bindings.as_slice(),
            CommittableMutation::Excluded { .. } => &[],
        };

        let mut script = String::from("BEGIN TRANSACTION;\n");
        for (index, statement) in statements.iter().enumerate() {
            let trimmed = statement.trim().trim_end_matches(';').trim();
            if trimmed.is_empty() {
                return Err(ApplyMarkerError::EmptyStatement(index));
            }
            script.push_str(trimmed);
            script.push_str(";\n");
        }
        script.push_str(
            "CREATE materializer_apply_marker CONTENT {\n\
             marker_key: $__canon_marker_key,\n\
             materializer_id: $__canon_materializer_id,\n\
             materializer_version: $__canon_materializer_version,\n\
             branch_id: $__canon_branch_id,\n\
             op_id_hex: $__canon_op_id_hex,\n\
             disposition: $__canon_disposition,\n\
             applied_at_hlc: $__canon_applied_at_hlc\n\
             };\n",
        );
        script.push_str("COMMIT TRANSACTION;");

        let mut query = self.db.query(script);
        for (name, value) in bindings {
            if name.starts_with(RESERVED_BINDING_PREFIX) {
                return Err(ApplyMarkerError::ReservedBinding(name.clone()));
            }
            let json =
                cbor_to_json(value).map_err(|detail| ApplyMarkerError::UnsupportedBinding {
                    name: name.clone(),
                    detail,
                })?;
            query = query.bind((name.clone(), json));
        }
        query = query
            .bind(("__canon_marker_key", marker_key(identity, op_id)))
            .bind((
                "__canon_materializer_id",
                identity.materializer_version.materializer_id.clone(),
            ))
            .bind((
                "__canon_materializer_version",
                i64::from(identity.materializer_version.version),
            ))
            .bind(("__canon_branch_id", identifier_to_hex(identity.branch_id)))
            .bind(("__canon_op_id_hex", op_id_to_hex(op_id)))
            .bind(("__canon_disposition", mutation.disposition()))
            .bind(("__canon_applied_at_hlc", local_stamp()));

        query
            .await
            .map_err(|error| StorageError::query("atomic apply", error))?
            .check()
            .map_err(|error| StorageError::query("atomic apply", error))?;

        Ok(match mutation {
            CommittableMutation::Apply { .. } => MarkerCommit::Applied,
            CommittableMutation::Excluded { .. } => MarkerCommit::Excluded,
        })
    }

    /// Every operation this projection has processed.
    pub async fn processed_op_ids(
        &self,
        identity: &ProjectionIdentity,
    ) -> Result<Vec<Hash32>, StorageError> {
        let mut response = self
            .db
            .query(
                "SELECT op_id_hex FROM materializer_apply_marker \
                 WHERE materializer_id = $id AND materializer_version = $version \
                 AND branch_id = $branch ORDER BY op_id_hex ASC",
            )
            .bind(("id", identity.materializer_version.materializer_id.clone()))
            .bind(("version", i64::from(identity.materializer_version.version)))
            .bind(("branch", identifier_to_hex(identity.branch_id)))
            .await
            .map_err(|error| StorageError::query("apply marker scan", error))?
            .check()
            .map_err(|error| StorageError::query("apply marker scan", error))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|error| StorageError::query("apply marker scan", error))?;
        let mut op_ids = Vec::with_capacity(rows.len());
        for row in rows {
            let text = row
                .get("op_id_hex")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    StorageError::malformed("apply marker scan", "op_id_hex is not a string")
                })?;
            op_ids.push(crate::canon_log::op_id_from_hex(text).ok_or_else(|| {
                StorageError::malformed("apply marker scan", format!("bad op_id hex {text}"))
            })?);
        }
        Ok(op_ids)
    }

    /// Drops every marker for one projection identity, for a §6.1 empty-state rebuild.
    ///
    /// Never used to "repair" a projection: §5 note 4 forbids advancing or retracting a marker
    /// by hand. The only caller is the rebuild path, which then replays from verified history.
    pub async fn clear_projection(
        &self,
        identity: &ProjectionIdentity,
    ) -> Result<(), StorageError> {
        self.db
            .query(
                "DELETE FROM materializer_apply_marker \
                 WHERE materializer_id = $id AND materializer_version = $version \
                 AND branch_id = $branch",
            )
            .bind(("id", identity.materializer_version.materializer_id.clone()))
            .bind(("version", i64::from(identity.materializer_version.version)))
            .bind(("branch", identifier_to_hex(identity.branch_id)))
            .await
            .map_err(|error| StorageError::query("apply marker clear", error))?
            .check()
            .map_err(|error| StorageError::query("apply marker clear", error))?;
        Ok(())
    }
}

/// Projects the log crate's canonical value model onto a SurrealDB binding value.
///
/// Deterministic by construction: byte strings become lowercase hex, integer map keys become
/// their decimal text, and nothing consults a clock, a locale, or a float. A materializer that
/// needs different bytes-to-column semantics emits its own `string::` conversion in the
/// statement rather than changing this projection.
pub fn cbor_to_json(value: &CborValue) -> Result<serde_json::Value, String> {
    Ok(match value {
        CborValue::Uint(number) => serde_json::Value::from(*number),
        CborValue::NegInt(number) => serde_json::Value::from(*number),
        CborValue::Bytes(bytes) => serde_json::Value::String(hex::encode(bytes)),
        CborValue::Text(text) => serde_json::Value::String(text.clone()),
        CborValue::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(cbor_to_json)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        CborValue::Map(entries) => {
            let mut object = serde_json::Map::with_capacity(entries.len());
            for (key, entry) in entries {
                let name = match key {
                    CborValue::Uint(number) => number.to_string(),
                    CborValue::Text(text) => text.clone(),
                    other => return Err(format!("map key {other:?} is not text or an integer")),
                };
                object.insert(name, cbor_to_json(entry)?);
            }
            serde_json::Value::Object(object)
        }
        CborValue::Null => serde_json::Value::Null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_canonical_log::envelope::Identifier32;
    use fe_canonical_log::materialize::identity::MaterializerVersion;

    fn identity() -> ProjectionIdentity {
        ProjectionIdentity::new(
            MaterializerVersion::new("scene-graph", 1),
            Identifier32([0x11; 32]),
        )
    }

    async fn memory_database() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("canon_log")
            .use_db("canon_log")
            .await
            .expect("ns/db");
        crate::schema::apply_all(&db).await.expect("apply schema");
        db.query("DEFINE TABLE IF NOT EXISTS atomicity_probe SCHEMALESS")
            .await
            .expect("probe table")
            .check()
            .expect("probe table check");
        db
    }

    async fn probe_row_count(db: &Db) -> usize {
        let mut response = db
            .query("SELECT * FROM atomicity_probe")
            .await
            .expect("probe select");
        let rows: Vec<serde_json::Value> = response.take(0).expect("probe rows");
        rows.len()
    }

    #[test]
    fn the_marker_key_separates_materializer_version_branch_and_operation() {
        let op_id = Hash32([0x22; 32]);
        let base = marker_key(&identity(), op_id);
        assert!(base.starts_with("scene-graph:1:"));
        assert!(base.ends_with(&op_id_to_hex(op_id)));

        let bumped = ProjectionIdentity::new(
            MaterializerVersion::new("scene-graph", 2),
            Identifier32([0x11; 32]),
        );
        assert_ne!(base, marker_key(&bumped, op_id));

        let other_branch = ProjectionIdentity::new(
            MaterializerVersion::new("scene-graph", 1),
            Identifier32([0x12; 32]),
        );
        assert_ne!(base, marker_key(&other_branch, op_id));
        assert_ne!(base, marker_key(&identity(), Hash32([0x23; 32])));
    }

    #[test]
    fn an_availability_state_has_no_committable_representation() {
        let artifact_id = Hash32([0x44; 32]);
        assert_eq!(
            CommittableMutation::try_from_projection(
                &ProjectionMutation::ReferencedArtifactUnavailable { artifact_id }
            ),
            Err(NotCommittable::ReferencedArtifactUnavailable { artifact_id })
        );
        assert!(
            CommittableMutation::try_from_projection(&ProjectionMutation::Excluded {
                reason: "disavowed author".to_string(),
            })
            .is_ok()
        );
    }

    #[test]
    fn dispositions_are_applied_or_prefixed_excluded() {
        let applied = CommittableMutation::Apply {
            statements: vec!["CREATE x".to_string()],
            bindings: Vec::new(),
        };
        assert_eq!(applied.disposition(), "applied");
        let excluded = CommittableMutation::Excluded {
            reason: "revoked-concurrent".to_string(),
        };
        assert_eq!(excluded.disposition(), "excluded:revoked-concurrent");
    }

    #[test]
    fn the_binding_projection_is_deterministic_and_float_free() {
        let value = CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Bytes(vec![0xde, 0xad])),
            (
                CborValue::Text("name".to_string()),
                CborValue::Array(vec![CborValue::Uint(7), CborValue::NegInt(-3)]),
            ),
            (CborValue::Uint(9), CborValue::Null),
        ]);
        let json = cbor_to_json(&value).expect("projection");
        assert_eq!(json["0"], serde_json::json!("dead"));
        assert_eq!(json["name"], serde_json::json!([7, -3]));
        assert!(json["9"].is_null());
        assert_eq!(json, cbor_to_json(&value).expect("projection"));
    }

    #[test]
    fn a_non_text_non_integer_map_key_has_no_projection() {
        let value = CborValue::Map(vec![(CborValue::Null, CborValue::Uint(1))]);
        assert!(cbor_to_json(&value).is_err());
    }

    #[tokio::test]
    async fn an_apply_and_its_marker_commit_together() {
        let db = memory_database().await;
        let store = SurrealApplyMarkerStore::new(db.clone());
        let identity = identity();
        let op_id = Hash32([0x31; 32]);

        let outcome = store
            .commit_with_marker(
                &identity,
                op_id,
                &CommittableMutation::Apply {
                    statements: vec!["CREATE atomicity_probe CONTENT { tag: $tag }".to_string()],
                    bindings: vec![("tag".to_string(), CborValue::Text("first".to_string()))],
                },
            )
            .await
            .expect("atomic apply");
        assert_eq!(outcome, MarkerCommit::Applied);
        assert_eq!(probe_row_count(&db).await, 1);
        assert_eq!(
            store.disposition_of(&identity, op_id).await.expect("read"),
            Some("applied".to_string())
        );

        // §3.4: re-applying an already-marked operation is a no-op with the same projection.
        let repeat = store
            .commit_with_marker(
                &identity,
                op_id,
                &CommittableMutation::Apply {
                    statements: vec!["CREATE atomicity_probe CONTENT { tag: $tag }".to_string()],
                    bindings: vec![("tag".to_string(), CborValue::Text("second".to_string()))],
                },
            )
            .await
            .expect("idempotent apply");
        assert_eq!(
            repeat,
            MarkerCommit::AlreadyProcessed {
                disposition: "applied".to_string()
            }
        );
        assert_eq!(probe_row_count(&db).await, 1);
    }

    /// SPEC-4 §3.5 against the real embedded engine. If this ever fails, §3.6 applies: this
    /// projection may NOT be described as exactly-once, and the caller must rebuild from the
    /// last valid checkpoint or empty state via `rebuild::rebuild_projection`.
    #[tokio::test]
    async fn a_failed_statement_rolls_back_the_projection_and_the_marker_together() {
        let db = memory_database().await;
        let store = SurrealApplyMarkerStore::new(db.clone());
        let identity = identity();
        let op_id = Hash32([0x32; 32]);

        let error = store
            .commit_with_marker(
                &identity,
                op_id,
                &CommittableMutation::Apply {
                    statements: vec![
                        "CREATE atomicity_probe CONTENT { tag: 'doomed' }".to_string(),
                        "THROW 'induced materialization failure'".to_string(),
                    ],
                    bindings: Vec::new(),
                },
            )
            .await
            .expect_err("a throwing statement must fail the whole commit");
        assert!(matches!(error, ApplyMarkerError::Storage(_)), "{error}");

        assert_eq!(
            probe_row_count(&db).await,
            0,
            "SPEC-4 §3.5: a failed apply must leave the projection unchanged; \
             if the embedded engine cannot honour this, §3.6 requires rebuild-from-checkpoint \
             instead of an exactly-once claim"
        );
        assert_eq!(
            store.disposition_of(&identity, op_id).await.expect("read"),
            None,
            "SPEC-4 §3.5: the apply marker must not advance when the projection did not commit"
        );
    }

    #[tokio::test]
    async fn an_exclusion_writes_only_the_marker() {
        let db = memory_database().await;
        let store = SurrealApplyMarkerStore::new(db.clone());
        let identity = identity();
        let op_id = Hash32([0x33; 32]);

        let outcome = store
            .commit_with_marker(
                &identity,
                op_id,
                &CommittableMutation::Excluded {
                    reason: "disavowed author".to_string(),
                },
            )
            .await
            .expect("exclusion commit");
        assert_eq!(outcome, MarkerCommit::Excluded);
        assert_eq!(probe_row_count(&db).await, 0);
        assert_eq!(
            store.disposition_of(&identity, op_id).await.expect("read"),
            Some("excluded:disavowed author".to_string())
        );
        assert!(store.is_applied(&identity, op_id).await.expect("read"));
    }

    #[tokio::test]
    async fn a_reserved_binding_name_is_refused_before_anything_commits() {
        let db = memory_database().await;
        let store = SurrealApplyMarkerStore::new(db.clone());
        let identity = identity();
        let op_id = Hash32([0x34; 32]);

        let error = store
            .commit_with_marker(
                &identity,
                op_id,
                &CommittableMutation::Apply {
                    statements: vec!["CREATE atomicity_probe CONTENT { tag: 'x' }".to_string()],
                    bindings: vec![(
                        "__canon_disposition".to_string(),
                        CborValue::Text("applied".to_string()),
                    )],
                },
            )
            .await
            .expect_err("a reserved binding must be refused");
        assert!(
            matches!(error, ApplyMarkerError::ReservedBinding(_)),
            "{error}"
        );
        assert_eq!(probe_row_count(&db).await, 0);
        assert_eq!(
            store.disposition_of(&identity, op_id).await.expect("read"),
            None
        );
    }

    #[tokio::test]
    async fn clearing_one_projection_leaves_another_versions_markers_alone() {
        let db = memory_database().await;
        let store = SurrealApplyMarkerStore::new(db.clone());
        let version_one = identity();
        let version_two = ProjectionIdentity::new(
            MaterializerVersion::new("scene-graph", 2),
            Identifier32([0x11; 32]),
        );
        let op_id = Hash32([0x35; 32]);

        for identity in [&version_one, &version_two] {
            store
                .commit_with_marker(
                    identity,
                    op_id,
                    &CommittableMutation::Excluded {
                        reason: "fixture".to_string(),
                    },
                )
                .await
                .expect("marker");
        }

        store
            .clear_projection(&version_one)
            .await
            .expect("clear v1");
        assert!(!store.is_applied(&version_one, op_id).await.expect("read"));
        assert!(store.is_applied(&version_two, op_id).await.expect("read"));
        assert_eq!(
            store.processed_op_ids(&version_two).await.expect("scan"),
            vec![op_id]
        );
    }
}
