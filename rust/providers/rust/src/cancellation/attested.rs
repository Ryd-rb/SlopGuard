//! Admission of one externally resolved, owner-approved read_exact arm.
//!
//! This is an in-process attestation boundary, not a resolver, signature verifier
//! or proof checker. `Approval` must come from an independently trusted owner
//! channel after review of the producer, loaded bytes and evidence. Never derive
//! production approval from the receipt being admitted. Matching hashes bind
//! approved claims; they do not establish that those claims are true.
//!
//! No syntax fact, Cargo package name/version, LSP display string or changed line
//! supplies approval. The existing raw `super::assess` interface remains trusted
//! input and is not retroactively protected by this optional adapter.

use super::{
    ArmFact, Attribution, Callee, Cancellation, ContextIdentity, Coverage,
    InputError, Introduction, IoOperation, MAX_OPERATIONS, Options, Ownership,
    Repetition, SelectMacro, SourceUnit, TokioPackage, TypedFacts, valid_identity,
    valid_span,
};
use crate::{MAX_SOURCE_BYTES, SourceEdition, Span};
use sha2::{Digest, Sha256};
use std::fmt;

pub const ADAPTER_ID: &str = "slophawk.rust.attested-read-exact/1";
pub const RECEIPT_SCHEMA: u32 = 1;
pub const MAX_ARMS: usize = 1;
/// Official Tokio 1.54.1 .crate checksum, not the loaded-source fingerprint.
pub const TOKIO_ARCHIVE_SHA256: [u8; 32] = [
    0xb5, 0x2f, 0xd3, 0x25, 0x52, 0x58, 0xd2, 0x6f,
    0xac, 0x72, 0x64, 0xab, 0x5a, 0x76, 0xb5, 0x3c,
    0x13, 0x36, 0x71, 0x36, 0xdc, 0x01, 0x1d, 0x3d,
    0x6a, 0xd4, 0xf8, 0xed, 0x07, 0x03, 0x96, 0xa3,
];

/// Producer-defined, context-bound semantic identity, never a path spelling.
/// Its qualified derivation must include the actual defining crate and item.
pub type DefinitionId = [u8; 32];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacroResolution {
    Resolved(DefinitionId),
    /// Positively resolved to a different macro, not a failed lookup.
    Other,
    Unresolved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MethodResolution {
    /// The actual selected trait implementation/body, including receiver
    /// dispatch, is the reviewed Tokio AsyncReadExt::read_exact implementation.
    Resolved(DefinitionId),
    /// Positively resolved to a non-Tokio method, such as a custom trait item.
    /// Other Tokio APIs are outside the admitted method class; use OtherTokioApi.
    Other,
    AmbiguousTrait,
    Unresolved,
    /// Positively resolved to official Tokio, but outside read_exact. Preserve
    /// unsupported API coverage instead of claiming a different/safe callee.
    OtherTokioApi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Configuration {
    Active,
    Inactive,
    Unknown,
}

/// The independently supplied owner expectation. None of these values may be
/// copied from an unqualified receipt as a way to authorize that receipt.
#[derive(Clone, Copy, Debug)]
pub struct Approval<'a> {
    pub producer: &'a str,
    /// Digest of the reviewed producer/version/options and its qualification.
    pub qualification_sha256: [u8; 32],
    /// `receipt_sha256` of the exact independently reviewed claims below.
    pub receipt_sha256: [u8; 32],
    /// Fingerprint of actual loaded source bytes independently compared with
    /// official Tokio 1.54.1. A registry checksum alone does not supply this.
    pub loaded_package_sha256: [u8; 32],
    /// Actual supporting macro bytes, expansions and source maps, including
    /// tokio-macros helpers, under the bound configuration and producer.
    pub macro_closure_sha256: [u8; 32],
    pub select_definition: DefinitionId,
    pub read_exact_definition: DefinitionId,
}

/// One original-source nomination, with external attestations. Evidence digests
/// identify reviewed artifacts retained by the owner; this library does not
/// fetch or authenticate those artifacts. ContextIdentity's comparison digest
/// must cover both revisions/contexts and the introduction policy. Merely
/// changing an enclosing function or line does not establish introduction.
#[derive(Clone, Copy, Debug)]
pub struct ReadExactReceipt<'a> {
    pub schema_version: u32,
    pub producer: &'a str,
    pub qualification_sha256: [u8; 32],
    pub logical_document: &'a str,
    pub source_sha256: [u8; 32],
    pub context: ContextIdentity,
    pub tokio_archive_sha256: [u8; 32],
    pub loaded_package_sha256: [u8; 32],
    pub macro_closure_sha256: [u8; 32],
    /// Resolution, mapping, active cfg, owner fate and poll/drop reasoning.
    pub semantic_evidence_sha256: [u8; 32],
    /// Cross-revision evidence for Introduced/Preexisting; may be zero only
    /// for Unknown. It is bound into the approved receipt with the context.
    pub introduction_evidence_sha256: [u8; 32],
    pub select_span: Span,
    pub future_span: Span,
    pub select: MacroResolution,
    pub method: MethodResolution,
    pub configuration: Configuration,
    pub attribution: Attribution,
    pub introduction: Introduction,
    pub ownership: Ownership,
    pub cancellation: Cancellation,
    pub repetition: Repetition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unavailable {
    MissingApproval,
    MissingReceipt,
    InactiveConfiguration,
}

/// Zero or one facts. Complete nomination coverage means this one explicitly
/// supplied arm only, never a file scan or whole-program cancellation safety.
#[derive(Debug)]
pub struct AdaptedFacts<'a> {
    unavailable: Option<Unavailable>,
    approved_receipt_sha256: Option<[u8; 32]>,
    unit: SourceUnit<'a>,
    source_sha256: [u8; 32],
    producer: &'a str,
    operations: Option<[ArmFact; MAX_ARMS]>,
}

