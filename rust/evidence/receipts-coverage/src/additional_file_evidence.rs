//! SH-FOLLOW-08: bounded transport of the exact managed settings witness.
//!
//! Parsing this sidecar is not admission. The trusted caller must authenticate
//! the managed producer, establish the CURRENT supervisor context, qualify one
//! AdditionalFiles observation, and validate the complete physical mapping. In
//! particular, the provider label and equal digests do not establish that trust.
//! The managed route does not yet produce evaluated AdditionalFiles attribution;
//! a separately admitted observation is therefore an explicit API argument.
//! No public argv, scheduler, publication, cache or execution route is added.

use crate::identity::{self, BuildInputs, CollectionMode, Stamp};
use crate::input_audit::{self, AdditionalFileObservation, Audit, BuildInputContext,
    CaptureCoherence, PathIdentity, SettingsFile, SettingsWitness};
use crate::{field, logical_path, valid_digest};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const SCHEMA: &str = "slophawk.additional-files-evidence.v1";
pub const PROVIDER: &str = "slophawk.managed-build-capture/1";
/// Applied before JSON parsing. Nested records/text also use the audit's caps.
pub const MAX_EVIDENCE_BYTES: usize = 256 * 1024;

/// These are advisory evidence dispositions, never `identity::Reuse` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceReason {
    EvidenceUnavailable,
    InvalidJson,
    UnsupportedSchema,
    UnsupportedProvider,
    BudgetExceeded,
    ContextMismatch,
    UnknownCollectionMode,
    IdentityIncomplete,
    IdentityMismatch,
    SettingsUnavailable,
    SettingsInvalid,
    SettingsDigestMismatch,
    EvaluatedAdditionalFilesUnavailable,
    CaptureCoherenceUnresolved,
    PathIdentityUnresolved,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureAudit {
    /// A prerequisite is absent; no membership claim can be made.
    Unavailable(EvidenceReason),
    /// Supplied evidence is malformed, stale, over budget or inconsistent.
    Rejected(EvidenceReason),
    /// The unchanged v1 audit's single, limited diagnostic.
    Advisory(Audit),
}

// Private wire records deliberately cannot be used as a public admission type.
// No new admission, PathIdentity, coherence or observation flags are decoded.
// The existing v1 path_mapping_trusted field is identity data, not authority.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    schema: String,
    provider: String,
    binding: Binding,
    identity_inputs: BuildInputs,
    stamp: Stamp,
    settings_files: Option<Vec<SettingsRecord>>,
    observation_status: ObservationStatus,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    root_identity: String,
    epoch: String,
    revision: u64,
    operation_id: String,
    capture_id: String,
    target_id: String,
    mapping_identity: String,
    collection_mode: CollectionMode,
    build_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsRecord { path: String, sha256: String, length: i64 }

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ObservationStatus { EvaluatedAdditionalFilesUnavailable }

#[derive(Default)]
struct Budget { records: usize, text_bytes: usize }

impl Budget {
    fn records(&mut self, count: usize) -> bool {
        if count > input_audit::MAX_RECORDS - self.records { return false; }
        self.records += count;
        true
    }

    fn text(&mut self, text: Option<&str>) -> bool {
        let bytes = text.map_or(0, str::len);
        if bytes > input_audit::MAX_FIELD_BYTES ||
            bytes > input_audit::MAX_TOTAL_TEXT_BYTES - self.text_bytes { return false; }
        self.text_bytes += bytes;
        true
    }

    fn context(&mut self, context: &BuildInputContext<'_>) -> bool {
        context_fields(context).into_iter().all(|text| self.text(Some(text)))
    }

