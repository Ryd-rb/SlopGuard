//! SG-CHECK-R04: advisory AdditionalFiles byte-membership audit of v1 settings.
//!
//! This is a trusted-caller library contract, not a receipt parser or an admission
//! capability. The caller must authenticate the provider, establish the CURRENT
//! build operation/root/epoch/revision, and validate stable file observations and
//! physical mappings before constructing these records. Equal strings/hashes do
//! not establish that trust. No public argv or Deserialize route is provided.
//! Live observations must be coherent with the witness at the same decision
//! capture, not merely two stable reads in one operation. Audit old and new arms
//! separately; never relabel an old witness with the current capture_id.
//!
//! A complete SG-13 settings witness can prove that one observed path/digest/length
//! contributed to this exact v1 stamp. A gap is only a candidate in that section:
//! other identity sections/aggregates may cover the input. Neither outcome proves
//! input closure, actual generator consumption, output relevance or a reuse error.

use crate::identity::{self, BuildInputs, CollectionMode, Stamp};
use crate::{field, logical_path, valid_digest};
use sha2::{Digest, Sha256};

/// Caps apply before v1 capture, sorting, hashing or cloning any caller text.
/// Records are counted across ALL identity vectors, stamp vectors and witness.
pub const MAX_RECORDS: usize = 4096;
pub const MAX_FIELD_BYTES: usize = 4096;
pub const MAX_TOTAL_TEXT_BYTES: usize = 1024 * 1024;

/// Identifies the build-input receipt class through this type and the exact build
/// invocation through operation_id. provider includes its implementation/version.
/// The expected value must come from current supervisor state, not the receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildInputContext<'a> {
    pub root_identity: &'a str,
    pub epoch: &'a str,
    pub revision: u64,
    pub operation_id: &'a str,
    pub capture_id: &'a str,
    pub provider: &'a str,
    pub target_id: &'a str,
    pub mapping_identity: &'a str,
    pub collection_mode: CollectionMode,
    pub coherence: CaptureCoherence,
    pub build_sha256: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureCoherence { ProviderVerifiedStable, Unknown }

/// Owned copy of the validated binding. Persist it with the diagnostic; publishing
/// still requires a fresh check against supervisor state and the original evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditBinding {
    pub root_identity: String,
    pub epoch: String,
    pub revision: u64,
    pub operation_id: String,
    pub capture_id: String,
    pub provider: String,
    pub target_id: String,
    pub mapping_identity: String,
    pub collection_mode: CollectionMode,
    pub build_sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationScope {
    /// Membership was attributed to the evaluated target's AdditionalFiles class.
    EvaluatedAdditionalFile,
    /// A request for generator closure includes reads outside that class.
    GeneratorWithUnresolvedReads,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputSemantics {
    /// Provider establishes that this request concerns unordered file bytes only.
    /// No position, Link, item metadata or path-dependent alias semantics is claimed.
    UnorderedFileBytes,
    OrderedOrMetadataSensitive,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathIdentity {
    /// Provider checked regular files, links, aliases and platform path identity
    /// under the bound mapping. This enum alone does not perform those checks.
    ProviderVerifiedUnlinked,
    AmbiguousOrLinked,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub struct AdditionalFileObservation<'a> {
    pub context: BuildInputContext<'a>,
    pub scope: ObservationScope,
    pub semantics: InputSemantics,
    pub path_identity: PathIdentity,
    pub path: Option<&'a str>,
    pub sha256: Option<&'a str>,
    pub length: Option<i64>,
}

/// One ByteVerified settings record from BuildArtifactCaptureResult.Evidence.
/// Preserve logical spelling; do not substitute a physical path or resolve links.
#[derive(Clone, Copy, Debug)]
pub struct SettingsFile<'a> {
    pub path: &'a str,
    pub sha256: &'a str,
    pub length: i64,
}

/// The entire settings list, not just the matching member or its aggregate hash.
/// Its digest is reconstructed using the existing managed SG-13/settings/v1 wire.
#[derive(Clone, Copy, Debug)]
pub struct SettingsWitness<'a> {
    pub context: BuildInputContext<'a>,
    pub path_identity: PathIdentity,
    pub files: &'a [SettingsFile<'a>],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditState { Covered, CandidateGap, Unknown, Unsupported }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditReason {
    CoveredBySettings,
    SettingsPathAbsent,
    SettingsDigestDiffers,
    SettingsLengthDiffers,
    ObservationIncomplete,
    ObservationInvalid,
    WitnessUnavailable,
    WitnessInvalid,
    WitnessDigestMismatch,
    ContextMismatch,
    IdentityMismatch,
    IdentityIncomplete,
    UnknownCollectionMode,
    CaptureCoherenceUnresolved,
    DynamicReadsUnresolved,
    OrderOrMetadataNotRepresented,
    InputSemanticsUnknown,
    PathIdentityUnresolved,
    BudgetExceeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditLimit {
    OtherIdentityRepresentationsUnchecked,
    InputClosureUnproven,
    ConsumptionAndBehaviorUnproven,
    ProviderAuthenticationExternal,
}

/// A single bounded diagnostic, not a Reuse value. No caller-supplied error text
/// is echoed. Evidence strings are copied only after validation and budget checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Audit {
    pub state: AuditState,
    pub reason: AuditReason,
    pub limits: [AuditLimit; 4],
    pub binding: Option<AuditBinding>,
    pub witness_records: usize,
    pub path: Option<String>,
    pub observed_sha256: Option<String>,
    pub represented_sha256: Option<String>,
    pub observed_length: Option<i64>,
    pub represented_length: Option<i64>,
}

