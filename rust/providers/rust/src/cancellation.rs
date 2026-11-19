//! Opt-in advisory consumer of externally established select-arm facts.
//!
//! This module does not extract facts, resolve names, expand macros, run Cargo,
//! or execute futures. `analyze_source` cannot produce this semantic evidence.
//! Public enums are a fixture/adapter interface, not proof that their assertions
//! are true. An owner must qualify the producer before using its assessments.
//!
//! The reviewed contract is official, unmodified Tokio 1.54.1, source commit
//! b53b2001ed2ccfd9b87e32a0c6da1a1eebdc80b8. Only read_exact and write_all are
//! risky summaries; read and write are cancellation-safe wait controls. All
//! other Tokio APIs remain unsupported. In particular, disabled expressions
//! are still constructed: the no-poll exclusion must not be generalized to
//! read_to_string, arbitrary arguments, constructors, helpers or Drop effects.
//!
//! An advisory establishes a documented possibility under supplied facts. It
//! does not establish an executed cancellation, lost bytes, or an application
//! defect. A one-shot shutdown may intentionally accept partial I/O.

pub mod attested;

use crate::{MAX_SOURCE_BYTES, SourceEdition, Span};
use sha2::{Digest, Sha256};
use std::fmt;

pub const RULE_ID: &str = "sg-check-r07.tokio-select-partial-io.v1";
pub const SUMMARY_VERSION: &str = "tokio-1.54.1/direct-io-v1";
pub const MAX_OPERATIONS: usize = 256;
pub const LIMITATIONS: &[&str] = &[
    "Semantic assertions are trusted inputs; this consumer does not extract or prove them.",
    "Risk is API-level: argument values, I/O implementation and actual partial effects are unmodeled.",
    "Application invariants, helper bodies and whole-program cancellation safety are outside this rule.",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Options {
    pub enabled: bool,
    /// 1..=MAX_OPERATIONS. Every assessment, including exclusions, uses a slot.
    pub max_records: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { enabled: false, max_records: MAX_OPERATIONS }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextIdentity {
    pub edition: SourceEdition,
    /// All semantic inputs: local modules/imports, dependency and macro bytes,
    /// source maps, target/features/cfg/toolchain, producer version and options.
    pub project_sha256: [u8; 32],
    /// Exact baseline/candidate revisions, contexts and comparison policy.
    pub comparison_sha256: [u8; 32],
}

#[derive(Clone, Copy, Debug)]
pub struct SourceUnit<'a> {
    pub logical_document: &'a str,
    /// Original UTF-8 bytes, including original line endings.
    pub source: &'a str,
    pub context: ContextIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Coverage {
    /// Complete only for the producer's explicitly declared nomination scope.
    Complete,
    Partial,
    /// No semantic producer available; operations must be empty.
    Unavailable,
}

#[derive(Clone, Copy, Debug)]
pub struct TypedFacts<'a> {
    /// Same bytes in a different module can resolve to different definitions.
    pub logical_document: &'a str,
    pub source_sha256: [u8; 32],
    pub context: ContextIdentity,
    /// Versioned producer identifier, at most 128 UTF-8 bytes, no controls.
    pub producer: &'a str,
    pub coverage: Coverage,
    pub operations: &'a [ArmFact],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokioPackage {
    /// Resolved official crates.io package with reviewed bytes, not a spelling
    /// or version-string assertion. Renaming is allowed; forks/patches are not.
    Canonical1_54_1,
    Unreviewed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectMacro {
    Tokio(TokioPackage),
    Other,
    Unresolved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoOperation {
    ReadExact,
    WriteAll,
    Read,
    Write,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Callee {
    /// Actual selected Tokio extension-trait item, including receiver dispatch.
    TokioIo { package: TokioPackage, operation: IoOperation },
    Other,
    AmbiguousTrait,
    Unresolved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Attribution {
    /// One reviewed future directly selected in active, mapped source. Proven
    /// transparent Pin/reborrow wrappers are allowed; arbitrary wrappers are not.
    Direct,
    HelperOrComposite,
    ExternalExpansion,
    UnknownConfiguration,
    UnknownSourceMapping,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Introduction {
    /// Introduced into this cancellation context against the bound baseline.
    /// A changed enclosing line/function by itself is insufficient evidence.
    Introduced,
    Preexisting,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ownership {
    /// The selected operation's owner is destroyed on the losing select path.
    /// This can include an owned Pin<Box<F>>; pinning does not prevent drop.
    OwnedBySelect,
    /// Only a reborrow is dropped; the same underlying future owner survives.
    RetainedOutsideSelect,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cancellation {
    /// The operation can be polled to Pending, then a distinct matching branch
    /// can win. This is a possibility fact, not an observed schedule or effect.
    MayDropPending,
    /// A false precondition or proven polling order excludes all target polls.
    NeverPolled,
    /// No losing path can discard this operation after a Pending poll.
    NoPendingDrop,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Repetition {
    /// Recreates the operation and can revisit it after a competing winner.
    RecreatedInLoop,
    OneShot,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArmFact {
    pub select_span: Span,
    /// Selected future expression in the original source, not expansion bytes.
    /// For a reborrow this anchors the borrow, not the earlier constructor.
    pub future_span: Span,
    pub select_macro: SelectMacro,
    pub callee: Callee,
    pub attribution: Attribution,
    pub introduction: Introduction,
    pub ownership: Ownership,
    pub cancellation: Cancellation,
    pub repetition: Repetition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartialEffect {
    ReadProgress,
    WriteProgress,
}

impl PartialEffect {
    pub const fn documentation(self) -> &'static str {
        match self {
            Self::ReadProgress => "https://docs.rs/tokio/1.54.1/tokio/io/trait.AsyncReadExt.html#method.read_exact",
            Self::WriteProgress => "https://docs.rs/tokio/1.54.1/tokio/io/trait.AsyncWriteExt.html#method.write_all",
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::ReadProgress => "This arm may drop a pending read_exact future. This API permits partial input progress; invocation-specific effects and a defect are unproved.",
            Self::WriteProgress => "This arm may drop a pending write_all future. This API permits partial output progress; invocation-specific effects and a defect are unproved.",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Exclusion {
    Preexisting,
    DifferentMacro,
    DifferentCallee,
    DocumentedSafeWait,
    RetainedFuture,
    NeverPolled,
    NoPendingDrop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unsupported {
    Introduction,
    MacroIdentity,
    MacroPackage,
    HelperOrComposite,
    ExternalExpansion,
    Configuration,
    SourceMapping,
    CalleeIdentity,
    TraitDispatch,
    CalleePackage,
    ApiSummary,
    Ownership,
    Cancellation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Advisory(PartialEffect),
    /// Excluded from this rule; never a certificate of application safety.
    NoAdvisory(Exclusion),
    /// First unsatisfied precondition in a documented, deterministic order.
    Unsupported(Unsupported),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Assessment {
    /// Retain every supplied fact, including Unknown repetition context.
    pub fact: ArmFact,
    pub outcome: Outcome,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Report<'a> {
    pub rule_id: &'static str,
    pub summary_version: &'static str,
    pub limitations: &'static [&'static str],
    pub logical_document: &'a str,
    pub source_sha256: [u8; 32],
    pub context: ContextIdentity,
    pub producer: &'a str,
    pub input_coverage: Coverage,
    /// True only for supplied facts with complete nomination coverage, no
    /// unsupported assessment and no omitted record. Not whole-program safety.
    pub complete_for_supplied_facts: bool,
    pub examined: usize,
    pub advisories: usize,
    pub unsupported: usize,
    pub omitted_records: usize,
    /// Ordered by original future span, then select span, regardless of input order.
    pub records: Vec<Assessment>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputError {
    InvalidIdentity,
    MissingContextDigest,
    SourceTooLarge,
    InvalidRecordLimit,
    TooManyOperations,
    IdentityMismatch,
    InconsistentCoverage,
    InvalidSpan,
    DuplicateFutureSpan,
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid cancellation-advisory input: {self:?}")
    }
}

impl std::error::Error for InputError {}

/// Explicit opt-in. Disabled returns None without inspecting input. Enabled
/// input is rejected atomically on identity/structural errors; there is no
/// partial advisory on stale bytes or an invalid span. Caps are never raised.
///
/// Work is O(source bytes + n log n), n <= 256, with O(n + max_records) extra
/// storage. No source-sized allocation, parser, filesystem, clock, or process.
/// All n facts are assessed; an output cap retains the earliest records and
/// exposes exact omitted/count totals. Caller allocations are outside this cap.
pub fn assess<'a>(
    unit: SourceUnit<'a>,
    facts: TypedFacts<'a>,
    options: Options,
) -> Result<Option<Report<'a>>, InputError> {
    if !options.enabled {
        return Ok(None);
    }
    if !valid_identity(unit.logical_document, 512) || !valid_identity(facts.producer, 128) {
        return Err(InputError::InvalidIdentity);
    }
    if unit.context.project_sha256 == [0; 32] || unit.context.comparison_sha256 == [0; 32] {
        return Err(InputError::MissingContextDigest);
    }
    if unit.source.len() > MAX_SOURCE_BYTES {
        return Err(InputError::SourceTooLarge);
    }
    if options.max_records == 0 || options.max_records > MAX_OPERATIONS {
        return Err(InputError::InvalidRecordLimit);
    }
    if facts.operations.len() > MAX_OPERATIONS {
        return Err(InputError::TooManyOperations);
    }
    let mut digest = [0; 32];
    digest.copy_from_slice(&Sha256::digest(unit.source.as_bytes()));
    if unit.logical_document != facts.logical_document
        || digest != facts.source_sha256 || unit.context != facts.context
    {
        return Err(InputError::IdentityMismatch);
    }
    if facts.coverage == Coverage::Unavailable && !facts.operations.is_empty() {
        return Err(InputError::InconsistentCoverage);
    }
    for fact in facts.operations {
        if !valid_span(unit.source, fact.select_span)
            || !valid_span(unit.source, fact.future_span)
            || !fact.select_span.contains(fact.future_span)
        {
            return Err(InputError::InvalidSpan);
        }
    }
    let mut ordered: Vec<_> = facts.operations.iter().collect();
    ordered.sort_unstable_by_key(|fact| (
        fact.future_span.start, fact.future_span.end,
        fact.select_span.start, fact.select_span.end,
    ));
    if ordered.windows(2).any(|pair| pair[0].future_span == pair[1].future_span) {
        return Err(InputError::DuplicateFutureSpan);
    }
    let mut records = Vec::with_capacity(ordered.len().min(options.max_records));
    let mut advisories = 0;
    let mut unsupported = 0;
    for fact in ordered {
        let outcome = classify(*fact);
        advisories += usize::from(matches!(outcome, Outcome::Advisory(_)));
        unsupported += usize::from(matches!(outcome, Outcome::Unsupported(_)));
        if records.len() < options.max_records {
            records.push(Assessment { fact: *fact, outcome });
        }
    }
    let omitted_records = facts.operations.len() - records.len();
    Ok(Some(Report {
        rule_id: RULE_ID,
        summary_version: SUMMARY_VERSION,
        limitations: LIMITATIONS,
        logical_document: unit.logical_document,
        source_sha256: digest,
        context: unit.context,
        producer: facts.producer,
        input_coverage: facts.coverage,
        complete_for_supplied_facts: facts.coverage == Coverage::Complete
            && unsupported == 0 && omitted_records == 0,
        examined: facts.operations.len(),
        advisories,
        unsupported,
        omitted_records,
        records,
    }))
}

fn valid_identity(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn valid_span(source: &str, span: Span) -> bool {
    span.start < span.end && span.end as usize <= source.len()
        && source.is_char_boundary(span.start as usize)
        && source.is_char_boundary(span.end as usize)
}

fn classify(fact: ArmFact) -> Outcome {
    use Outcome::{Advisory, NoAdvisory, Unsupported as Unknown};
    match fact.introduction {
        Introduction::Preexisting => return NoAdvisory(Exclusion::Preexisting),
        Introduction::Unknown => return Unknown(Unsupported::Introduction),
        Introduction::Introduced => {}
    }
    match fact.select_macro {
        SelectMacro::Other => return NoAdvisory(Exclusion::DifferentMacro),
        SelectMacro::Unresolved => return Unknown(Unsupported::MacroIdentity),
        SelectMacro::Tokio(TokioPackage::Canonical1_54_1) => {}
        SelectMacro::Tokio(_) => return Unknown(Unsupported::MacroPackage),
    }
    match fact.attribution {
        Attribution::HelperOrComposite => return Unknown(Unsupported::HelperOrComposite),
        Attribution::ExternalExpansion => return Unknown(Unsupported::ExternalExpansion),
        Attribution::UnknownConfiguration => return Unknown(Unsupported::Configuration),
        Attribution::UnknownSourceMapping => return Unknown(Unsupported::SourceMapping),
        Attribution::Direct => {}
    }
    let operation = match fact.callee {
        Callee::Other => return NoAdvisory(Exclusion::DifferentCallee),
        Callee::Unresolved => return Unknown(Unsupported::CalleeIdentity),
        Callee::AmbiguousTrait => return Unknown(Unsupported::TraitDispatch),
        Callee::TokioIo { package: TokioPackage::Canonical1_54_1, operation } => operation,
        Callee::TokioIo { .. } => return Unknown(Unsupported::CalleePackage),
    };
    let effect = match operation {
        IoOperation::ReadExact => PartialEffect::ReadProgress,
        IoOperation::WriteAll => PartialEffect::WriteProgress,
        IoOperation::Read | IoOperation::Write => return NoAdvisory(Exclusion::DocumentedSafeWait),
        IoOperation::Other => return Unknown(Unsupported::ApiSummary),
    };
    // These two reviewed constructors do not perform the summarized I/O before
    // polling. This exclusion says nothing about evaluating their arguments.
    if fact.cancellation == Cancellation::NeverPolled {
        return NoAdvisory(Exclusion::NeverPolled);
    }
    match fact.ownership {
        Ownership::RetainedOutsideSelect => return NoAdvisory(Exclusion::RetainedFuture),
        Ownership::Unknown => return Unknown(Unsupported::Ownership),
        Ownership::OwnedBySelect => {}
    }
    match fact.cancellation {
        Cancellation::MayDropPending => Advisory(effect),
        Cancellation::NoPendingDrop => NoAdvisory(Exclusion::NoPendingDrop),
        Cancellation::Unknown => Unknown(Unsupported::Cancellation),
        Cancellation::NeverPolled => NoAdvisory(Exclusion::NeverPolled),
    }
}
