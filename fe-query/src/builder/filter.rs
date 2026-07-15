use super::value::QueryValue;

/// A composable filter predicate for SurrealQL WHERE clauses.
///
/// Each variant carries field names and values that get rendered into
/// parameterized SQL fragments. Values are never interpolated directly
/// into query strings — they always become `$param_N` bind parameters.
#[derive(Debug, Clone)]
pub enum Filter {
    /// field = value
    Eq(String, QueryValue),
    /// field != value
    Ne(String, QueryValue),
    /// field > value
    Gt(String, QueryValue),
    /// field >= value
    Gte(String, QueryValue),
    /// field < value
    Lt(String, QueryValue),
    /// field <= value
    Lte(String, QueryValue),
    /// field IN [values]
    In(String, QueryValue),
    /// field CONTAINS value
    Contains(String, QueryValue),
    /// field IS NULL
    IsNull(String),
    /// field IS NOT NULL
    IsNotNull(String),
    /// left AND right
    And(Box<Filter>, Box<Filter>),
    /// left OR right
    Or(Box<Filter>, Box<Filter>),
    /// NOT inner
    Not(Box<Filter>),
    /// Spatial: field <|radius_m|> point — "distance within"
    DWithin(String, QueryValue, f64),
    /// Spatial: geometry INSIDE polygon
    Within(String, QueryValue),
    /// string::starts_with(field, value)
    StartsWith(String, QueryValue),
    /// field IS NONE (SurrealDB-native none check)
    IsNone(String),
    /// field IS NOT NONE (SurrealDB-native none check)
    IsNotNone(String),
    /// field IN (subquery) — embeds a sub-SELECT with merged params
    InSubquery(String, Box<super::render::BuiltQuery>),
    /// Raw SurrealQL fragment (escape hatch). The string is emitted verbatim
    /// with no parameters. Use sparingly.
    Raw(String),
}

impl Filter {
    /// Render this filter into a parameterized SQL fragment and its bindings.
    ///
    /// `param_counter` is incremented for each parameter produced, ensuring
    /// unique names across an entire query.
    pub fn render(&self, param_counter: &mut usize) -> (String, Vec<(String, serde_json::Value)>) {
        match self {
            Filter::Eq(field, val) => {
                let name = format!("p{}", *param_counter);
                *param_counter += 1;
                (
                    format!("{} = ${}", field, name),
                    vec![(name, val.to_bind_value())],
                )
            }
            Filter::Ne(field, val) => {
                let name = format!("p{}", *param_counter);
                *param_counter += 1;
                (
                    format!("{} != ${}", field, name),
                    vec![(name, val.to_bind_value())],
                )
            }
            Filter::Gt(field, val) => {
                let name = format!("p{}", *param_counter);
                *param_counter += 1;
                (
                    format!("{} > ${}", field, name),
                    vec![(name, val.to_bind_value())],
                )
            }
            Filter::Gte(field, val) => {
                let name = format!("p{}", *param_counter);
                *param_counter += 1;
                (
                    format!("{} >= ${}", field, name),
                    vec![(name, val.to_bind_value())],
                )
            }
            Filter::Lt(field, val) => {
                let name = format!("p{}", *param_counter);
                *param_counter += 1;
                (
                    format!("{} < ${}", field, name),
                    vec![(name, val.to_bind_value())],
                )
            }
            Filter::Lte(field, val) => {
                let name = format!("p{}", *param_counter);
                *param_counter += 1;
                (
                    format!("{} <= ${}", field, name),
                    vec![(name, val.to_bind_value())],
                )
            }
            Filter::In(field, val) => {
                let name = format!("p{}", *param_counter);
                *param_counter += 1;
                (
                    format!("{} IN ${}", field, name),
                    vec![(name, val.to_bind_value())],
                )
            }
            Filter::Contains(field, val) => {
                let name = format!("p{}", *param_counter);
                *param_counter += 1;
                (
                    format!("{} CONTAINS ${}", field, name),
                    vec![(name, val.to_bind_value())],
                )
            }
            Filter::IsNull(field) => (format!("{} IS NONE", field), vec![]),
            Filter::IsNotNull(field) => (format!("{} IS NOT NONE", field), vec![]),
            Filter::And(left, right) => {
                let (left_sql, left_params) = left.render(param_counter);
                let (right_sql, right_params) = right.render(param_counter);
                let mut params = left_params;
                params.extend(right_params);
                (format!("({} AND {})", left_sql, right_sql), params)
            }
            Filter::Or(left, right) => {
                let (left_sql, left_params) = left.render(param_counter);
                let (right_sql, right_params) = right.render(param_counter);
                let mut params = left_params;
                params.extend(right_params);
                (format!("({} OR {})", left_sql, right_sql), params)
            }
            Filter::Not(inner) => {
                let (inner_sql, inner_params) = inner.render(param_counter);
                (format!("NOT ({})", inner_sql), inner_params)
            }
            Filter::DWithin(field, point, radius_m) => {
                let point_name = format!("p{}", *param_counter);
                *param_counter += 1;
                let radius_name = format!("p{}", *param_counter);
                *param_counter += 1;
                (
                    format!(
                        "geo::distance({}, ${}) <= ${}",
                        field, point_name, radius_name
                    ),
                    vec![
                        (point_name, point.to_bind_value()),
                        (radius_name, serde_json::json!(radius_m)),
                    ],
                )
            }
            Filter::Within(field, geometry) => {
                let name = format!("p{}", *param_counter);
                *param_counter += 1;
                (
                    format!("geo::inside({}, ${})", field, name),
                    vec![(name, geometry.to_bind_value())],
                )
            }
            Filter::StartsWith(field, val) => {
                let name = format!("p{}", *param_counter);
                *param_counter += 1;
                (
                    format!("string::starts_with({}, ${})", field, name),
                    vec![(name, val.to_bind_value())],
                )
            }
            Filter::IsNone(field) => (format!("{} IS NONE", field), vec![]),
            Filter::IsNotNone(field) => (format!("{} IS NOT NONE", field), vec![]),
            Filter::InSubquery(field, subquery) => {
                // Remap subquery params to avoid collisions with outer query
                let mut remapped_sql = subquery.sql.clone();
                let mut params = Vec::new();
                for (old_name, value) in &subquery.params {
                    let new_name = format!("p{}", *param_counter);
                    *param_counter += 1;
                    remapped_sql =
                        remapped_sql.replace(&format!("${}", old_name), &format!("${}", new_name));
                    params.push((new_name, value.clone()));
                }
                (format!("{} IN ({})", field, remapped_sql), params)
            }
            Filter::Raw(sql) => (sql.clone(), vec![]),
        }
    }
}

