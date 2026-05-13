//! Helper for executing `fe_query::BuiltQuery` against a SurrealDB handle.
//!
//! Centralises the bind-loop so every call-site doesn't repeat it.

use fe_query::BuiltQuery;

use crate::repo::Db;

/// Execute a [`BuiltQuery`] against the database, binding all parameters.
///
/// Returns the raw `surrealdb::IndexedResults` so callers can `.take::<T>(0)`
/// as needed.
pub(crate) async fn exec_query(
    db: &Db,
    q: &BuiltQuery,
) -> Result<surrealdb::IndexedResults, surrealdb::Error> {
    let mut query = db.query(&q.sql);
    for (name, val) in &q.params {
        query = query.bind((name.clone(), val.clone()));
    }
    query.await
}
