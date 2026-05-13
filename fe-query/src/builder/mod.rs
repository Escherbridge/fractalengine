pub mod filter;
pub mod render;
pub mod schema;
pub mod value;

use filter::Filter;
use render::{BuiltQuery, DeleteParts, InsertParts, ReturnClause, SelectParts, SortDir, UpdateParts};
use value::QueryValue;

// ── Typestate markers ──

/// Initial state — no operations applied yet.
#[derive(Debug, Clone)]
pub struct Initial;

/// Fields have been selected.
#[derive(Debug, Clone)]
pub struct Selecting;

/// A FROM table has been set (query is now buildable).
#[derive(Debug, Clone)]
pub struct FromSet;

// ── QueryBuilder (SELECT) ──

/// A LINQ-style fluent query builder for SurrealQL SELECT statements.
///
/// Uses typestate to enforce correct call order at compile time:
/// `Initial` -> `Selecting` -> `FromSet` -> (filter/order/limit/build).
///
/// All values are parameterized — no string interpolation of user data.
#[derive(Debug, Clone)]
pub struct QueryBuilder<S> {
    parts: SelectParts,
    _state: std::marker::PhantomData<S>,
}

impl QueryBuilder<Initial> {
    /// Start building a new SELECT query.
    pub fn new() -> Self {
        QueryBuilder {
            parts: SelectParts {
                fields: vec![],
                table: String::new(),
                filters: vec![],
                group_by: vec![],
                group_all: false,
                order_by: vec![],
                limit: None,
                offset: None,
            },
            _state: std::marker::PhantomData,
        }
    }

    /// Select specific fields. Pass `&["*"]` for all fields.
    pub fn select(mut self, fields: &[&str]) -> QueryBuilder<Selecting> {
        self.parts.fields = fields.iter().map(|f| f.to_string()).collect();
        QueryBuilder {
            parts: self.parts,
            _state: std::marker::PhantomData,
        }
    }
}

impl Default for QueryBuilder<Initial> {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryBuilder<Selecting> {
    /// Set the table to query from.
    pub fn from(mut self, table: &str) -> QueryBuilder<FromSet> {
        self.parts.table = table.to_string();
        QueryBuilder {
            parts: self.parts,
            _state: std::marker::PhantomData,
        }
    }
}

impl QueryBuilder<FromSet> {
    /// Add a filter predicate. Multiple filters are combined with AND.
    pub fn filter(mut self, f: Filter) -> Self {
        self.parts.filters.push(f);
        self
    }

    /// Alias for `filter` — reads better when chaining after a prior filter.
    pub fn and(self, f: Filter) -> Self {
        self.filter(f)
    }

    /// Add a GROUP BY clause with specific fields.
    pub fn group_by(mut self, fields: &[&str]) -> Self {
        self.parts.group_by = fields.iter().map(|f| f.to_string()).collect();
        self
    }

    /// Shorthand for `GROUP ALL` — aggregates all rows into a single group.
    pub fn group_all(mut self) -> Self {
        self.parts.group_all = true;
        self
    }

    /// Add an ORDER BY clause.
    pub fn order_by(mut self, field: &str, dir: SortDir) -> Self {
        self.parts.order_by.push((field.to_string(), dir));
        self
    }

    /// Set the maximum number of rows to return.
    pub fn limit(mut self, n: u64) -> Self {
        self.parts.limit = Some(n);
        self
    }

    /// Set the offset (START) for pagination.
    pub fn offset(mut self, n: u64) -> Self {
        self.parts.offset = Some(n);
        self
    }

    /// Build the final parameterized query.
    pub fn build(self) -> BuiltQuery {
        render::render_select(&self.parts)
    }
}

// ── InsertBuilder ──

/// A builder for SurrealQL CREATE ... CONTENT statements (append-only inserts).
#[derive(Debug, Clone)]
pub struct InsertBuilder {
    parts: InsertParts,
}

impl InsertBuilder {
    /// Start building an insert into the given table.
    pub fn insert_into(table: &str) -> Self {
        InsertBuilder {
            parts: InsertParts {
                table: table.to_string(),
                content: serde_json::Value::Null,
            },
        }
    }

    /// Set the content (JSON object) to insert.
    pub fn values(mut self, content: serde_json::Value) -> Self {
        self.parts.content = content;
        self
    }

    /// Build the final parameterized query.
    pub fn build(self) -> BuiltQuery {
        render::render_insert(&self.parts)
    }
}

// ── UpdateBuilder ──

/// A builder for SurrealQL UPDATE ... SET ... WHERE statements.
#[derive(Debug, Clone)]
pub struct UpdateBuilder {
    parts: UpdateParts,
}

impl UpdateBuilder {
    /// Start building an update on the given table.
    pub fn update(table: &str) -> Self {
        UpdateBuilder {
            parts: UpdateParts {
                table: table.to_string(),
                set_clauses: vec![],
                set_exprs: vec![],
                filters: vec![],
                return_clause: None,
            },
        }
    }