impl AdaptedFacts<'_> {
    pub fn unavailable(&self) -> Option<Unavailable> {
        self.unavailable
    }

    pub fn approved_receipt_sha256(&self) -> Option<[u8; 32]> {
        self.approved_receipt_sha256
    }

    /// Borrow these facts for the unchanged opt-in `cancellation::assess`.
    pub fn typed_facts(&self) -> TypedFacts<'_> {
        TypedFacts {
            logical_document: self.unit.logical_document,
            source_sha256: self.source_sha256,
            context: self.unit.context,
            producer: self.producer,
            coverage: if self.unavailable.is_some() {
                Coverage::Unavailable
            } else {
                Coverage::Complete
            },
            operations: self.operations.as_ref().map_or(&[], |arms| &arms[..]),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Input(InputError),
    UnsupportedSchema,
    InvalidApproval,
    ProducerMismatch,
    QualificationMismatch,
    ReceiptMismatch,
    UnreviewedArchive,
    LoadedPackageMismatch,
    MacroClosureMismatch,
    MissingSemanticEvidence,
    MissingIntroductionEvidence,
    DefinitionMismatch,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid attested read_exact input: {self:?}")
    }
}

impl std::error::Error for Error {}

/// Disabled returns None without inspection. Missing approval/receipt yields
/// unavailable empty facts. Other binding errors reject atomically. This call
/// performs no parsing, file access, clock access, process or future execution.
/// Work is O(source bytes), with fixed storage for one ArmFact. Caller-owned
/// evidence production and storage are outside this bound.
pub fn adapt<'a>(
    unit: SourceUnit<'a>,
    approval: Option<Approval<'a>>,
    receipt: Option<ReadExactReceipt<'a>>,
    options: Options,
) -> Result<Option<AdaptedFacts<'a>>, Error> {
    if !options.enabled {
        return Ok(None);
    }
    if !valid_identity(unit.logical_document, 512) {
        return Err(Error::Input(InputError::InvalidIdentity));
    }
    if unit.context.project_sha256 == [0; 32] || unit.context.comparison_sha256 == [0; 32] {
        return Err(Error::Input(InputError::MissingContextDigest));
    }
    if unit.source.len() > MAX_SOURCE_BYTES {
        return Err(Error::Input(InputError::SourceTooLarge));
    }
    if options.max_records == 0 || options.max_records > MAX_OPERATIONS {
        return Err(Error::Input(InputError::InvalidRecordLimit));
    }
    let source_sha256: [u8; 32] = Sha256::digest(unit.source.as_bytes()).into();
    let mut out = AdaptedFacts {
        unavailable: None, approved_receipt_sha256: None,
        unit, source_sha256, producer: ADAPTER_ID, operations: None,
    };
    let Some(approval) = approval else {
        out.unavailable = Some(Unavailable::MissingApproval);
        return Ok(Some(out));
    };
    let Some(receipt) = receipt else {
        out.unavailable = Some(Unavailable::MissingReceipt);
        return Ok(Some(out));
    };
    validate_approval(approval)?;
    // Hashing validates the bounded identities and the exact schema first.
    let receipt_digest = receipt_sha256(receipt)?;
    if receipt.producer != approval.producer {
        return Err(Error::ProducerMismatch);
    }
    if receipt.qualification_sha256 != approval.qualification_sha256 {
        return Err(Error::QualificationMismatch);
    }
    if unit.logical_document != receipt.logical_document
        || source_sha256 != receipt.source_sha256 || unit.context != receipt.context
    {
        return Err(Error::Input(InputError::IdentityMismatch));
    }
    if receipt_digest != approval.receipt_sha256 {
        return Err(Error::ReceiptMismatch);
    }
    if receipt.tokio_archive_sha256 != TOKIO_ARCHIVE_SHA256 {
        return Err(Error::UnreviewedArchive);
    }
    if receipt.loaded_package_sha256 != approval.loaded_package_sha256 {
        return Err(Error::LoadedPackageMismatch);
    }
    if receipt.macro_closure_sha256 != approval.macro_closure_sha256 {
        return Err(Error::MacroClosureMismatch);
    }
    if receipt.semantic_evidence_sha256 == [0; 32] {
        return Err(Error::MissingSemanticEvidence);
    }
    if receipt.introduction != Introduction::Unknown
        && receipt.introduction_evidence_sha256 == [0; 32]
    {
        return Err(Error::MissingIntroductionEvidence);
    }
    if !valid_span(unit.source, receipt.select_span)
        || !valid_span(unit.source, receipt.future_span)
        || !receipt.select_span.contains(receipt.future_span)
    {
        return Err(Error::Input(InputError::InvalidSpan));
    }
    let select_macro = match receipt.select {
        MacroResolution::Resolved(id) if id == approval.select_definition => {
            SelectMacro::Tokio(TokioPackage::Canonical1_54_1)
        }
        MacroResolution::Resolved(_) => return Err(Error::DefinitionMismatch),
        MacroResolution::Other => SelectMacro::Other,
        MacroResolution::Unresolved => SelectMacro::Unresolved,
    };
    let callee = match receipt.method {
        MethodResolution::Resolved(id) if id == approval.read_exact_definition => {
            Callee::TokioIo {
                package: TokioPackage::Canonical1_54_1, operation: IoOperation::ReadExact,
            }
        }
        MethodResolution::Resolved(_) => return Err(Error::DefinitionMismatch),
        MethodResolution::Other => Callee::Other,
        MethodResolution::AmbiguousTrait => Callee::AmbiguousTrait,
        MethodResolution::Unresolved => Callee::Unresolved,
        MethodResolution::OtherTokioApi => Callee::TokioIo {
            package: TokioPackage::Canonical1_54_1, operation: IoOperation::Other,
        },
    };
    out.producer = receipt.producer;
    out.approved_receipt_sha256 = Some(receipt_digest);
    if receipt.configuration == Configuration::Inactive {
        out.unavailable = Some(Unavailable::InactiveConfiguration);
        return Ok(Some(out));
    }
    out.operations = Some([ArmFact {
        select_span: receipt.select_span, future_span: receipt.future_span,
        select_macro, callee,
        attribution: if receipt.configuration == Configuration::Unknown {
            Attribution::UnknownConfiguration
        } else {
            receipt.attribution
        },
        introduction: receipt.introduction, ownership: receipt.ownership,
        cancellation: receipt.cancellation, repetition: receipt.repetition,
    }]);
    Ok(Some(out))
}