fn result(state: AuditState, reason: AuditReason) -> Audit {
    Audit {
        state, reason,
        limits: [AuditLimit::OtherIdentityRepresentationsUnchecked,
            AuditLimit::InputClosureUnproven, AuditLimit::ConsumptionAndBehaviorUnproven,
            AuditLimit::ProviderAuthenticationExternal],
        binding: None, witness_records: 0, path: None, observed_sha256: None,
        represented_sha256: None, observed_length: None, represented_length: None,
    }
}

#[derive(Default)]
struct Budget { records: usize, text_bytes: usize }

impl Budget {
    fn records(&mut self, count: usize) -> bool {
        if count > MAX_RECORDS - self.records { return false; }
        self.records += count;
        true
    }

    fn text(&mut self, value: Option<&str>) -> bool {
        let bytes = value.map_or(0, str::len);
        if bytes > MAX_FIELD_BYTES || bytes > MAX_TOTAL_TEXT_BYTES - self.text_bytes {
            return false;
        }
        self.text_bytes += bytes;
        true
    }

    fn context(&mut self, context: &BuildInputContext<'_>) -> bool {
        context_text(context).into_iter().all(|text| self.text(Some(text)))
    }
}

fn context_text<'a>(context: &BuildInputContext<'a>) -> [&'a str; 8] {
    [context.root_identity, context.epoch, context.operation_id, context.capture_id,
        context.provider, context.target_id, context.mapping_identity, context.build_sha256]
}