    /// Add a SET clause: `field = value`.
    pub fn set(mut self, field: &str, val: impl Into<QueryValue>) -> Self {
        self.parts.set_clauses.push((field.to_string(), val.into()));
        self
    }

    /// Add a SET clause with a raw SurrealQL expression (not parameterized).
    ///
    /// Example: `.set_expr("edit_seq", "edit_seq + 1")` renders as `edit_seq = edit_seq + 1`.
    pub fn set_expr(mut self, field: &str, expr: &str) -> Self {
        self.parts.set_exprs.push((field.to_string(), expr.to_string()));
        self
    }

    /// Add a WHERE filter. Multiple filters are combined with AND.
    pub fn where_clause(mut self, f: Filter) -> Self {
        self.parts.filters.push(f);
        self
    }

    /// Append `RETURN AFTER` to the statement.
    pub fn return_after(mut self) -> Self {
        self.parts.return_clause = Some(ReturnClause::After);
        self
    }

    /// Append `RETURN BEFORE` to the statement.
    pub fn return_before(mut self) -> Self {
        self.parts.return_clause = Some(ReturnClause::Before);
        self
    }

    /// Append `RETURN NONE` to the statement.
    pub fn return_none(mut self) -> Self {
        self.parts.return_clause = Some(ReturnClause::None);
        self
    }

    /// Build the final parameterized query.
    pub fn build(self) -> BuiltQuery {
        render::render_update(&self.parts)
    }
}

// ── DeleteBuilder ──

/// A builder for SurrealQL DELETE FROM ... WHERE statements.
#[derive(Debug, Clone)]
pub struct DeleteBuilder {
    parts: DeleteParts,
}

impl DeleteBuilder {
    /// Start building a delete from the given table.
    pub fn delete_from(table: &str) -> Self {
        DeleteBuilder {
            parts: DeleteParts {
                table: table.to_string(),
                filters: vec![],
            },
        }
    }

    /// Add a WHERE filter. Multiple filters are combined with AND.
    pub fn where_clause(mut self, f: Filter) -> Self {
        self.parts.filters.push(f);
        self
    }