fn validate_approval(approval: Approval<'_>) -> Result<(), Error> {
    if !valid_identity(approval.producer, 128)
        || [approval.qualification_sha256, approval.receipt_sha256,
            approval.loaded_package_sha256, approval.macro_closure_sha256,
            approval.select_definition, approval.read_exact_definition]
            .contains(&[0; 32])
        || approval.select_definition == approval.read_exact_definition
    {
        return Err(Error::InvalidApproval);
    }
    Ok(())
}

/// Canonical v1 content digest for an independent review/approval channel. This
/// function grants no authority. Length-prefixed UTF-8 text, little-endian u32
/// scalars, fixed 32-byte digests and explicit enum tags are used; Debug output,
/// memory layout and enum declaration order are not a serialization contract.
/// Source bytes themselves are bound by source_sha256 and rehashed in adapt.
pub fn receipt_sha256(receipt: ReadExactReceipt<'_>) -> Result<[u8; 32], Error> {
    if receipt.schema_version != RECEIPT_SCHEMA {
        return Err(Error::UnsupportedSchema);
    }
    if !valid_identity(receipt.producer, 128) || !valid_identity(receipt.logical_document, 512) {
        return Err(Error::Input(InputError::InvalidIdentity));
    }
    let mut hash = Sha256::new();
    hash.update(b"slophawk.rust.attested-read-exact\0");
    hash.update(receipt.schema_version.to_le_bytes());
    for value in [receipt.producer, receipt.logical_document] {
        hash.update((value.len() as u32).to_le_bytes());
        hash.update(value.as_bytes());
    }
    for value in [
        receipt.qualification_sha256, receipt.source_sha256,
        receipt.context.project_sha256, receipt.context.comparison_sha256,
        receipt.tokio_archive_sha256, receipt.loaded_package_sha256,
        receipt.macro_closure_sha256, receipt.semantic_evidence_sha256,
        receipt.introduction_evidence_sha256,
    ] {
        hash.update(value);
    }
    hash.update([match receipt.context.edition {
        SourceEdition::E2015 => 0, SourceEdition::E2018 => 1,
        SourceEdition::E2021 => 2, SourceEdition::E2024 => 3,
    }]);
    for value in [receipt.select_span.start, receipt.select_span.end,
        receipt.future_span.start, receipt.future_span.end]
    {
        hash.update(value.to_le_bytes());
    }
    match receipt.select {
        MacroResolution::Resolved(id) => { hash.update([0]); hash.update(id); }
        MacroResolution::Other => hash.update([1]),
        MacroResolution::Unresolved => hash.update([2]),
    }
    match receipt.method {
        MethodResolution::Resolved(id) => { hash.update([0]); hash.update(id); }
        MethodResolution::Other => hash.update([1]),
        MethodResolution::AmbiguousTrait => hash.update([2]),
        MethodResolution::Unresolved => hash.update([3]),
        MethodResolution::OtherTokioApi => hash.update([4]),
    }
    hash.update([match receipt.configuration {
        Configuration::Active => 0, Configuration::Inactive => 1, Configuration::Unknown => 2,
    }]);
    hash.update([match receipt.attribution {
        Attribution::Direct => 0, Attribution::HelperOrComposite => 1,
        Attribution::ExternalExpansion => 2, Attribution::UnknownConfiguration => 3,
        Attribution::UnknownSourceMapping => 4,
    }]);
    hash.update([match receipt.introduction {
        Introduction::Introduced => 0, Introduction::Preexisting => 1, Introduction::Unknown => 2,
    }]);
    hash.update([match receipt.ownership {
        Ownership::OwnedBySelect => 0, Ownership::RetainedOutsideSelect => 1, Ownership::Unknown => 2,
    }]);
    hash.update([match receipt.cancellation {
        Cancellation::MayDropPending => 0, Cancellation::NeverPolled => 1,
        Cancellation::NoPendingDrop => 2, Cancellation::Unknown => 3,
    }]);
    hash.update([match receipt.repetition {
        Repetition::RecreatedInLoop => 0, Repetition::OneShot => 1, Repetition::Unknown => 2,
    }]);
    Ok(hash.finalize().into())
}
