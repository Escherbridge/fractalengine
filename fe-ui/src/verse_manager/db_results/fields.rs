//! Handler for field-definition list results. See ../AGENTS.md §db-results.

use fe_runtime::messages::FieldDefInfo;

use crate::plugin::InspectorFormState;

/// `FieldDefsListed`: populate the inspector's field-def table.
pub(super) fn handle_field_defs_listed(
    field_defs: &[FieldDefInfo],
    inspector: &mut InspectorFormState,
) {
    inspector.field_defs = field_defs
        .iter()
        .map(|f| crate::plugin::FieldDefEntry {
            field_def_id: f.field_def_id.clone(),
            key: f.key.clone(),
            value_type: f.value_type.clone(),
            description: String::new(),
            required: false,
            default_val: f.default_val.clone(),
        })
        .collect();
    inspector.field_defs_loading = false;
}