    /// Build the final parameterized query.
    pub fn build(self) -> BuiltQuery {
        render::render_delete(&self.parts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fluent_select_roundtrip() {
        let q = QueryBuilder::new()
            .select(&["node_id", "petal_id", "display_name"])
            .from("node")
            .filter(Filter::eq("petal_id", "p-abc"))
            .and(Filter::gt("edit_seq", 5i64))
            .order_by("created_at", SortDir::Asc)
            .limit(100)
            .offset(0)
            .build();

        assert_eq!(
            q.sql,
            "SELECT node_id, petal_id, display_name FROM node \
             WHERE petal_id = $p0 AND edit_seq > $p1 \
             ORDER BY created_at ASC \
             LIMIT $p2 START $p3"
        );
        assert_eq!(q.params.len(), 4);
        assert_eq!(q.params[0], ("p0".into(), serde_json::json!("p-abc")));
        assert_eq!(q.params[1], ("p1".into(), serde_json::json!(5)));
        assert_eq!(q.params[2], ("p2".into(), serde_json::json!(100)));
        assert_eq!(q.params[3], ("p3".into(), serde_json::json!(0)));
    }

    #[test]
    fn select_all_no_filters() {
        let q = QueryBuilder::new()
            .select(&["*"])
            .from("node_log")
            .build();
        assert_eq!(q.sql, "SELECT * FROM node_log");
        assert!(q.params.is_empty());
    }

    #[test]
    fn select_with_spatial_filter() {
        let point = QueryValue::Point { x: 10.5, z: 20.3 };
        let q = QueryBuilder::new()
            .select(&["node_id", "display_name"])
            .from("node")
            .filter(Filter::d_within("position", point, 1000.0))
            .build();

        assert!(q.sql.contains("geo::distance(position, $p0) <= $p1"));
        assert_eq!(q.params.len(), 2);
    }

    #[test]
    fn select_with_compound_filter() {
        let compound = Filter::eq("petal_id", "p1")
            .and(Filter::gt("edit_seq", 0i64).or(Filter::is_not_null("asset_id")));
        let q = QueryBuilder::new()
            .select(&["*"])
            .from("node")
            .filter(compound)
            .limit(50)
            .build();

        assert!(q.sql.contains("(petal_id = $p0 AND (edit_seq > $p1 OR asset_id IS NOT NONE))"));
    }

    #[test]
    fn insert_builder_roundtrip() {
        let content = serde_json::json!({
            "log_id": "log-001",
            "node_id": "n1",
            "hlc_timestamp": 123456,
            "op": "transform_update",
            "source_did": "did:key:z6Mktest",
            "payload": { "position": [1.0, 2.0, 3.0] },
            "row_version": 1
        });
        let q = InsertBuilder::insert_into("node_log")
            .values(content.clone())
            .build();

        assert_eq!(q.sql, "CREATE node_log CONTENT $p0");
        assert_eq!(q.params.len(), 1);
        assert_eq!(q.params[0].1["log_id"], "log-001");
        assert_eq!(q.params[0].1["op"], "transform_update");
    }

    #[test]
    fn update_builder_roundtrip() {
        let q = UpdateBuilder::update("node")
            .set("display_name", "Renamed Node")
            .set("edit_seq", 42i64)
            .set("properties[\"color\"]", "red")
            .where_clause(Filter::eq("node_id", "n1"))
            .build();

        assert_eq!(
            q.sql,
            "UPDATE node SET display_name = $p0, edit_seq = $p1, properties[\"color\"] = $p2 WHERE node_id = $p3"
        );
        assert_eq!(q.params.len(), 4);
        assert_eq!(q.params[0].1, serde_json::json!("Renamed Node"));
        assert_eq!(q.params[1].1, serde_json::json!(42));
        assert_eq!(q.params[2].1, serde_json::json!("red"));
        assert_eq!(q.params[3].1, serde_json::json!("n1"));
    }

    #[test]
    fn update_builder_multiple_where() {
        let q = UpdateBuilder::update("node")
            .set("interactive", false)
            .where_clause(Filter::eq("petal_id", "p1"))
            .where_clause(Filter::gt("edit_seq", 10i64))
            .build();

        assert_eq!(
            q.sql,
            "UPDATE node SET interactive = $p0 WHERE petal_id = $p1 AND edit_seq > $p2"
        );
    }

    #[test]
    fn select_multiple_order_by() {
        let q = QueryBuilder::new()
            .select(&["*"])
            .from("node_log")
            .order_by("hlc_timestamp", SortDir::Desc)
            .order_by("row_version", SortDir::Asc)
            .build();

        assert!(q.sql.contains("ORDER BY hlc_timestamp DESC, row_version ASC"));
    }

    #[test]
    fn select_limit_without_offset() {
        let q = QueryBuilder::new()
            .select(&["node_id"])
            .from("node")
            .limit(10)
            .build();

        assert!(q.sql.contains("LIMIT $p0"));
        assert!(!q.sql.contains("START"));
    }

    // ── DeleteBuilder tests ──

    #[test]
    fn delete_basic() {
        let q = DeleteBuilder::delete_from("node").build();
        assert_eq!(q.sql, "DELETE FROM node");
        assert!(q.params.is_empty());
    }

    #[test]
    fn delete_with_where() {
        let q = DeleteBuilder::delete_from("node")
            .where_clause(Filter::eq("node_id", "n1"))
            .build();
        assert_eq!(q.sql, "DELETE FROM node WHERE node_id = $p0");
        assert_eq!(q.params.len(), 1);
        assert_eq!(q.params[0].1, serde_json::json!("n1"));
    }

    // ── GROUP BY / GROUP ALL tests ──

    #[test]
    fn select_group_all_with_aggregate() {
        let q = QueryBuilder::new()
            .select(&["count() AS total"])
            .from("node")
            .group_all()
            .build();
        assert_eq!(q.sql, "SELECT count() AS total FROM node GROUP ALL");
        assert!(q.params.is_empty());
    }

    #[test]
    fn select_group_by_fields() {
        let q = QueryBuilder::new()
            .select(&["petal_id", "count() AS cnt"])
            .from("node")
            .group_by(&["petal_id"])
            .build();
        assert_eq!(
            q.sql,
            "SELECT petal_id, count() AS cnt FROM node GROUP BY petal_id"
        );
        assert!(q.params.is_empty());
    }

    // ── SET expression tests ──

    #[test]
    fn update_set_expr() {
        let q = UpdateBuilder::update("node")
            .set_expr("edit_seq", "edit_seq + 1")
            .where_clause(Filter::eq("node_id", "n1"))
            .build();
        assert_eq!(
            q.sql,
            "UPDATE node SET edit_seq = edit_seq + 1 WHERE node_id = $p0"
        );
        assert_eq!(q.params.len(), 1);
        assert_eq!(q.params[0].1, serde_json::json!("n1"));
    }

    #[test]
    fn update_mixed_set_and_set_expr() {
        let q = UpdateBuilder::update("node")
            .set("display_name", "New Name")
            .set_expr("edit_seq", "edit_seq + 1")
            .where_clause(Filter::eq("node_id", "n1"))
            .build();
        assert_eq!(
            q.sql,
            "UPDATE node SET display_name = $p0, edit_seq = edit_seq + 1 WHERE node_id = $p1"
        );
        assert_eq!(q.params.len(), 2);
    }

    // ── RETURN clause tests ──

    #[test]
    fn update_return_after() {
        let q = UpdateBuilder::update("node")
            .set("display_name", "Foo")
            .where_clause(Filter::eq("node_id", "n1"))
            .return_after()
            .build();
        assert!(q.sql.ends_with("RETURN AFTER"));
    }

    #[test]
    fn update_return_before() {
        let q = UpdateBuilder::update("node")
            .set("display_name", "Foo")
            .return_before()
            .build();
        assert!(q.sql.ends_with("RETURN BEFORE"));
    }

    #[test]
    fn update_return_none() {
        let q = UpdateBuilder::update("node")
            .set("display_name", "Foo")
            .return_none()
            .build();
        assert!(q.sql.ends_with("RETURN NONE"));
    }

    // ── Filter::StartsWith tests ──

    #[test]
    fn filter_starts_with() {
        let q = QueryBuilder::new()
            .select(&["*"])
            .from("node")
            .filter(Filter::starts_with("scope", "VERSE#v1"))
            .build();
        assert_eq!(
            q.sql,
            "SELECT * FROM node WHERE string::starts_with(scope, $p0)"
        );
        assert_eq!(q.params[0].1, serde_json::json!("VERSE#v1"));
    }

    // ── Filter::IsNone / IsNotNone tests ──

    #[test]
    fn filter_is_none() {
        let q = QueryBuilder::new()
            .select(&["*"])
            .from("node")
            .filter(Filter::is_none("asset_id"))
            .build();
        assert_eq!(q.sql, "SELECT * FROM node WHERE asset_id IS NONE");
        assert!(q.params.is_empty());
    }

    #[test]
    fn filter_is_not_none() {
        let q = QueryBuilder::new()
            .select(&["*"])
            .from("node")
            .filter(Filter::is_not_none("asset_id"))
            .build();
        assert_eq!(q.sql, "SELECT * FROM node WHERE asset_id IS NOT NONE");
        assert!(q.params.is_empty());
    }

    // ── Filter::InSubquery tests ──

    #[test]
    fn filter_in_subquery() {
        let sub = QueryBuilder::new()
            .select(&["petal_id"])
            .from("petal")
            .filter(Filter::eq("fractal_id", "f1"))
            .build();
        let q = QueryBuilder::new()
            .select(&["*"])
            .from("node")
            .filter(Filter::in_subquery("petal_id", sub))
            .build();
        assert_eq!(
            q.sql,
            "SELECT * FROM node WHERE petal_id IN (SELECT petal_id FROM petal WHERE fractal_id = $p0)"
        );
        assert_eq!(q.params.len(), 1);
        assert_eq!(q.params[0].1, serde_json::json!("f1"));
    }

    #[test]
    fn filter_in_subquery_param_remapping() {
        // Outer query has params before the subquery, so subquery params must be remapped
        let sub = QueryBuilder::new()
            .select(&["petal_id"])
            .from("petal")
            .filter(Filter::eq("fractal_id", "f1"))
            .build();
        let q = QueryBuilder::new()
            .select(&["*"])
            .from("node")
            .filter(Filter::eq("status", "active"))
            .and(Filter::in_subquery("petal_id", sub))
            .build();
        // .and() on QueryBuilder adds a second top-level filter (no parens wrapper).
        // Outer filter uses $p0, subquery's fractal_id param remapped to $p1.
        assert_eq!(
            q.sql,
            "SELECT * FROM node WHERE status = $p0 AND petal_id IN (SELECT petal_id FROM petal WHERE fractal_id = $p1)"
        );
        assert_eq!(q.params.len(), 2);
        assert_eq!(q.params[0].1, serde_json::json!("active"));
        assert_eq!(q.params[1].1, serde_json::json!("f1"));
    }

    // ── BuiltQuery::sql() convenience ──

    #[test]
    fn built_query_sql_method() {
        let q = QueryBuilder::new()
            .select(&["*"])
            .from("node")
            .build();
        assert_eq!(q.sql(), "SELECT * FROM node");
    }

    #[test]
    fn parameterization_prevents_injection() {
        // Even if user passes something that looks like SQL, it becomes a param
        let q = QueryBuilder::new()
            .select(&["*"])
            .from("node")
            .filter(Filter::eq("petal_id", "'; DROP TABLE node; --"))
            .build();

        // The malicious string should be in params, not in the SQL
        assert_eq!(q.sql, "SELECT * FROM node WHERE petal_id = $p0");
        assert_eq!(q.params[0].1, serde_json::json!("'; DROP TABLE node; --"));
    }
}