// ── Convenience constructors ──

impl Filter {
    pub fn eq(field: impl Into<String>, val: impl Into<QueryValue>) -> Self {
        Filter::Eq(field.into(), val.into())
    }

    pub fn ne(field: impl Into<String>, val: impl Into<QueryValue>) -> Self {
        Filter::Ne(field.into(), val.into())
    }

    pub fn gt(field: impl Into<String>, val: impl Into<QueryValue>) -> Self {
        Filter::Gt(field.into(), val.into())
    }

    pub fn gte(field: impl Into<String>, val: impl Into<QueryValue>) -> Self {
        Filter::Gte(field.into(), val.into())
    }

    pub fn lt(field: impl Into<String>, val: impl Into<QueryValue>) -> Self {
        Filter::Lt(field.into(), val.into())
    }

    pub fn lte(field: impl Into<String>, val: impl Into<QueryValue>) -> Self {
        Filter::Lte(field.into(), val.into())
    }

    pub fn is_in(field: impl Into<String>, val: impl Into<QueryValue>) -> Self {
        Filter::In(field.into(), val.into())
    }

    pub fn contains(field: impl Into<String>, val: impl Into<QueryValue>) -> Self {
        Filter::Contains(field.into(), val.into())
    }

    pub fn is_null(field: impl Into<String>) -> Self {
        Filter::IsNull(field.into())
    }

    pub fn is_not_null(field: impl Into<String>) -> Self {
        Filter::IsNotNull(field.into())
    }

    pub fn and(self, other: Filter) -> Self {
        Filter::And(Box::new(self), Box::new(other))
    }

    pub fn or(self, other: Filter) -> Self {
        Filter::Or(Box::new(self), Box::new(other))
    }

    #[allow(clippy::should_implement_trait)] // builder DSL method, not std::ops::Not
    pub fn not(self) -> Self {
        Filter::Not(Box::new(self))
    }

    pub fn d_within(field: impl Into<String>, point: impl Into<QueryValue>, radius_m: f64) -> Self {
        Filter::DWithin(field.into(), point.into(), radius_m)
    }

    pub fn within(field: impl Into<String>, geometry: impl Into<QueryValue>) -> Self {
        Filter::Within(field.into(), geometry.into())
    }

    pub fn starts_with(field: impl Into<String>, val: impl Into<QueryValue>) -> Self {
        Filter::StartsWith(field.into(), val.into())
    }

    pub fn is_none(field: impl Into<String>) -> Self {
        Filter::IsNone(field.into())
    }

    pub fn is_not_none(field: impl Into<String>) -> Self {
        Filter::IsNotNone(field.into())
    }

    pub fn in_subquery(field: impl Into<String>, subquery: super::render::BuiltQuery) -> Self {
        Filter::InSubquery(field.into(), Box::new(subquery))
    }

