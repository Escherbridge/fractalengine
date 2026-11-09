//! Compiled-in extension descriptors; see `fe-ui/src/extensions/builtin/AGENTS.md`.

pub mod copy_for_bi;

pub use copy_for_bi::{
    snippet_rows, BoundSnippet, ConnectorDescriptor, SnippetInputs, SnippetRow, COPY_FOR_BI_TARGETS,
};