    fn identity(&mut self, input: &BuildInputs, stamp: &Stamp) -> bool {
        let target = &input.target;
        for count in [target.compile.as_ref().map_or(0, Vec::len),
            target.references.as_ref().map_or(0, Vec::len),
            input.ordered_compile_inputs.as_ref().map_or(0, Vec::len),
            input.reference_assemblies.as_ref().map_or(0, Vec::len),
            input.runtime_assets.as_ref().map_or(0, Vec::len),
            input.declared_outputs.as_ref().map_or(0, Vec::len),
            stamp.sections.len(), stamp.problems.len()] {
            if !self.records(count) { return false; }
        }
        for text in [target.id.as_deref(), target.project_path.as_deref(),
            target.framework.as_deref(), target.configuration.as_deref(),
            target.platform.as_deref(), target.runtime_identifier.as_deref(),
            target.sdk_version.as_deref(), target.evaluation_fingerprint.as_deref(),
            input.path_mapping_identity.as_deref(), input.settings_sha256.as_deref(),
            input.runner_identity.as_deref(), input.adapter_identity.as_deref(),
            Some(stamp.sha256.as_str())] {
            if !self.text(text) { return false; }
        }
        for item in target.compile.iter().flatten() {
            if !self.text(Some(&item.path)) || !self.text(item.link.as_deref()) { return false; }
        }
        for item in target.references.iter().flatten() {
            if !self.text(Some(item)) { return false; }
        }
        for item in input.ordered_compile_inputs.iter().flatten() {
            if !self.text(Some(&item.path)) || !self.text(item.link.as_deref()) ||
                !self.text(item.sha256.as_deref()) { return false; }
        }
        for list in [&input.reference_assemblies, &input.runtime_assets] {
            for item in list.iter().flatten() {
                if !self.text(Some(&item.path)) || !self.text(item.sha256.as_deref()) { return false; }
            }
        }
        for item in input.declared_outputs.iter().flatten() {
            if !self.text(Some(&item.path)) || !self.text(item.sha256.as_deref()) { return false; }
        }
        for section in &stamp.sections {
            if !self.text(Some(&section.name)) || !self.text(Some(&section.sha256)) { return false; }
        }
        for problem in &stamp.problems {
            if !self.text(Some(&problem.field)) { return false; }
        }
        true
    }
}

fn context_fields<'a>(context: &BuildInputContext<'a>) -> [&'a str; 8] {
    [context.root_identity, context.epoch, context.operation_id, context.capture_id,
        context.provider, context.target_id, context.mapping_identity, context.build_sha256]
}

fn binding_fields(binding: &Binding) -> [&str; 7] {
    [&binding.root_identity, &binding.epoch, &binding.operation_id, &binding.capture_id,
        &binding.target_id, &binding.mapping_identity, &binding.build_sha256]
}

fn matches(binding: &Binding, expected: &BuildInputContext<'_>) -> bool {
    binding.root_identity == expected.root_identity && binding.epoch == expected.epoch &&
        binding.revision == expected.revision && binding.operation_id == expected.operation_id &&
        binding.capture_id == expected.capture_id && binding.target_id == expected.target_id &&
        binding.mapping_identity == expected.mapping_identity &&
        binding.collection_mode == expected.collection_mode && binding.build_sha256 == expected.build_sha256
}

fn canonical_path(path: &str) -> bool {
    logical_path(path, input_audit::MAX_FIELD_BYTES).as_deref() == Some(path)
}

fn hash_text(hash: &mut Sha256, value: &str) {
    hash.update((value.len() as i32).to_be_bytes());
    hash.update(value.as_bytes());
}

// Validate independently of attribution so a malformed settings list is not
// hidden behind the normal producer's missing AdditionalFiles observation.
// This is the existing SG-13 settings wire; no new identity domain is introduced.
fn settings_digest(files: &[SettingsRecord]) -> Result<String, EvidenceReason> {
    if files.iter().any(|file| !canonical_path(&file.path) ||
        !valid_digest(&file.sha256) || file.length < 0) {
        return Err(EvidenceReason::SettingsInvalid);
    }
    let mut sorted: Vec<_> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.encode_utf16().cmp(b.path.encode_utf16()));
    if sorted.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return Err(EvidenceReason::SettingsInvalid);
    }
    let mut hash = Sha256::new();
    hash_text(&mut hash, "SlopHawk/SG-13/settings/v1");
    hash.update((sorted.len() as i32).to_be_bytes());
    for file in sorted {
        hash_text(&mut hash, &file.path);
        hash_text(&mut hash, &file.sha256.to_ascii_uppercase());
        hash.update(file.length.to_be_bytes());
    }
    Ok(format!("{:X}", hash.finalize()))
}

