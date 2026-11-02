//! Guarded read-only facade over the API's direct SurrealDB connection.
//! Rationale, the one write exception, and why `Deref` is deliberately
//! absent: see `fe-api/AGENTS.md` §db-reader.

use surrealdb::engine::local::Db as LocalEngine;
use surrealdb::method::Query;
use surrealdb::Surreal;

/// The API's direct (non-DB-thread) SurrealDB connection type — what
/// [`ReadDb`] wraps and [`ReadDb::writable`]'s return type.
pub type Db = Surreal<LocalEngine>;

/// A read-only facade over [`Db`].
///
/// SurrealDB's mutating methods (`create`/`update`/`delete`/`insert`/
/// `upsert`) all take `&self`, not `&mut self` — the client is an async
/// handle over a channel, not a borrow-checked value — so the usual "expose
/// `Deref` but withhold `DerefMut`" guard cannot block them: autoderef would
/// still resolve `.delete()` through any `Deref<Target = Db>`. This type
/// guards instead by never implementing `Deref` and re-exposing only
/// [`ReadDb::query`] — the one method every existing `db_reader` call site
/// already used (every other inherent method on [`Db`] is simply
/// unreachable through this type, not merely discouraged).
///
/// ```compile_fail
/// # async fn f(db: fe_api::db_reader::ReadDb) {
/// // E0599: no method named `delete` on `ReadDb`, and no `Deref` chain
/// // leads to one — this must fail to compile.
/// db.delete(("table", "id"));
/// # }
/// ```
#[derive(Clone)]
pub struct ReadDb(Db, bool);

impl ReadDb {
    /// Wrap an existing connection as read-only.
    pub fn new(db: Db) -> Self {
        Self(db, false)
    }

    pub(crate) fn with_command_writes(&self) -> Self { Self(self.0.clone(), true) }

    pub(crate) fn command_writes_only(&self) -> bool { self.1 }

    /// Run a SurrealQL statement — mirrors [`Db::query`] exactly. Still
    /// executes whatever SQL it is given, including a raw `CREATE`/`DELETE`
    /// string (that guard is the existing SELECT-only validator in
    /// `query_guard.rs`, unchanged by this slice); the invariant *this* type
    /// adds is narrower and type-level: a caller cannot reach the *typed*
    /// `create`/`update`/`delete`/`insert`/`upsert` builder methods through
    /// it, so a new handler cannot reach for them by accident.
    pub fn query<'a>(
        &'a self,
        query: impl Into<std::borrow::Cow<'a, str>>,
    ) -> Query<'a, LocalEngine> {
        self.0.query(query)
    }

    /// Escape hatch for the one documented write exception (§iot-readings:
    /// the API-thread append-only insert path in `iot.rs`). Crate-private so
    /// no code outside fe-api can turn a `ReadDb` back into a writer; grep for
    /// existing callers before adding a new one here.
    pub(crate) fn writable(&self) -> &Db {
        &self.0
    }
}