fn identity_budget(input: &BuildInputs, stamp: &Stamp, budget: &mut Budget) -> bool {
    let target = &input.target;
    for count in [target.compile.as_ref().map_or(0, Vec::len),
        target.references.as_ref().map_or(0, Vec::len),
        input.ordered_compile_inputs.as_ref().map_or(0, Vec::len),
        input.reference_assemblies.as_ref().map_or(0, Vec::len),
        input.runtime_assets.as_ref().map_or(0, Vec::len),
        input.declared_outputs.as_ref().map_or(0, Vec::len),
        stamp.sections.len(), stamp.problems.len()] {
        if !budget.records(count) { return false; }
    }
    for text in [target.id.as_deref(), target.project_path.as_deref(),
        target.framework.as_deref(), target.configuration.as_deref(),
        target.platform.as_deref(), target.runtime_identifier.as_deref(),
        target.sdk_version.as_deref(), target.evaluation_fingerprint.as_deref(),
        input.path_mapping_identity.as_deref(), input.settings_sha256.as_deref(),
        input.runner_identity.as_deref(), input.adapter_identity.as_deref(),
        Some(stamp.sha256.as_str())] {
        if !budget.text(text) { return false; }
    }
    for item in target.compile.iter().flatten() {
        if !budget.text(Some(&item.path)) || !budget.text(item.link.as_deref()) { return false; }
    }
    for item in target.references.iter().flatten() {
        if !budget.text(Some(item)) { return false; }
    }
    for item in input.ordered_compile_inputs.iter().flatten() {
        if !budget.text(Some(&item.path)) || !budget.text(item.link.as_deref()) ||
            !budget.text(item.sha256.as_deref()) { return false; }
    }
    for list in [&input.reference_assemblies, &input.runtime_assets] {
        for item in list.iter().flatten() {
            if !budget.text(Some(&item.path)) || !budget.text(item.sha256.as_deref()) { return false; }
        }
    }
    for item in input.declared_outputs.iter().flatten() {
        if !budget.text(Some(&item.path)) || !budget.text(item.sha256.as_deref()) { return false; }
    }
    for section in &stamp.sections {
        if !budget.text(Some(&section.name)) || !budget.text(Some(&section.sha256)) { return false; }
    }
    for problem in &stamp.problems {
        if !budget.text(Some(&problem.field)) { return false; }
    }
    true
}

fn canonical_path(path: &str) -> bool {
    // Managed capture emits normalized logical paths. Refuse an alias spelling;
    // no case folding, filesystem access or Unicode normalization is performed.
    logical_path(path, MAX_FIELD_BYTES).as_deref() == Some(path)
}

fn hash_text(hash: &mut Sha256, value: &str) {
    hash.update((value.len() as i32).to_be_bytes());
    hash.update(value.as_bytes());
}

