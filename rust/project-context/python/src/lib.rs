//! Conservative, read-only Python project context and admitted pytest receipt checks.
//!
//! No Python imports, filesystem reads, subprocesses or pytest invocations occur here.
//! The coordinator supplies an explicitly scoped byte snapshot and a separately admitted
//! worker supplies observed collection and execution facts. Static test-file names are
//! discovery hints, never evidence that no tests exist.

mod digest;
mod project;
mod receipt;

pub use project::{
    Change, ContextError, ContextIdentity, ContextStatus, Gap, Inventory, PackageRoot,
    ProjectContext, ProjectFacts, ProjectSelection, SelectionContext, SelectionStatus,
    SnapshotFile, WorkspaceScope, analyze, select_tests,
};
pub use receipt::{
    Admission, Artifact, CaseOutcome, Collected, Observation, Process, PytestCase, ReceiptBinding,
    ReceiptDisposition, ReceiptResult, Run, classify_receipt, node_ids_digest,
};