/// Inspect a sidecar from an authenticated, current managed capture and audit
/// ONE separately admitted observation. `expected` must originate from current
/// supervisor state, never this JSON. `settings_path_identity` independently
/// attests the entire settings mapping, including aliases/platform ambiguity;
/// per-file double reads and a provider string cannot supply that attestation.
///
/// The observation must belong to this capture's exact operation/target/mapping
/// and be coherent with its entire settings witness. Do not relabel an old
/// observation or set ProviderVerifiedStable because two file reads agreed.
/// Missing evidence and all rejected sidecars leave v1 reuse authority unchanged.
pub fn audit_capture(bytes: Option<&[u8]>, expected: &BuildInputContext<'_>,
    observation: Option<&AdditionalFileObservation<'_>>,
    settings_path_identity: PathIdentity) -> CaptureAudit {
    use CaptureAudit::{Advisory, Rejected, Unavailable};
    use EvidenceReason as Reason;

    let Some(bytes) = bytes else { return Unavailable(Reason::EvidenceUnavailable); };
    if bytes.len() > MAX_EVIDENCE_BYTES { return Rejected(Reason::BudgetExceeded); }
    let envelope: Envelope = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return Rejected(Reason::InvalidJson),
    };
    let mut budget = Budget::default();
    if !budget.text(Some(&envelope.schema)) || !budget.text(Some(&envelope.provider)) ||
        !budget.context(expected) ||
        !binding_fields(&envelope.binding).into_iter().all(|text| budget.text(Some(text))) ||
        !budget.identity(&envelope.identity_inputs, &envelope.stamp) {
        return Rejected(Reason::BudgetExceeded);
    }
    if let Some(observation) = observation {
        if !budget.context(&observation.context) || !budget.text(observation.path) ||
            !budget.text(observation.sha256) { return Rejected(Reason::BudgetExceeded); }
    }
    if let Some(files) = &envelope.settings_files {
        if !budget.records(files.len()) { return Rejected(Reason::BudgetExceeded); }
        for file in files {
            if !budget.text(Some(&file.path)) || !budget.text(Some(&file.sha256)) {
                return Rejected(Reason::BudgetExceeded);
            }
        }
    }
    if envelope.schema != SCHEMA { return Rejected(Reason::UnsupportedSchema); }
    if envelope.provider != PROVIDER || expected.provider != PROVIDER {
        return Rejected(Reason::UnsupportedProvider);
    }
    if !context_fields(expected).into_iter().all(|text| field(text, input_audit::MAX_FIELD_BYTES)) ||
        !valid_digest(expected.build_sha256) || !matches(&envelope.binding, expected) ||
        observation.is_some_and(|value| value.context != *expected) ||
        envelope.identity_inputs.target.id.as_deref() != Some(expected.target_id) ||
        envelope.identity_inputs.path_mapping_identity.as_deref() != Some(expected.mapping_identity) ||
        envelope.identity_inputs.collection_mode != expected.collection_mode {
        return Rejected(Reason::ContextMismatch);
    }
    if expected.collection_mode == CollectionMode::Unknown {
        return Unavailable(Reason::UnknownCollectionMode);
    }
    let recomputed = identity::capture(&envelope.identity_inputs);
    if !recomputed.complete { return Unavailable(Reason::IdentityIncomplete); }
    if recomputed != envelope.stamp || envelope.stamp.sha256 != expected.build_sha256 {
        return Rejected(Reason::IdentityMismatch);
    }
    let Some(files) = &envelope.settings_files else { return Unavailable(Reason::SettingsUnavailable); };
    let digest = match settings_digest(files) {
        Ok(value) => value,
        Err(reason) => return Rejected(reason),
    };
    if !envelope.identity_inputs.settings_sha256.as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case(&digest)) {
        return Rejected(Reason::SettingsDigestMismatch);
    }
    // The status is descriptive of this producer, not a permission switch. An
    // external qualified observation may exist even while this producer lacks it.
    let ObservationStatus::EvaluatedAdditionalFilesUnavailable = envelope.observation_status;
    let Some(observation) = observation else {
        return Unavailable(Reason::EvaluatedAdditionalFilesUnavailable);
    };
    if expected.coherence != CaptureCoherence::ProviderVerifiedStable {
        return Unavailable(Reason::CaptureCoherenceUnresolved);
    }
    if settings_path_identity != PathIdentity::ProviderVerifiedUnlinked {
        return Unavailable(Reason::PathIdentityUnresolved);
    }
    let borrowed: Vec<_> = files.iter().map(|file| SettingsFile {
        path: &file.path, sha256: &file.sha256, length: file.length,
    }).collect();
    let witness = SettingsWitness {
        context: *expected, path_identity: settings_path_identity, files: &borrowed,
    };
    Advisory(input_audit::audit_additional_file(&envelope.identity_inputs, &envelope.stamp,
        expected, observation, Some(&witness)))
}
