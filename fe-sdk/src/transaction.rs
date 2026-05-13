//! Plugin transaction trait for the extension SDK.
//!
//! Extension authors use [`PluginTransaction`] to batch scene mutations.
//! The engine provides a concrete implementation via [`PluginContext::transaction()`].
//!
//! The trait is object-safe so it can be passed as `Box<dyn PluginTransaction>`.

use crate::property::PropertyValue;

/// A transaction for batching scene mutations.
///
/// Object-safe: extensions receive `Box<dyn PluginTransaction>` from the
/// plugin context and queue operations before calling [`commit()`].
///
/// [`commit()`]: PluginTransaction::commit
pub trait PluginTransaction: Send {
    /// Queue a property-set operation on a node.
    fn set_property(&mut self, node_id: &str, key: &str, value: PropertyValue);

    /// Queue a node-creation operation. Returns a provisional node ID that
    /// will be replaced with the real ID after commit.
    fn create_node(&mut self, petal_id: &str, name: &str) -> String;

    /// Queue a node-deletion operation.
    fn delete_node(&mut self, node_id: &str);

    /// Commit all queued operations atomically.
    fn commit(&mut self) -> Result<(), String>;
}

#[cfg(test)]
mod object_safety_tests {
    use crate::transaction::PluginTransaction;
    use crate::context::PluginContext;

    fn _assert_transaction_object_safe(_x: Box<dyn PluginTransaction>) {}
    fn _assert_context_object_safe(_x: &dyn PluginContext) {}

    #[test]
    fn traits_are_object_safe() {
        // This test only compiles if both traits are object-safe.
        // If not object-safe, the compiler will reject the fn declarations above.
    }
}