/// Audit one provider-attributed AdditionalFiles observation against ONLY the
/// settings aggregate of a bound, complete v1 build identity. All inputs are
/// borrowed, no state is changed and no file/tool/clock/network is accessed.
/// Missing or malformed evidence is never promoted to a CandidateGap.
pub fn audit_additional_file(input: &BuildInputs, stamp: &Stamp,
    expected: &BuildInputContext<'_>, observation: &AdditionalFileObservation<'_>,
    witness: Option<&SettingsWitness<'_>>) -> Audit {
    use AuditReason as Reason;
    use AuditState as State;

    let mut budget = Budget::default();
    if !identity_budget(input, stamp, &mut budget) || !budget.context(expected) ||
        !budget.context(&observation.context) || !budget.text(observation.path) ||
        !budget.text(observation.sha256) {
        return result(State::Unsupported, Reason::BudgetExceeded);
    }
    if let Some(witness) = witness {
        if !budget.records(witness.files.len()) || !budget.context(&witness.context) {
            return result(State::Unsupported, Reason::BudgetExceeded);
        }
        for file in witness.files {
            if !budget.text(Some(file.path)) || !budget.text(Some(file.sha256)) {
                return result(State::Unsupported, Reason::BudgetExceeded);
            }
        }
    }
    if !context_text(expected).into_iter().all(|text| field(text, MAX_FIELD_BYTES)) ||
        !valid_digest(expected.build_sha256) || observation.context != *expected ||
        witness.is_some_and(|value| value.context != *expected) ||
        input.target.id.as_deref() != Some(expected.target_id) ||
        input.path_mapping_identity.as_deref() != Some(expected.mapping_identity) ||
        input.collection_mode != expected.collection_mode {
        return result(State::Unknown, Reason::ContextMismatch);
    }
    if expected.collection_mode == CollectionMode::Unknown {
        return result(State::Unsupported, Reason::UnknownCollectionMode);
    }
    if expected.coherence == CaptureCoherence::Unknown {
        return result(State::Unknown, Reason::CaptureCoherenceUnresolved);
    }
    if observation.scope == ObservationScope::GeneratorWithUnresolvedReads {
        return result(State::Unknown, Reason::DynamicReadsUnresolved);
    }
    match observation.semantics {
        InputSemantics::OrderedOrMetadataSensitive =>
            return result(State::Unsupported, Reason::OrderOrMetadataNotRepresented),
        InputSemantics::Unknown => return result(State::Unknown, Reason::InputSemanticsUnknown),
        InputSemantics::UnorderedFileBytes => (),
    }
    if observation.path_identity != PathIdentity::ProviderVerifiedUnlinked ||
        witness.is_some_and(|value| value.path_identity != PathIdentity::ProviderVerifiedUnlinked) {
        return result(State::Unsupported, Reason::PathIdentityUnresolved);
    }
    let (Some(path), Some(digest), Some(length)) =
        (observation.path, observation.sha256, observation.length) else {
            return result(State::Unknown, Reason::ObservationIncomplete);
        };
    if !canonical_path(path) || !valid_digest(digest) || length < 0 {
        return result(State::Unknown, Reason::ObservationInvalid);
    }

    // Full recomputation also checks complete v1 domain/sections/problems. This
    // deliberately does not change capture, compare, closure_checked or v1 JSON.
    let recomputed = identity::capture(input);
    if !recomputed.complete {
        return result(State::Unknown, Reason::IdentityIncomplete);
    }
    if recomputed != *stamp || stamp.sha256 != expected.build_sha256 {
        return result(State::Unknown, Reason::IdentityMismatch);
    }
    let Some(witness) = witness else { return result(State::Unknown, Reason::WitnessUnavailable); };
    if witness.files.iter().any(|file|
        !canonical_path(file.path) || !valid_digest(file.sha256) || file.length < 0) {
        return result(State::Unknown, Reason::WitnessInvalid);
    }
    let mut sorted: Vec<_> = witness.files.iter().collect();
    // StringComparer.Ordinal compares UTF-16 code units, not UTF-8 bytes.
    sorted.sort_by(|a, b| a.path.encode_utf16().cmp(b.path.encode_utf16()));
    if sorted.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return result(State::Unknown, Reason::WitnessInvalid);
    }
    let mut hash = Sha256::new();
    hash_text(&mut hash, "SlopHawk/SG-13/settings/v1");
    hash.update((sorted.len() as i32).to_be_bytes());
    for file in &sorted {
        hash_text(&mut hash, file.path);
        hash_text(&mut hash, &file.sha256.to_ascii_uppercase());
        hash.update(file.length.to_be_bytes());
    }
    let aggregate = format!("{:X}", hash.finalize());
    if !input.settings_sha256.as_deref().is_some_and(|value| value.eq_ignore_ascii_case(&aggregate)) {
        return result(State::Unknown, Reason::WitnessDigestMismatch);
    }

    let member = sorted.iter().find(|file| file.path == path);
    let (state, reason) = match member {
        None => (State::CandidateGap, Reason::SettingsPathAbsent),
        Some(file) if !file.sha256.eq_ignore_ascii_case(digest) =>
            (State::CandidateGap, Reason::SettingsDigestDiffers),
        Some(file) if file.length != length => (State::CandidateGap, Reason::SettingsLengthDiffers),
        Some(_) => (State::Covered, Reason::CoveredBySettings),
    };
    let mut audit = result(state, reason);
    audit.binding = Some(AuditBinding {
        root_identity: expected.root_identity.into(), epoch: expected.epoch.into(),
        revision: expected.revision, operation_id: expected.operation_id.into(),
        capture_id: expected.capture_id.into(), provider: expected.provider.into(),
        target_id: expected.target_id.into(), mapping_identity: expected.mapping_identity.into(),
        collection_mode: expected.collection_mode, build_sha256: expected.build_sha256.into(),
    });
    audit.witness_records = sorted.len();
    audit.path = Some(path.into());
    audit.observed_sha256 = Some(digest.to_ascii_uppercase());
    audit.observed_length = Some(length);
    if let Some(file) = member {
        audit.represented_sha256 = Some(file.sha256.to_ascii_uppercase());
        audit.represented_length = Some(file.length);
    }
    audit
}