    pub fn raw(sql: impl Into<String>) -> Self {
        Filter::Raw(sql.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_eq() {
        let f = Filter::eq("petal_id", "abc");
        let mut c = 0;
        let (sql, params) = f.render(&mut c);
        assert_eq!(sql, "petal_id = $p0");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].0, "p0");
        assert_eq!(params[0].1, serde_json::json!("abc"));
        assert_eq!(c, 1);
    }

    #[test]
    fn render_ne() {
        let f = Filter::ne("status", "deleted");
        let mut c = 0;
        let (sql, _) = f.render(&mut c);
        assert_eq!(sql, "status != $p0");
    }

    #[test]
    fn render_gt_gte_lt_lte() {
        let mut c = 0;
        let (sql, _) = Filter::gt("edit_seq", 5i64).render(&mut c);
        assert_eq!(sql, "edit_seq > $p0");

        let (sql, _) = Filter::gte("edit_seq", 5i64).render(&mut c);
        assert_eq!(sql, "edit_seq >= $p1");

        let (sql, _) = Filter::lt("edit_seq", 10i64).render(&mut c);
        assert_eq!(sql, "edit_seq < $p2");

        let (sql, _) = Filter::lte("edit_seq", 10i64).render(&mut c);
        assert_eq!(sql, "edit_seq <= $p3");
    }

    #[test]
    fn render_in() {
        let vals = QueryValue::Array(vec![
            QueryValue::String("a".into()),
            QueryValue::String("b".into()),
        ]);
        let f = Filter::is_in("petal_id", vals);
        let mut c = 0;
        let (sql, params) = f.render(&mut c);
        assert_eq!(sql, "petal_id IN $p0");
        assert_eq!(params[0].1, serde_json::json!(["a", "b"]));
    }

    #[test]
    fn render_contains() {
        let f = Filter::contains("tags", "urgent");
        let mut c = 0;
        let (sql, _) = f.render(&mut c);
        assert_eq!(sql, "tags CONTAINS $p0");
    }

    #[test]
    fn render_is_null_and_not_null() {
        let mut c = 0;
        let (sql, params) = Filter::is_null("asset_id").render(&mut c);
        assert_eq!(sql, "asset_id IS NONE");
        assert!(params.is_empty());

        let (sql, params) = Filter::is_not_null("asset_id").render(&mut c);
        assert_eq!(sql, "asset_id IS NOT NONE");
        assert!(params.is_empty());
    }

    #[test]
    fn render_and() {
        let f = Filter::eq("petal_id", "abc").and(Filter::gt("edit_seq", 5i64));
        let mut c = 0;
        let (sql, params) = f.render(&mut c);
        assert_eq!(sql, "(petal_id = $p0 AND edit_seq > $p1)");
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn render_or() {
        let f = Filter::eq("status", "active").or(Filter::eq("status", "pending"));
        let mut c = 0;
        let (sql, params) = f.render(&mut c);
        assert_eq!(sql, "(status = $p0 OR status = $p1)");
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn render_not() {
        let f = Filter::eq("deleted", true).not();
        let mut c = 0;
        let (sql, params) = f.render(&mut c);
        assert_eq!(sql, "NOT (deleted = $p0)");
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn render_d_within() {
        let point = QueryValue::Point { x: 1.0, z: 2.0 };
        let f = Filter::d_within("position", point, 500.0);
        let mut c = 0;
        let (sql, params) = f.render(&mut c);
        assert_eq!(sql, "geo::distance(position, $p0) <= $p1");
        assert_eq!(params.len(), 2);
        assert_eq!(params[1].1, serde_json::json!(500.0));
    }

    #[test]
    fn render_within() {
        let poly = QueryValue::Polygon(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]);
        let f = Filter::within("position", poly);
        let mut c = 0;
        let (sql, params) = f.render(&mut c);
        assert_eq!(sql, "geo::inside(position, $p0)");
        assert_eq!(params[0].1["type"], "Polygon");
    }

    #[test]
    fn render_raw() {
        let f = Filter::raw("custom_fn(field) = true");
        let mut c = 0;
        let (sql, params) = f.render(&mut c);
        assert_eq!(sql, "custom_fn(field) = true");
        assert!(params.is_empty());
    }

    #[test]
    fn nested_composition() {
        // (petal_id = 'abc' AND (edit_seq > 5 OR status = 'active'))
        let inner = Filter::gt("edit_seq", 5i64).or(Filter::eq("status", "active"));
        let f = Filter::eq("petal_id", "abc").and(inner);
        let mut c = 0;
        let (sql, params) = f.render(&mut c);
        assert_eq!(sql, "(petal_id = $p0 AND (edit_seq > $p1 OR status = $p2))");
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn param_counter_continuity() {
        let mut c = 5;
        let f = Filter::eq("a", "x");
        let (sql, _) = f.render(&mut c);
        assert_eq!(sql, "a = $p5");
        assert_eq!(c, 6);
    }
}
