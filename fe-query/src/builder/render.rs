use super::filter::Filter;
use super::value::QueryValue;

/// A fully rendered, parameterized SurrealQL query ready for execution.
#[derive(Debug, Clone)]
pub struct BuiltQuery {
    /// The SurrealQL statement with `$param_N` placeholders.
    pub sql: String,
    /// Ordered bind parameters: (name, value) pairs.
    pub params: Vec<(String, serde_json::Value)>,
}

impl BuiltQuery {
    /// Returns the SQL string by reference.
    pub fn sql(&self) -> &str {
        &self.sql
    }
}

/// Sort direction for ORDER BY clauses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortDir {
    Asc,
    Desc,
}

// ── SELECT rendering ──

/// Internal representation accumulated by `QueryBuilder`.
#[derive(Debug, Clone)]
pub(crate) struct SelectParts {
    pub fields: Vec<String>,
    pub table: String,
    pub filters: Vec<Filter>,
    pub group_by: Vec<String>,
    pub group_all: bool,
    pub order_by: Vec<(String, SortDir)>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

/// Render a SELECT query from accumulated parts.
pub(crate) fn render_select(parts: &SelectParts) -> BuiltQuery {
    let mut param_counter = 0usize;
    let mut all_params = Vec::new();

    // SELECT fields
    let fields = if parts.fields.is_empty() || parts.fields == ["*"] {
        "*".to_string()
    } else {
        parts.fields.join(", ")
    };

    let mut sql = format!("SELECT {} FROM {}", fields, parts.table);

    // WHERE
    if !parts.filters.is_empty() {
        let mut where_parts = Vec::new();
        for f in &parts.filters {
            let (fragment, params) = f.render(&mut param_counter);
            where_parts.push(fragment);
            all_params.extend(params);
        }
        sql.push_str(" WHERE ");
        sql.push_str(&where_parts.join(" AND "));
    }

    // GROUP BY
    if parts.group_all {
        sql.push_str(" GROUP ALL");
    } else if !parts.group_by.is_empty() {
        sql.push_str(" GROUP BY ");
        sql.push_str(&parts.group_by.join(", "));
    }

    // ORDER BY
    if !parts.order_by.is_empty() {
        let clauses: Vec<String> = parts
            .order_by
            .iter()
            .map(|(field, dir)| {
                let d = match dir {
                    SortDir::Asc => "ASC",
                    SortDir::Desc => "DESC",
                };
                format!("{} {}", field, d)
            })
            .collect();
        sql.push_str(" ORDER BY ");
        sql.push_str(&clauses.join(", "));
    }

    // LIMIT
    if let Some(limit) = parts.limit {
        let name = format!("p{}", param_counter);
        param_counter += 1;
        sql.push_str(&format!(" LIMIT ${}", name));
        all_params.push((name, serde_json::json!(limit)));
    }

    // OFFSET (START in SurrealQL)
    if let Some(offset) = parts.offset {
        let name = format!("p{}", param_counter);
        // param_counter not needed after this but keep consistent
        let _ = param_counter;
        sql.push_str(&format!(" START ${}", name));
        all_params.push((name, serde_json::json!(offset)));
    }

    BuiltQuery {
        sql,
        params: all_params,
    }
}

// ── INSERT rendering ──

/// Internal representation for CREATE (insert) queries.
#[derive(Debug, Clone)]
pub(crate) struct InsertParts {
    pub table: String,
    pub content: serde_json::Value,
}

/// Render a CREATE ... CONTENT query (SurrealDB's insert equivalent).
pub(crate) fn render_insert(parts: &InsertParts) -> BuiltQuery {
    let name = "p0".to_string();
    let sql = format!("CREATE {} CONTENT ${}", parts.table, name);
    BuiltQuery {
        sql,
        params: vec![(name, parts.content.clone())],
    }
}

// ── UPDATE rendering ──

/// Return clause variants for UPDATE/DELETE statements.
#[derive(Debug, Clone, PartialEq)]
pub enum ReturnClause {
    After,
    Before,
    None,
}

/// Internal representation for UPDATE queries.
#[derive(Debug, Clone)]
pub(crate) struct UpdateParts {
    pub table: String,
    pub set_clauses: Vec<(String, QueryValue)>,
    /// Raw expression assignments: `field = expr` (not parameterized).
    pub set_exprs: Vec<(String, String)>,
    pub filters: Vec<Filter>,
    pub return_clause: Option<ReturnClause>,
}

/// Render an UPDATE ... SET ... WHERE query.
pub(crate) fn render_update(parts: &UpdateParts) -> BuiltQuery {
    let mut param_counter = 0usize;
    let mut all_params = Vec::new();

    let mut set_fragments = Vec::new();
    for (field, val) in &parts.set_clauses {
        let name = format!("p{}", param_counter);
        param_counter += 1;
        set_fragments.push(format!("{} = ${}", field, name));
        all_params.push((name, val.to_bind_value()));
    }

    // Raw expression assignments (not parameterized)
    for (field, expr) in &parts.set_exprs {
        set_fragments.push(format!("{} = {}", field, expr));
    }

    let mut sql = format!("UPDATE {} SET {}", parts.table, set_fragments.join(", "));

    // WHERE
    if !parts.filters.is_empty() {
        let mut where_parts = Vec::new();
        for f in &parts.filters {
            let (fragment, params) = f.render(&mut param_counter);
            where_parts.push(fragment);
            all_params.extend(params);
        }
        sql.push_str(" WHERE ");
        sql.push_str(&where_parts.join(" AND "));
    }

    // RETURN
    if let Some(ret) = &parts.return_clause {
        match ret {
            ReturnClause::After => sql.push_str(" RETURN AFTER"),
            ReturnClause::Before => sql.push_str(" RETURN BEFORE"),
            ReturnClause::None => sql.push_str(" RETURN NONE"),
        }
    }

    BuiltQuery {
        sql,
        params: all_params,
    }
}

// ── DELETE rendering ──

/// Internal representation for DELETE queries.
#[derive(Debug, Clone)]
pub(crate) struct DeleteParts {
    pub table: String,
    pub filters: Vec<Filter>,
}

/// Render a DELETE FROM ... WHERE query.
pub(crate) fn render_delete(parts: &DeleteParts) -> BuiltQuery {
    let mut param_counter = 0usize;
    let mut all_params = Vec::new();

    let mut sql = format!("DELETE FROM {}", parts.table);

    // WHERE
    if !parts.filters.is_empty() {
        let mut where_parts = Vec::new();
        for f in &parts.filters {
            let (fragment, params) = f.render(&mut param_counter);
            where_parts.push(fragment);
            all_params.extend(params);
        }
        sql.push_str(" WHERE ");
        sql.push_str(&where_parts.join(" AND "));
    }

    BuiltQuery {
        sql,
        params: all_params,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_simple_select() {
        let parts = SelectParts {
            fields: vec!["node_id".into(), "petal_id".into()],
            table: "node".into(),
            filters: vec![],
            group_by: vec![],
            group_all: false,
            order_by: vec![],
            limit: None,
            offset: None,
        };
        let q = render_select(&parts);
        assert_eq!(q.sql, "SELECT node_id, petal_id FROM node");
        assert!(q.params.is_empty());
    }

    #[test]
    fn render_select_star() {
        let parts = SelectParts {
            fields: vec!["*".into()],
            table: "node".into(),
            filters: vec![],
            group_by: vec![],
            group_all: false,
            order_by: vec![],
            limit: None,
            offset: None,
        };
        let q = render_select(&parts);
        assert_eq!(q.sql, "SELECT * FROM node");
    }

    #[test]
    fn render_select_with_where_order_limit_offset() {
        let parts = SelectParts {
            fields: vec!["*".into()],
            table: "node".into(),
            filters: vec![
                Filter::eq("petal_id", "p1"),
                Filter::gt("edit_seq", 5i64),
            ],
            group_by: vec![],
            group_all: false,
            order_by: vec![("created_at".into(), SortDir::Asc)],
            limit: Some(100),
            offset: Some(20),
        };
        let q = render_select(&parts);
        assert_eq!(
            q.sql,
            "SELECT * FROM node WHERE petal_id = $p0 AND edit_seq > $p1 ORDER BY created_at ASC LIMIT $p2 START $p3"
        );
        assert_eq!(q.params.len(), 4);
        assert_eq!(q.params[0].1, serde_json::json!("p1"));
        assert_eq!(q.params[1].1, serde_json::json!(5));
        assert_eq!(q.params[2].1, serde_json::json!(100));
        assert_eq!(q.params[3].1, serde_json::json!(20));
    }

    #[test]
    fn render_insert_content() {
        let content = serde_json::json!({
            "node_id": "n1",
            "petal_id": "p1",
            "display_name": "Test Node"
        });
        let parts = InsertParts {
            table: "node".into(),
            content,
        };
        let q = render_insert(&parts);
        assert_eq!(q.sql, "CREATE node CONTENT $p0");
        assert_eq!(q.params.len(), 1);
        assert_eq!(q.params[0].1["node_id"], "n1");
    }

    #[test]
    fn render_update_with_where() {
        let parts = UpdateParts {
            table: "node".into(),
            set_clauses: vec![
                ("display_name".into(), QueryValue::String("Renamed".into())),
                ("edit_seq".into(), QueryValue::Int(42)),
            ],
            set_exprs: vec![],
            filters: vec![Filter::eq("node_id", "n1")],
            return_clause: None,
        };
        let q = render_update(&parts);
        assert_eq!(
            q.sql,
            "UPDATE node SET display_name = $p0, edit_seq = $p1 WHERE node_id = $p2"
        );
        assert_eq!(q.params.len(), 3);
        assert_eq!(q.params[0].1, serde_json::json!("Renamed"));
        assert_eq!(q.params[1].1, serde_json::json!(42));
        assert_eq!(q.params[2].1, serde_json::json!("n1"));
    }

    #[test]
    fn render_update_no_where() {
        let parts = UpdateParts {
            table: "node".into(),
            set_clauses: vec![("interactive".into(), QueryValue::Bool(true))],
            set_exprs: vec![],
            filters: vec![],
            return_clause: None,
        };
        let q = render_update(&parts);
        assert_eq!(q.sql, "UPDATE node SET interactive = $p0");
        assert_eq!(q.params.len(), 1);
    }

    #[test]
    fn render_select_multiple_order_by() {
        let parts = SelectParts {
            fields: vec![],
            table: "node_log".into(),
            filters: vec![],
            group_by: vec![],
            group_all: false,
            order_by: vec![
                ("hlc_timestamp".into(), SortDir::Desc),
                ("row_version".into(), SortDir::Asc),
            ],
            limit: None,
            offset: None,
        };
        let q = render_select(&parts);
        assert_eq!(
            q.sql,
            "SELECT * FROM node_log ORDER BY hlc_timestamp DESC, row_version ASC"
        );
    }
}
