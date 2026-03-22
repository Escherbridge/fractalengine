#[cfg(test)]
mod tests {
    // test_public_cannot_write: connect as public role, attempt create_petal → Err
    // test_custom_role_scoped: assign custom role, verify they can access assigned petal
    // test_admin_full_access: verify admin can CRUD all tables
    // test_op_log_written_before_change: intercept op_log write order
}
