//! Paged scene reads over `scene.query-page`; see tools/fe-ext/AGENTS.md §guest-sdk.

use serde_json::Value;

use crate::bindings::fractalengine::plugin::scene;
use crate::{HostError, NodeSnapshot};

/// Every matching node of a query, read page by page.
#[derive(Debug, Default)]
pub struct AllNodes {
    /// Matching nodes in node-id order.
    pub nodes: Vec<NodeSnapshot>,
    /// Matching nodes over the host's per-node read bound, skipped by the host.
    pub omitted: Vec<String>,
}

/// Read every page of `query` (a NodeQuery object, e.g. `{"limit":200}`), following each page's cursor.
/// Each page is one host read: a hook that pages past its read budget gets `budget-exceeded`.
pub fn all(query: &Value) -> Result<AllNodes, HostError> {
    let mut query = query.clone();
    let Some(fields) = query.as_object_mut() else {
        return Err(HostError::InvalidInput("a node query is a JSON object".into()));
    };
    fields.remove("after");
    let mut all = AllNodes::default();
    loop {
        let page = scene::query_page(&query.to_string())?;
        all.nodes.extend(page.nodes);
        all.omitted.extend(page.omitted);
        match page.next {
            Some(next) => query["after"] = Value::String(next),
            None => return Ok(all),
        }
    }
}
